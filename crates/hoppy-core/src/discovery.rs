//! LAN discovery over UDP multicast + broadcast (spec §2), the unicast sweep for nodes that
//! cannot use multicast (spec §2.1, iOS without the multicast entitlement), plus the
//! `callback` datagram that lets a firewalled receiver connect back to the sender (spec §6).

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
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
/// Receiver-side cap on callback attempts started per rolling minute, per source IP.
pub const MAX_CALLBACKS_PER_MINUTE: usize = 10;
/// Receiver-side cap on callback attempts started per rolling minute, all sources together.
pub const MAX_CALLBACKS_PER_MINUTE_TOTAL: usize = 30;
/// Receiver remembers tokens that connected back this long to drop the sender's re-sends.
const TOKEN_MEMORY: Duration = Duration::from_secs(120);
const MAX_REMEMBERED_TOKENS: usize = 256;
/// A sweeping node queries every host of its local subnets this often (spec §2.1). Shorter
/// than `PEER_TTL`, so nodes that only hear us through the sweep keep listing us.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(10);
/// Gap between two sweep datagrams (≈ 0.5 s for a /24): no burst of ARP requests.
const SWEEP_PACING: Duration = Duration::from_millis(2);
/// `refresh()` sweeps at most this often.
const SWEEP_MIN_GAP: Duration = Duration::from_secs(3);
/// Upper bound of sweep targets over all interfaces.
pub const MAX_SWEEP_HOSTS: usize = 1024;

/// Unicast discovery for nodes that cannot send or hear multicast/broadcast (spec §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Sweep {
    /// Multicast/broadcast + seeds only (spec §2 rule 2).
    #[default]
    Off,
    /// Query every host of each private IPv4 subnet of this machine (at most its /24, see
    /// [`sweep_targets`]) on [`DISCOVERY_PORT`], and query known peers every interval.
    LocalSubnets,
    /// Like `LocalSubnets` with fixed targets (tests, loopback).
    Hosts(Vec<SocketAddr>),
}

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
    /// Unicast sweep (spec §2.1). Off by default.
    pub sweep: Sweep,
}

