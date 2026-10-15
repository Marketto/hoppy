//! LAN discovery over UDP multicast + broadcast (spec §2), plus the `callback`
//! datagram that lets a firewalled receiver connect back to the sender (spec §6).

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

use crate::protocol::{DISCOVERY_PORT, DeviceInfo, DiscoveryMessage, MULTICAST_ADDR, MessageKind};

pub const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
pub const PEER_TTL: Duration = Duration::from_secs(15);
/// How long a sender keeps its callback listener open and keeps asking (spec §6).
pub const CALLBACK_WINDOW: Duration = Duration::from_secs(20);
/// Sender re-sends the `callback` datagram this often (and on every announce/query of the target).
pub const CALLBACK_RESEND: Duration = Duration::from_secs(2);
/// Receiver-side TCP connect timeout when connecting back.
pub const CALLBACK_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Receiver-side cap on callbacks started per rolling minute (all sources together).
pub const MAX_CALLBACKS_PER_MINUTE: usize = 10;
/// Receiver remembers handled tokens this long to drop the sender's re-sends.
const TOKEN_MEMORY: Duration = Duration::from_secs(120);
const MAX_REMEMBERED_TOKENS: usize = 256;

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub me: DeviceInfo,
    /// TCP port where this node accepts transfers (announced to others).
    pub transfer_port: u16,
    /// UDP port to bind. `DISCOVERY_PORT` in production; tests use others.
    pub bind_port: u16,
    /// Extra unicast targets (for networks without multicast, or tests).
    pub seeds: Vec<SocketAddr>,
    /// Send to multicast/broadcast. Disable in tests to stay on loopback.
    pub use_multicast: bool,
}

