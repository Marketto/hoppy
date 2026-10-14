//! LAN discovery over UDP multicast + broadcast (spec §2).

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

use crate::protocol::{DISCOVERY_PORT, DeviceInfo, DiscoveryMessage, MULTICAST_ADDR, MessageKind};

pub const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
pub const PEER_TTL: Duration = Duration::from_secs(15);

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
    last: Instant,
}

/// Running discovery. Dropping it sends `bye` and stops the background threads.
pub struct Discovery {
    socket: Arc<UdpSocket>,
    config: DiscoveryConfig,
    peers: Arc<Mutex<HashMap<String, Seen>>>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Discovery {
    pub fn start(config: DiscoveryConfig) -> io::Result<Self> {
        let socket = Arc::new(bind_socket(config.bind_port, config.use_multicast)?);
        let peers = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let rx = {
            let (socket, peers, stop, config) =
                (socket.clone(), peers.clone(), stop.clone(), config.clone());
            thread::Builder::new()
                .name("hoppy-discovery-rx".into())
                .spawn(move || receive_loop(&socket, &config, &peers, &stop))?
        };
        let tx = {
            let (socket, stop, config) = (socket.clone(), stop.clone(), config.clone());
            thread::Builder::new()
                .name("hoppy-discovery-tx".into())
                .spawn(move || {
                    send_all(&socket, &config, MessageKind::Query);
                    // Send-only nodes (port 0) keep querying instead of announcing (spec §2 rule 6).
                    let periodic = if config.transfer_port == 0 {
                        MessageKind::Query
                    } else {
                        MessageKind::Announce
                    };
                    while !stop.load(Ordering::Relaxed) {
                        send_all(&socket, &config, periodic);
                        sleep_unless_stopped(ANNOUNCE_INTERVAL, &stop);
                    }
                })?
        };
        Ok(Self {
            socket,
            config,
            peers,
            stop,
            threads: vec![rx, tx],
        })
    }

    /// UDP port actually bound (useful when `bind_port` is 0).
    pub fn local_port(&self) -> u16 {
        self.socket.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// Asks everyone to announce themselves now (e.g. when the picker opens).
    pub fn refresh(&self) {
        send_all(&self.socket, &self.config, MessageKind::Query);
    }

    /// Live peers, sorted by name.
    pub fn peers(&self) -> Vec<Peer> {
        let mut map = self.peers.lock().expect("peer map poisoned");
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
        send_all(&self.socket, &self.config, MessageKind::Bye);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
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

fn receive_loop(
    socket: &UdpSocket,
    config: &DiscoveryConfig,
    peers: &Mutex<HashMap<String, Seen>>,
    stop: &AtomicBool,
) {
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
        let mut map = peers.lock().expect("peer map poisoned");
        match msg.kind {
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
                            last: Instant::now(),
                        },
                    );
                }
                drop(map);
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

fn sleep_unless_stopped(total: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(100));
    }
}