impl DiscoveryConfig {
    pub fn new(me: DeviceInfo, transfer_port: u16) -> Self {
        Self {
            me,
            transfer_port,
            bind_port: DISCOVERY_PORT,
            seeds: Vec::new(),
            use_multicast: true,
            sweep: Sweep::Off,
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
#[derive(Clone)]
pub struct CallbackRequest {
    /// The sender as it described itself in the datagram (display only, not authenticated).
    pub sender: DeviceInfo,
    /// UDP source IP of the datagram + the requested port. Never an IP from the JSON.
    pub addr: SocketAddr,
    pub token: String,
    connected: Arc<dyn Fn() + Send + Sync>,
}

impl CallbackRequest {
    /// Call once the TCP connect succeeded and the callback hello was written: only then
    /// is the token recorded as used, so a re-send after a failed connect can retry.
    pub fn mark_connected(&self) {
        (self.connected)();
    }
}

impl std::fmt::Debug for CallbackRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackRequest")
            .field("sender", &self.sender)
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

/// Receiver-side hook: connect to `addr`, send the callback hello, receive. Runs on its
/// own thread and may block for the whole transfer.
pub type CallbackHook = Arc<dyn Fn(CallbackRequest) + Send + Sync>;

/// Sender-side outstanding request (see [`Discovery::request_callback`]).
struct Pending {
    peer_id: String,
    /// The IP the peer was discovered at: only datagrams from it may carry its id (spec §6).
    peer_ip: IpAddr,
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
    /// Stop flag of the announce listener (see [`Discovery::request_callback`]), if running.
    announce_listener: Mutex<Option<Arc<AtomicBool>>>,
    /// Current TCP transfer port (0 = send-only); starts as `config.transfer_port`.
    transfer_port: AtomicU16,
    /// `refresh()` asked for a sweep; the tx thread runs it.
    sweep_requested: AtomicBool,
}

impl Inner {
    fn port(&self) -> u16 {
        self.transfer_port.load(Ordering::Relaxed)
    }

    fn sweeping(&self) -> bool {
        self.config.sweep != Sweep::Off
    }

    /// Discovery addresses of the live peers (where unicast to them goes).
    fn known_udp(&self) -> Vec<SocketAddr> {
        let mut map = self.peers.lock().expect("peer map poisoned");
        map.retain(|_, s| s.last.elapsed() < PEER_TTL);
        map.values().map(|s| s.udp).collect()
    }

    /// A node whose datagrams do not reach everyone by multicast (sweeping, or multicast
    /// off) also unicasts query/announce/bye to the peers it knows.
    fn unicast_known(&self) -> bool {
        self.sweeping() || !self.config.use_multicast
    }

    /// Seeds + multicast/broadcast (spec §2 rule 2), and the known peers when
    /// [`Self::unicast_known`].
    fn send_everywhere(&self, kind: MessageKind) {
        let payload = DiscoveryMessage::new(kind, &self.config.me, self.port()).encode();
        send_payload_everywhere(self, &payload);
    }

    /// One unicast `query` to every sweep target, paced; stops early when `stop` is set.
    fn sweep_once(&self, stop: &AtomicBool) {
        let targets = match &self.config.sweep {
            Sweep::Off => return,
            Sweep::Hosts(hosts) => hosts.clone(),
            Sweep::LocalSubnets => sweep_targets(&sweep_interfaces())
                .into_iter()
                .map(|ip| SocketAddr::from((ip, DISCOVERY_PORT)))
                .collect(),
        };
        let payload =
            DiscoveryMessage::new(MessageKind::Query, &self.config.me, self.port()).encode();
        for target in targets {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let _ = self.socket.send_to(&payload, target); // unreachable hosts just fail
            thread::sleep(SWEEP_PACING);
        }
    }
}

/// Running discovery. Dropping it sends `bye` (receiving nodes only) and stops the threads.
pub struct Discovery {
    inner: Arc<Inner>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Discovery {
    pub fn start(config: DiscoveryConfig) -> io::Result<Self> {
        let socket = bind_socket(config.bind_port, config.use_multicast)?;
        let config_port = config.transfer_port;
        let inner = Arc::new(Inner {
            socket,
            config,
            peers: Mutex::new(HashMap::new()),
            hook: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_ticket: AtomicU64::new(1),
            limiter: Mutex::new(CallbackLimiter::default()),
            announce_listener: Mutex::new(None),
            transfer_port: AtomicU16::new(config_port),
            sweep_requested: AtomicBool::new(false),
        });
        let stop = Arc::new(AtomicBool::new(false));

        let rx = {
            let (inner, stop) = (inner.clone(), stop.clone());
            thread::Builder::new()
                .name("hoppy-discovery-rx".into())
                .spawn(move || receive_loop(&inner, &inner.socket, &stop))?
        };
        let tx = {
            let (inner, stop) = (inner.clone(), stop.clone());
            thread::Builder::new()
                .name("hoppy-discovery-tx".into())
                .spawn(move || tx_loop(&inner, &stop))?
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

    /// Asks everyone to announce themselves now (e.g. when the picker opens). A sweeping
    /// node also sweeps its subnets again (at most every few seconds).
    pub fn refresh(&self) {
        self.inner.send_everywhere(MessageKind::Query);
        if self.inner.sweeping() {
            self.inner.sweep_requested.store(true, Ordering::Relaxed);
        }
    }

    /// The TCP transfer port this node announces; 0 = send-only (spec §2 rule 6).
    pub fn transfer_port(&self) -> u16 {
        self.inner.port()
    }

    /// Switches between receiving (`port != 0`) and send-only (`0`) without rebinding the
    /// UDP socket. Going send-only sends `bye` (others drop us at once); going back to
    /// receiving announces right away.
    pub fn set_transfer_port(&self, port: u16) {
        let old = self.inner.transfer_port.swap(port, Ordering::Relaxed);
        if old == port {
            return;
        }
        if port == 0 {
            // `bye` carries the old port only for display; receivers just drop the id.
            let bye = DiscoveryMessage::new(MessageKind::Bye, &self.inner.config.me, old).encode();
            send_payload_everywhere(&self.inner, &bye);
        } else {
            self.inner.send_everywhere(MessageKind::Announce);
        }
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
    ///
    /// A multicast node bound to another port than [`DISCOVERY_PORT`] (send-only nodes)
    /// cannot hear the peer's multicast announces, so while tickets live it also listens on
    /// a second socket bound to `DISCOVERY_PORT` with address reuse (see
    /// [`bind_announce_listener`]). Replies still leave from the main socket.
    pub fn request_callback(&self, peer: &Peer, port: u16, token: &str) -> CallbackTicket {
        let key = self.inner.next_ticket.fetch_add(1, Ordering::Relaxed);
        self.inner.pending.lock().expect("pending poisoned").insert(
            key,
            Pending {
                peer_id: peer.device.id.clone(),
                peer_ip: peer.addr.ip(),
                port,
                token: token.to_string(),
            },
        );
        // After the insert, so a concurrent last-ticket drop cannot stop our listener.
        if self.inner.config.use_multicast && self.local_port() != DISCOVERY_PORT {
            start_announce_listener(&self.inner);
        }
        let ticket = CallbackTicket {
            inner: self.inner.clone(),
            key,
            peer_id: peer.device.id.clone(),
            peer_ip: peer.addr.ip(),
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
        // A send-only node is never anyone's peer, and on desktops it shares its id with the
        // tray receiver: its `bye` would only make others drop the tray for a while.
        if self.inner.port() != 0 {
            self.inner.send_everywhere(MessageKind::Bye);
        }
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
    peer_ip: IpAddr,
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
            .filter(|udp| same_ip(udp.ip(), self.peer_ip))
            .unwrap_or(self.fallback);
        let _ = self.inner.socket.send_to(&self.payload, target);
    }
}

impl Drop for CallbackTicket {
    fn drop(&mut self) {
        let Ok(mut pending) = self.inner.pending.lock() else {
            return;
        };
        pending.remove(&self.key);
        if pending.is_empty()
            && let Ok(mut listener) = self.inner.announce_listener.lock()
            && let Some(stop) = listener.take()
        {
            stop.store(true, Ordering::Relaxed); // the thread exits within one read timeout
        }
    }
}

/// Starts the announce listener thread unless it already runs. Best effort: without it the
/// periodic [`CallbackTicket::resend`] still lands in the peer's response window sometimes.
fn start_announce_listener(inner: &Arc<Inner>) {
    let mut slot = inner
        .announce_listener
        .lock()
        .expect("listener slot poisoned");
    if slot.is_some() {
        return;
    }
    let Ok(socket) = bind_announce_listener() else {
        return;
    };
    let stop = Arc::new(AtomicBool::new(false));
    let (inner2, stop2) = (inner.clone(), stop.clone());
    if thread::Builder::new()
        .name("hoppy-discovery-listen".into())
        .spawn(move || receive_loop(&inner2, &socket, &stop2))
        .is_ok()
    {
        *slot = Some(stop);
    }
}

/// Listen-only socket on [`DISCOVERY_PORT`], shared (address reuse) with a receiver that may
/// run on the same machine (e.g. the Windows tray while `hoppyw send` runs).
///
/// Unicast datagrams to a shared UDP port reach only one of the sockets (Linux: hashed per
/// source with `SO_REUSEPORT`; Windows: unspecified), so this socket must not take the
/// receiver's unicast (query answers, `callback` datagrams). Where the OS allows it, it is
/// bound to the multicast group address, which filters unicast out; Windows cannot bind to
/// a multicast address, so there it binds to 0.0.0.0 and the window is kept short: it lives
/// only while a callback request is outstanding (at most `CALLBACK_WINDOW`).
fn bind_announce_listener() -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    socket.set_reuse_port(true)?;
    let local = if cfg!(windows) {
        Ipv4Addr::UNSPECIFIED
    } else {
        MULTICAST_ADDR
    };
    socket.bind(&SocketAddrV4::new(local, DISCOVERY_PORT).into())?;
    let mut joined = false;
    for ip in local_ipv4s() {
        joined |= socket.join_multicast_v4(&MULTICAST_ADDR, &ip).is_ok();
    }
    if !joined {
        socket.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED)?;
    }
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    Ok(socket.into())
}

/// Receiver-side admission control for callbacks (spec §6 "Limits").
#[derive(Default)]
pub(crate) struct CallbackLimiter {
    /// Tokens that connected back (see [`Self::mark_used`]).
    used: VecDeque<(String, Instant)>,
    outstanding: HashSet<IpAddr>,
    started: VecDeque<(IpAddr, Instant)>,
}

impl CallbackLimiter {
    /// `true` if a callback from `ip` with `token` may start now; then the caller must
    /// call [`Self::release`] when it ends, and [`Self::mark_used`] if it connected.
    pub(crate) fn admit(&mut self, ip: IpAddr, token: &str, now: Instant) -> bool {
        while self
            .used
            .front()
            .is_some_and(|(_, t)| now.duration_since(*t) > TOKEN_MEMORY)
        {
            self.used.pop_front();
        }
        while self
            .started
            .front()
            .is_some_and(|(_, t)| now.duration_since(*t) >= Duration::from_secs(60))
        {
            self.started.pop_front();
        }
        let from_ip = self.started.iter().filter(|(i, _)| *i == ip).count();
        if self.used.iter().any(|(t, _)| t == token)
            || self.outstanding.contains(&ip)
            || from_ip >= MAX_CALLBACKS_PER_MINUTE
            || self.started.len() >= MAX_CALLBACKS_PER_MINUTE_TOTAL
        {
            return false;
        }
        self.outstanding.insert(ip);
        self.started.push_back((ip, now));
        true
    }

    /// The connect-back with `token` succeeded: drop its re-sends from now on.
    pub(crate) fn mark_used(&mut self, token: &str, now: Instant) {
        if self.used.len() >= MAX_REMEMBERED_TOKENS {
            self.used.pop_front();
        }
        self.used.push_back((token.to_string(), now));
    }

    pub(crate) fn release(&mut self, ip: IpAddr) {
        self.outstanding.remove(&ip);
    }
}

/// IP equality that treats IPv4-mapped IPv6 (`::ffff:a.b.c.d`) as the IPv4 address.
pub(crate) fn same_ip(a: IpAddr, b: IpAddr) -> bool {
    a.to_canonical() == b.to_canonical()
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

fn send_all(socket: &UdpSocket, config: &DiscoveryConfig, kind: MessageKind, port: u16) {
    let payload = DiscoveryMessage::new(kind, &config.me, port).encode();
    send_payload(socket, config, &payload);
}

/// [`send_all`] for a prepared payload, plus the known peers (see [`Inner::unicast_known`]).
fn send_payload_everywhere(inner: &Inner, payload: &[u8]) {
    send_payload(&inner.socket, &inner.config, payload);
    if inner.unicast_known() {
        for udp in inner.known_udp() {
            if !inner.config.seeds.contains(&udp) {
                let _ = inner.socket.send_to(payload, udp);
            }
        }
    }
}

fn send_payload(socket: &UdpSocket, config: &DiscoveryConfig, payload: &[u8]) {
    for seed in &config.seeds {
        let _ = socket.send_to(payload, seed);
    }
    if !config.use_multicast {
        return;
    }
    let group = SocketAddrV4::new(MULTICAST_ADDR, DISCOVERY_PORT);
    // One copy per interface so multi-adapter PCs (VPN, Hyper-V, Wi-Fi + Ethernet) all work.
    let sock = socket2::SockRef::from(socket);
    for ip in local_ipv4s() {
        if sock.set_multicast_if_v4(&ip).is_ok() {
            let _ = socket.send_to(payload, group);
        }
    }
    let _ = sock.set_multicast_if_v4(&Ipv4Addr::UNSPECIFIED);
    let _ = socket.send_to(payload, group);
    let _ = socket.send_to(
        payload,
        SocketAddrV4::new(Ipv4Addr::BROADCAST, DISCOVERY_PORT),
    );
}

/// Reads datagrams from `rx` (the main socket or the announce listener) until `stop`.
/// Replies always leave from the main socket.
fn receive_loop(inner: &Arc<Inner>, rx: &UdpSocket, stop: &AtomicBool) {
    let (socket, config) = (&inner.socket, &inner.config);
    let mut buf = [0u8; 2048];
    while !stop.load(Ordering::Relaxed) {
        let (len, from) = match rx.recv_from(&mut buf) {
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
                // While we wait for this id to connect back, only its known IP may speak
                // for it: a claim from elsewhere would collect the token (spec §6 step 2).
                let claimed = inner
                    .pending
                    .lock()
                    .expect("pending poisoned")
                    .values()
                    .any(|p| p.peer_id == msg.id && !trusted_source(p.peer_ip, from.ip()));
                if claimed {
                    continue;
                }
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
                let port = inner.port();
                if msg.kind == MessageKind::Query && port != 0 {
                    let reply = DiscoveryMessage::new(MessageKind::Announce, &config.me, port);
                    let _ = socket.send_to(&reply.encode(), from);
                }
            }
        }
    }
}

/// Receiver side of spec §6: validate, rate-limit, then run the hook on its own thread.
/// `from` may speak for a peer discovered at `known` (spec §6 sender step 2).
fn trusted_source(known: IpAddr, from: IpAddr) -> bool {
    same_ip(known, from) && is_local_address(from)
}

fn handle_callback(inner: &Arc<Inner>, msg: DiscoveryMessage, from: SocketAddr) {
    if inner.port() == 0 || msg.to.as_deref() != Some(inner.config.me.id.as_str()) {
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
    let marker = (inner.clone(), token.clone());
    let request = CallbackRequest {
        sender: msg.device(),
        addr: SocketAddr::new(ip, msg.port),
        token,
        connected: Arc::new(move || {
            let (inner, token) = &marker;
            if let Ok(mut limiter) = inner.limiter.lock() {
                limiter.mark_used(token, Instant::now());
            }
        }),
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

/// Periodic sends: `query` at start, then every `ANNOUNCE_INTERVAL` an `announce` (or a
/// `query` while send-only, spec §2 rule 6). A sweeping node (spec §2.1) also queries its
/// known peers each interval (they answer and keep listing us) and sweeps its subnets.
fn tx_loop(inner: &Inner, stop: &AtomicBool) {
    inner.send_everywhere(MessageKind::Query);
    let mut next_periodic = Instant::now() + ANNOUNCE_INTERVAL;
    inner.send_everywhere(if inner.port() == 0 {
        MessageKind::Query
    } else {
        MessageKind::Announce
    });
    let mut last_sweep: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= next_periodic {
            if inner.port() == 0 {
                inner.send_everywhere(MessageKind::Query);
            } else {
                send_all(
                    &inner.socket,
                    &inner.config,
                    MessageKind::Announce,
                    inner.port(),
                );
                if inner.sweeping() {
                    // Known peers answer a query with an announce: both sides stay fresh.
                    let q =
                        DiscoveryMessage::new(MessageKind::Query, &inner.config.me, inner.port());
                    let payload = q.encode();
                    for udp in inner.known_udp() {
                        let _ = inner.socket.send_to(&payload, udp);
                    }
                }
            }
            next_periodic = now + ANNOUNCE_INTERVAL;
        }
        if inner.sweeping()
            && sweep_due(
                last_sweep,
                now,
                inner.sweep_requested.load(Ordering::Relaxed),
            )
        {
            inner.sweep_requested.store(false, Ordering::Relaxed);
            last_sweep = Some(now);
            inner.sweep_once(stop);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// First sweep at once, then every [`SWEEP_INTERVAL`], or sooner on request but never
/// within [`SWEEP_MIN_GAP`] of the previous one.
fn sweep_due(last: Option<Instant>, now: Instant, requested: bool) -> bool {
    match last {
        None => true,
        Some(t) => {
            let gap = now.duration_since(t);
            gap >= SWEEP_INTERVAL || (requested && gap >= SWEEP_MIN_GAP)
        }
    }
}

/// Up, broadcast-capable (not point-to-point: no cellular, no VPN tunnels), non-loopback
/// IPv4 interfaces as (address, netmask).
pub fn sweep_interfaces() -> Vec<(Ipv4Addr, Ipv4Addr)> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback() && !i.is_p2p())
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4) if v4.broadcast.is_some() => Some((v4.ip, v4.netmask)),
            _ => None,
        })
        .collect()
}

/// Hosts a sweep queries (spec §2.1): for each interface with a private RFC 1918 address,
/// every host of its subnet, but at most its /24 (a /16 office network is not swept);
/// never the interface's own address, network or broadcast address; /31 and /32 are
/// skipped. Deduplicated, at most [`MAX_SWEEP_HOSTS`].
pub fn sweep_targets(interfaces: &[(Ipv4Addr, Ipv4Addr)]) -> Vec<Ipv4Addr> {
    let own: HashSet<Ipv4Addr> = interfaces.iter().map(|(ip, _)| *ip).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (ip, mask) in interfaces {
        if !ip.is_private() {
            continue;
        }
        let mask = u32::from(*mask);
        if mask.leading_ones() + mask.trailing_zeros() != 32 || mask.leading_ones() > 30 {
            continue; // not a contiguous netmask, or /31 /32
        }
        let mask = mask | 0xFFFF_FF00; // at most the /24 around the address
        let net = u32::from(*ip) & mask;
        let broadcast = net | !mask;
        for host in net + 1..broadcast {
            let host = Ipv4Addr::from(host);
            if out.len() >= MAX_SWEEP_HOSTS {
                return out;
            }
            if !own.contains(&host) && seen.insert(host) {
                out.push(host);
            }
        }
    }
    out
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

    /// The send-side announce listener shares DISCOVERY_PORT with a receiver on the same
    /// machine and must leave unicast to it (Windows is checked by hand: no group bind).
    #[cfg(unix)]
    #[test]
    fn announce_listener_leaves_unicast_to_the_receiver() {
        let Ok(receiver) = bind_socket(DISCOVERY_PORT, false) else {
            eprintln!("skipped: UDP {DISCOVERY_PORT} not available here");
            return;
        };
        let Ok(listener) = bind_announce_listener() else {
            eprintln!("skipped: no multicast-capable interface here");
            return;
        };
        let probe = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        // Fail instead of hanging if this OS hands the unicast to the listener (macOS CI).
        receiver
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut buf = [0u8; 16];
        for i in 0..8u8 {
            probe
                .send_to(&[i], (Ipv4Addr::LOCALHOST, DISCOVERY_PORT))
                .unwrap();
            let (n, _) = receiver
                .recv_from(&mut buf)
                .expect("receiver got the unicast");
            assert_eq!(&buf[..n], &[i]);
        }
        listener
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        assert!(
            listener.recv_from(&mut buf).is_err(),
            "listener took unicast"
        );
        // ...while it does hear the group (when this host can loop multicast back).
        let mcast = bind_socket(0, true).unwrap();
        if mcast
            .send_to(b"m", SocketAddrV4::new(MULTICAST_ADDR, DISCOVERY_PORT))
            .is_ok()
        {
            let (n, _) = listener
                .recv_from(&mut buf)
                .expect("listener heard the group");
            assert_eq!(&buf[..n], b"m");
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
        assert!(l.admit(a, "t1", now), "failed connect: the token may retry");
        l.mark_used("t1", now);
        l.release(a);
        assert!(!l.admit(a, "t1", now), "token that connected back");
        for i in 3..=MAX_CALLBACKS_PER_MINUTE {
            assert!(l.admit(a, &format!("t{i}"), now), "attempt {i}");
            l.release(a);
        }
        assert!(!l.admit(a, "fresh", now), "per-IP cap");
        let mut n = MAX_CALLBACKS_PER_MINUTE;
        for i in 0..MAX_CALLBACKS_PER_MINUTE_TOTAL - n {
            assert!(l.admit(ip(&format!("10.0.0.{i}")), &format!("u{i}"), now));
            n += 1;
        }
        assert_eq!(n, MAX_CALLBACKS_PER_MINUTE_TOTAL);
        assert!(!l.admit(ip("10.0.1.1"), "other", now), "global cap");
        let later = now + Duration::from_secs(61);
        assert!(l.admit(ip("10.0.1.1"), "other", later));
        assert!(
            !l.admit(ip("10.0.1.2"), "t1", later),
            "used tokens remembered for 2 min"
        );
        assert!(l.admit(ip("10.0.1.2"), "t1", now + Duration::from_secs(181)));
    }

    fn v4(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn sweep_covers_the_local_slash_24_at_most() {
        let t = sweep_targets(&[(v4("192.168.1.20"), v4("255.255.255.0"))]);
        assert_eq!(t.len(), 253, "254 hosts minus our own address");
        assert_eq!(t[0], v4("192.168.1.1"));
        assert_eq!(*t.last().unwrap(), v4("192.168.1.254"));
        assert!(!t.contains(&v4("192.168.1.20")));
        // A /16 is narrowed to the /24 around us.
        let t = sweep_targets(&[(v4("10.7.42.9"), v4("255.255.0.0"))]);
        assert_eq!(t.len(), 253);
        assert!(t.iter().all(|h| h.octets()[..3] == [10, 7, 42]));
        // A small subnet (iPhone hotspot /28) stays small.
        let t = sweep_targets(&[(v4("172.20.10.2"), v4("255.255.255.240"))]);
        assert_eq!(
            t,
            (1..15)
                .filter(|&h| h != 2)
                .map(|h| Ipv4Addr::new(172, 20, 10, h))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn sweep_skips_public_tiny_and_odd_subnets() {
        assert!(sweep_targets(&[(v4("8.8.8.8"), v4("255.255.255.0"))]).is_empty());
        assert!(
            sweep_targets(&[(v4("100.64.1.2"), v4("255.255.255.0"))]).is_empty(),
            "CGNAT"
        );
        assert!(
            sweep_targets(&[(v4("169.254.1.2"), v4("255.255.0.0"))]).is_empty(),
            "link-local"
        );
        assert!(sweep_targets(&[(v4("192.168.1.2"), v4("255.255.255.254"))]).is_empty());
        assert!(sweep_targets(&[(v4("192.168.1.2"), v4("255.255.255.255"))]).is_empty());
        assert!(sweep_targets(&[(v4("192.168.1.2"), v4("255.0.255.0"))]).is_empty());
    }

    #[test]
    fn sweep_dedupes_and_caps() {
        let two = [
            (v4("192.168.1.20"), v4("255.255.255.0")),
            (v4("192.168.1.21"), v4("255.255.255.0")),
        ];
        let t = sweep_targets(&two);
        assert_eq!(t.len(), 252, "same /24 twice, both own addresses left out");
        let many: Vec<_> = (0..10)
            .map(|i| (Ipv4Addr::new(10, 0, i, 1), v4("255.255.255.0")))
            .collect();
        assert_eq!(sweep_targets(&many).len(), MAX_SWEEP_HOSTS);
    }

    #[test]
    fn sweep_schedule() {
        let t0 = Instant::now();
        assert!(sweep_due(None, t0, false));
        assert!(!sweep_due(Some(t0), t0 + Duration::from_secs(1), true));
        assert!(sweep_due(Some(t0), t0 + SWEEP_MIN_GAP, true));
        assert!(!sweep_due(Some(t0), t0 + SWEEP_MIN_GAP, false));
        assert!(sweep_due(Some(t0), t0 + SWEEP_INTERVAL, false));
    }

    #[test]
    fn trusted_source_needs_known_local_ip() {
        let known = ip("192.168.1.20");
        assert!(trusted_source(known, ip("192.168.1.20")));
        assert!(trusted_source(known, ip("::ffff:192.168.1.20")));
        assert!(!trusted_source(known, ip("192.168.1.21")));
        assert!(!trusted_source(ip("8.8.8.8"), ip("8.8.8.8")));
    }
}