impl DiscoveryConfig {
    pub fn new(me: DeviceInfo, transfer_port: u16) -> Self {
        Self {
            me,
            transfer_port,
            bind_port: DISCOVERY_PORT,
            seeds: Vec::new(),
            use_multicast: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub device: DeviceInfo,
    /// Address for the TCP transfer connection.
    pub addr: SocketAddr,
}

struct Seen {
    peer: Peer,
    /// UDP source of the peer's last datagram: where `callback` datagrams go.
    udp: SocketAddr,
    last: Instant,
}

/// A receiver-side request to connect back (spec §6), already validated and rate-limited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackRequest {
    /// The sender as it described itself in the datagram (display only, not authenticated).
    pub sender: DeviceInfo,
    /// UDP source IP of the datagram + the requested port. Never an IP from the JSON.
    pub addr: SocketAddr,
    pub token: String,
}

/// Receiver-side hook: connect to `addr`, send the callback hello, receive. Runs on its
/// own thread and may block for the whole transfer.
pub type CallbackHook = Arc<dyn Fn(CallbackRequest) + Send + Sync>;

/// Sender-side outstanding request (see [`Discovery::request_callback`]).
struct Pending {
    peer_id: String,
    port: u16,
    token: String,
}

struct Inner {
    socket: UdpSocket,
    config: DiscoveryConfig,
    peers: Mutex<HashMap<String, Seen>>,
    hook: Mutex<Option<CallbackHook>>,
    pending: Mutex<HashMap<u64, Pending>>,
    next_ticket: AtomicU64,
    limiter: Mutex<CallbackLimiter>,
}

/// Running discovery. Dropping it sends `bye` and stops the background threads.
pub struct Discovery {
    inner: Arc<Inner>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Discovery {
    pub fn start(config: DiscoveryConfig) -> io::Result<Self> {
        let socket = bind_socket(config.bind_port, config.use_multicast)?;
        let inner = Arc::new(Inner {
            socket,
            config,
            peers: Mutex::new(HashMap::new()),
            hook: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_ticket: AtomicU64::new(1),
            limiter: Mutex::new(CallbackLimiter::default()),
        });
        let stop = Arc::new(AtomicBool::new(false));

        let rx = {
            let (inner, stop) = (inner.clone(), stop.clone());
            thread::Builder::new()
                .name("hoppy-discovery-rx".into())
                .spawn(move || receive_loop(&inner, &stop))?
        };
        let tx = {
            let (inner, stop) = (inner.clone(), stop.clone());
            thread::Builder::new()
                .name("hoppy-discovery-tx".into())
                .spawn(move || {
                    let (socket, config) = (&inner.socket, &inner.config);
                    send_all(socket, config, MessageKind::Query);
                    // Send-only nodes (port 0) keep querying instead of announcing (spec §2 rule 6).
                    let periodic = if config.transfer_port == 0 {
                        MessageKind::Query
                    } else {
                        MessageKind::Announce
                    };
                    while !stop.load(Ordering::Relaxed) {
                        send_all(socket, config, periodic);
                        sleep_unless_stopped(ANNOUNCE_INTERVAL, &stop);
                    }
                })?
        };
        Ok(Self {
            inner,
            stop,
            threads: vec![rx, tx],
        })
    }

    /// UDP port actually bound (useful when `bind_port` is 0).
    pub fn local_port(&self) -> u16 {
        self.inner
            .socket
            .local_addr()
            .map(|a| a.port())
            .unwrap_or(0)
    }

    /// The identity this node announces.
    pub fn me(&self) -> &DeviceInfo {
        &self.inner.config.me
    }

    /// Asks everyone to announce themselves now (e.g. when the picker opens).
    pub fn refresh(&self) {
        send_all(&self.inner.socket, &self.inner.config, MessageKind::Query);
    }

    /// Receiver side (spec §6): run `hook` for every valid `callback` datagram addressed to
    /// this node. Only effective on receiving nodes (`transfer_port != 0`). Replaces any
    /// previous hook. Normally installed via `transfer::accept_callbacks`.
    pub fn on_callback(&self, hook: CallbackHook) {
        *self.inner.hook.lock().expect("hook poisoned") = Some(hook);
    }

    /// Sender side (spec §6): asks `peer` to connect back to our TCP `port`, proving itself
    /// with `token`. Sends one datagram now, and one more each time `peer` announces or
    /// queries while the ticket lives; call [`CallbackTicket::resend`] periodically too.
    pub fn request_callback(&self, peer: &Peer, port: u16, token: &str) -> CallbackTicket {
        let key = self.inner.next_ticket.fetch_add(1, Ordering::Relaxed);
        self.inner.pending.lock().expect("pending poisoned").insert(
            key,
            Pending {
                peer_id: peer.device.id.clone(),
                port,
                token: token.to_string(),
            },
        );
        let ticket = CallbackTicket {
            inner: self.inner.clone(),
            key,
            peer_id: peer.device.id.clone(),
            fallback: SocketAddr::new(peer.addr.ip(), DISCOVERY_PORT),
            payload: DiscoveryMessage::callback(
                &self.inner.config.me,
                port,
                token,
                &peer.device.id,
            )
            .encode(),
        };
        ticket.resend();
        ticket
    }

    /// Live peers, sorted by name.
    pub fn peers(&self) -> Vec<Peer> {
        let mut map = self.inner.peers.lock().expect("peer map poisoned");
        map.retain(|_, s| s.last.elapsed() < PEER_TTL);
        let mut list: Vec<Peer> = map.values().map(|s| s.peer.clone()).collect();
        list.sort_by(|a, b| {
            a.device
                .name
                .to_lowercase()
                .cmp(&b.device.name.to_lowercase())
        });
        list
    }

    /// Blocks until at least `min` peers are known or `timeout` expires.
    pub fn wait_for_peers(&self, min: usize, timeout: Duration) -> Vec<Peer> {
        let deadline = Instant::now() + timeout;
        loop {
            let peers = self.peers();
            if peers.len() >= min || Instant::now() >= deadline {
                return peers;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        send_all(&self.inner.socket, &self.inner.config, MessageKind::Bye);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// An outstanding sender-side callback request. Dropping it stops the re-sends.
pub struct CallbackTicket {
    inner: Arc<Inner>,
    key: u64,
    peer_id: String,
    /// Used when the peer's discovery address is unknown (e.g. it already expired).
    fallback: SocketAddr,
    payload: Vec<u8>,
}

impl CallbackTicket {
    /// Sends the `callback` datagram again to the peer's last known discovery address.
    pub fn resend(&self) {
        let target = self
            .inner
            .peers
            .lock()
            .expect("peer map poisoned")
            .get(&self.peer_id)
            .map(|s| s.udp)
            .unwrap_or(self.fallback);
        let _ = self.inner.socket.send_to(&self.payload, target);
    }
}

impl Drop for CallbackTicket {
    fn drop(&mut self) {
        if let Ok(mut p) = self.inner.pending.lock() {
            p.remove(&self.key);
        }
    }
}

/// Receiver-side admission control for callbacks (spec §6 "Limits").
#[derive(Default)]
pub(crate) struct CallbackLimiter {
    tokens: VecDeque<(String, Instant)>,
    outstanding: HashSet<IpAddr>,
    started: VecDeque<Instant>,
}

impl CallbackLimiter {
    /// `true` if a callback from `ip` with `token` may start now; then the caller must
    /// call [`Self::release`] when it ends.
    pub(crate) fn admit(&mut self, ip: IpAddr, token: &str, now: Instant) -> bool {
        while self
            .tokens
            .front()
            .is_some_and(|(_, t)| now.duration_since(*t) > TOKEN_MEMORY)
        {
            self.tokens.pop_front();
        }
        while self
            .started
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60))
        {
            self.started.pop_front();
        }
        if self.tokens.iter().any(|(t, _)| t == token)
            || self.outstanding.contains(&ip)
            || self.started.len() >= MAX_CALLBACKS_PER_MINUTE
        {
            return false;
        }
        if self.tokens.len() >= MAX_REMEMBERED_TOKENS {
            self.tokens.pop_front();
        }
        self.tokens.push_back((token.to_string(), now));
        self.outstanding.insert(ip);
        self.started.push_back(now);
        true
    }

    pub(crate) fn release(&mut self, ip: IpAddr) {
        self.outstanding.remove(&ip);
    }
}

/// Spec §6: a receiver only connects back to loopback, private (RFC 1918), CGNAT
/// (100.64/10), link-local, or IPv6 ULA/link-local addresses.
pub fn is_local_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_local_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_local_v4(v4);
            }
            let first = v6.segments()[0];
            v6 == Ipv6Addr::LOCALHOST || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

fn is_local_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || (a == 100 && (64..128).contains(&b))
}

fn bind_socket(port: u16, multicast: bool) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    socket.set_reuse_port(true)?;
    socket.set_broadcast(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())?;
    if multicast {
        socket.set_multicast_loop_v4(true)?;
        socket.set_multicast_ttl_v4(1)?;
        let mut joined = false;
        for ip in local_ipv4s() {
            joined |= socket.join_multicast_v4(&MULTICAST_ADDR, &ip).is_ok();
        }
        if !joined {
            // Fall back to the default interface; broadcast still works without it.
            let _ = socket.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED);
        }
    }
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    Ok(socket.into())
}

/// Non-loopback IPv4 addresses of this machine.
pub fn local_ipv4s() -> Vec<Ipv4Addr> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if !v4.is_loopback() => Some(v4),
            _ => None,
        })
        .collect()
}

fn send_all(socket: &UdpSocket, config: &DiscoveryConfig, kind: MessageKind) {
    let payload = DiscoveryMessage::new(kind, &config.me, config.transfer_port).encode();
    for seed in &config.seeds {
        let _ = socket.send_to(&payload, seed);
    }
    if !config.use_multicast {
        return;
    }
    let group = SocketAddrV4::new(MULTICAST_ADDR, DISCOVERY_PORT);
    // One copy per interface so multi-adapter PCs (VPN, Hyper-V, Wi-Fi + Ethernet) all work.
    let sock = socket2::SockRef::from(socket);
    for ip in local_ipv4s() {
        if sock.set_multicast_if_v4(&ip).is_ok() {
            let _ = socket.send_to(&payload, group);
        }
    }
    let _ = sock.set_multicast_if_v4(&Ipv4Addr::UNSPECIFIED);
    let _ = socket.send_to(&payload, group);
    let _ = socket.send_to(
        &payload,
        SocketAddrV4::new(Ipv4Addr::BROADCAST, DISCOVERY_PORT),
    );
}

fn receive_loop(inner: &Arc<Inner>, stop: &AtomicBool) {
    let (socket, config) = (&inner.socket, &inner.config);
    let mut buf = [0u8; 2048];
    while !stop.load(Ordering::Relaxed) {
        let (len, from) = match socket.recv_from(&mut buf) {
            Ok(v) => v,
            Err(_) => continue, // timeout or transient error; re-check stop flag
        };
        let Some(msg) = DiscoveryMessage::decode(&buf[..len]) else {
            continue;
        };
        if msg.id == config.me.id {
            continue;
        }
        if msg.kind == MessageKind::Callback {
            handle_callback(inner, msg, from);
            continue;
        }
        let mut map = inner.peers.lock().expect("peer map poisoned");
        match msg.kind {
            MessageKind::Callback => {}
            MessageKind::Bye => {
                map.remove(&msg.id);
            }
            MessageKind::Announce | MessageKind::Query => {
                if msg.port != 0 {
                    let peer = Peer {
                        device: msg.device(),
                        addr: SocketAddr::new(from.ip(), msg.port),
                    };
                    map.insert(
                        msg.id.clone(),
                        Seen {
                            peer,
                            udp: from,
                            last: Instant::now(),
                        },
                    );
                }
                drop(map);
                // A firewalled receiver accepts unicast replies to its own announces for a
                // few seconds only: answer pending callback requests right now (spec §6).
                for p in inner.pending.lock().expect("pending poisoned").values() {
                    if p.peer_id == msg.id {
                        let cb =
                            DiscoveryMessage::callback(&config.me, p.port, &p.token, &p.peer_id);
                        let _ = socket.send_to(&cb.encode(), from);
                    }
                }
                if msg.kind == MessageKind::Query && config.transfer_port != 0 {
                    let reply = DiscoveryMessage::new(
                        MessageKind::Announce,
                        &config.me,
                        config.transfer_port,
                    );
                    let _ = socket.send_to(&reply.encode(), from);
                }
            }
        }
    }
}

/// Receiver side of spec §6: validate, rate-limit, then run the hook on its own thread.
fn handle_callback(inner: &Arc<Inner>, msg: DiscoveryMessage, from: SocketAddr) {
    if inner.config.transfer_port == 0 || msg.to.as_deref() != Some(inner.config.me.id.as_str()) {
        return;
    }
    if !is_local_address(from.ip()) {
        return;
    }
    let Some(hook) = inner.hook.lock().expect("hook poisoned").clone() else {
        return;
    };
    let token = msg.token.clone().unwrap_or_default();
    let ip = from.ip();
    if !inner
        .limiter
        .lock()
        .expect("limiter poisoned")
        .admit(ip, &token, Instant::now())
    {
        return;
    }
    let request = CallbackRequest {
        sender: msg.device(),
        addr: SocketAddr::new(ip, msg.port),
        token,
    };
    let inner2 = inner.clone();
    let spawned = thread::Builder::new()
        .name("hoppy-callback".into())
        .spawn(move || {
            hook(request);
            inner2.limiter.lock().expect("limiter poisoned").release(ip);
        });
    if spawned.is_err() {
        inner.limiter.lock().expect("limiter poisoned").release(ip);
    }
}

fn sleep_unless_stopped(total: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn local_addresses_only() {
        for ok in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.20",
            "100.64.0.1",
            "100.127.255.254",
            "169.254.3.4",
            "::1",
            "fd00::1",
            "fc12::1",
            "fe80::1",
            "::ffff:192.168.0.2",
        ] {
            assert!(is_local_address(ip(ok)), "{ok}");
        }
        for bad in [
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "100.63.255.255",
            "192.169.0.1",
            "2001:db8::1",
            "::ffff:8.8.8.8",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
        ] {
            assert!(!is_local_address(ip(bad)), "{bad}");
        }
    }

    #[test]
    fn limiter_dedupes_and_caps() {
        let mut l = CallbackLimiter::default();
        let now = Instant::now();
        let a = ip("192.168.1.2");
        assert!(l.admit(a, "t1", now));
        assert!(!l.admit(a, "t2", now), "one outstanding per source IP");
        l.release(a);
        assert!(!l.admit(a, "t1", now), "duplicate token");
        assert!(l.admit(a, "t2", now));
        l.release(a);
        for i in 3..=MAX_CALLBACKS_PER_MINUTE {
            assert!(l.admit(ip(&format!("10.0.0.{i}")), &format!("t{i}"), now));
        }
        assert!(!l.admit(ip("10.0.1.1"), "fresh", now), "per-minute cap");
        let later = now + Duration::from_secs(61);
        assert!(l.admit(ip("10.0.1.1"), "fresh", later));
        assert!(
            !l.admit(ip("10.0.1.2"), "t1", later),
            "tokens remembered for 2 min"
        );
        assert!(l.admit(ip("10.0.1.2"), "t1", now + Duration::from_secs(181)));
    }
}
