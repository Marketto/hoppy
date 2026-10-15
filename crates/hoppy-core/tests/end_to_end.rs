//! Loopback end-to-end tests: discovery + transfer between two in-process nodes, protocol
//! v1 security (encryption, SAS, trust on first use, emoji verification) and v0 fallback.

use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use hoppy_core::protocol::{
    self, CallbackHello, Decision, DiscoveryMessage, FileEntry, MessageKind,
};
use hoppy_core::session::{
    Initiator, MAX_HANDSHAKE_MESSAGE, NoiseReader, NoiseWriter, Responder, read_frame, write_frame,
};
use hoppy_core::transfer::{receive_one, serve};
use hoppy_core::{
    DeviceInfo, DeviceKey, Discovery, DiscoveryConfig, Error, Local, Offer, Peer, PeerKey,
    Progress, ReceiveHandler, SendOutcome, SessionInfo, Sweep, TrustState, Warning,
    accept_callbacks, send_files, send_to_peer,
};

fn device(id: &str, name: &str) -> DeviceInfo {
    DeviceInfo {
        id: id.into(),
        name: name.into(),
        platform: "linux".into(),
    }
}

fn local(id: &str, name: &str) -> Local {
    Local::ephemeral(device(id, name))
}

struct Recorder {
    accept: bool,
    /// Pretend to be `hoppy receive --yes` (accepts without a person).
    auto: bool,
    finished: Mutex<Vec<Result<Vec<PathBuf>, String>>>,
    /// The session of every offer that reached `decide`.
    sessions: Mutex<Vec<SessionInfo>>,
    blocked: Mutex<Vec<String>>,
    warnings: Mutex<Vec<Warning>>,
}

impl Recorder {
    fn new(accept: bool) -> Self {
        Self {
            accept,
            auto: false,
            finished: Mutex::new(Vec::new()),
            sessions: Mutex::new(Vec::new()),
            blocked: Mutex::new(Vec::new()),
            warnings: Mutex::new(Vec::new()),
        }
    }
}

impl ReceiveHandler for Recorder {
    fn decide(&self, _offer: &Offer, _from: SocketAddr, session: &SessionInfo) -> bool {
        self.sessions.lock().unwrap().push(session.clone());
        self.accept
    }
    fn finished(&self, _from: SocketAddr, result: &Result<Vec<PathBuf>, Error>) {
        let r = result.as_ref().cloned().map_err(ToString::to_string);
        self.finished.lock().unwrap().push(r);
    }
    fn blocked(&self, _from: SocketAddr, _offer: &Offer, _session: &SessionInfo, reason: &str) {
        self.blocked.lock().unwrap().push(reason.into());
    }
    fn warning(&self, _from: SocketAddr, warning: &Warning) {
        self.warnings.lock().unwrap().push(warning.clone());
    }
    fn auto_accepts(&self) -> bool {
        self.auto
    }
}

/// A receiver on loopback with its own identity ("rx", "Receiver").
fn start_receiver(accept: bool) -> (SocketAddr, tempfile::TempDir, Arc<Recorder>) {
    let (addr, dir, rec, _) = start_receiver_as(accept, local("rx", "Receiver"));
    (addr, dir, rec)
}

fn start_receiver_as(
    accept: bool,
    me: Local,
) -> (SocketAddr, tempfile::TempDir, Arc<Recorder>, Local) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::new(accept));
    let handler: Arc<dyn ReceiveHandler> = recorder.clone();
    let out = dir.path().to_path_buf();
    let me2 = me.clone();
    thread::spawn(move || serve(listener, out, me2, handler));
    (addr, dir, recorder, me)
}

/// The peer as discovery would list it (v1, with its id).
fn peer_at(me: &Local, addr: SocketAddr) -> Peer {
    Peer {
        device: me.device.clone(),
        addr,
        crypto: Some(1),
    }
}

fn wait_finished(rec: &Recorder) -> Result<Vec<PathBuf>, String> {
    for _ in 0..100 {
        if let Some(r) = rec.finished.lock().unwrap().pop() {
            return r;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("receiver never finished");
}

#[test]
fn sends_multiple_files_and_verifies_content() {
    let (addr, out, rec) = start_receiver(true);
    let src = tempfile::tempdir().unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    fs::write(src.path().join("big.bin"), &big).unwrap();
    fs::write(src.path().join("empty.txt"), b"").unwrap();
    fs::write(src.path().join("ciao 🐰.txt"), "ciao").unwrap();
    let paths = ["big.bin", "empty.txt", "ciao 🐰.txt"]
        .map(|n| src.path().join(n))
        .to_vec();

    let mut last_done = 0;
    let mut session = None;
    let outcome = send_files(addr, &local("a", "A"), &paths, &mut |p| match p {
        Progress::Bytes { done, .. } => last_done = done,
        Progress::Session(s) => session = Some(s),
        _ => {}
    })
    .unwrap();
    assert!(session.unwrap().encrypted, "v1 ↔ v1 is always encrypted");

    assert_eq!(outcome, SendOutcome::Delivered);
    assert_eq!(last_done, big.len() as u64 + 4);
    let saved = wait_finished(&rec).unwrap();
    assert_eq!(saved.len(), 3);
    assert_eq!(fs::read(out.path().join("big.bin")).unwrap(), big);
    assert_eq!(fs::read(out.path().join("empty.txt")).unwrap(), b"");
    assert_eq!(
        fs::read_to_string(out.path().join("ciao 🐰.txt")).unwrap(),
        "ciao"
    );
    let leftovers: Vec<_> = fs::read_dir(out.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".hoppy-part")
        })
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn name_clash_gets_counter() {
    let (addr, out, rec) = start_receiver(true);
    fs::write(out.path().join("doc.txt"), "old").unwrap();
    let src = tempfile::tempdir().unwrap();
    fs::write(src.path().join("doc.txt"), "new").unwrap();
    send_files(
        addr,
        &local("a", "A"),
        &[src.path().join("doc.txt")],
        &mut |_| {},
    )
    .unwrap();
    wait_finished(&rec).unwrap();
    assert_eq!(
        fs::read_to_string(out.path().join("doc.txt")).unwrap(),
        "old"
    );
    assert_eq!(
        fs::read_to_string(out.path().join("doc (1).txt")).unwrap(),
        "new"
    );
}

#[test]
fn declined_offer_reports_reason() {
    let (addr, out, _rec) = start_receiver(false);
    let src = tempfile::tempdir().unwrap();
    fs::write(src.path().join("x.txt"), "x").unwrap();
    let outcome = send_files(
        addr,
        &local("a", "A"),
        &[src.path().join("x.txt")],
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(outcome, SendOutcome::Declined(Some("declined".into())));
    assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
}

#[test]
fn rejects_directories_on_send() {
    let src = tempfile::tempdir().unwrap();
    let err = send_files(
        "127.0.0.1:9".parse().unwrap(),
        &local("a", "A"),
        &[src.path().to_path_buf()],
        &mut |_| {},
    );
    assert!(matches!(err, Err(Error::Input(_))));
}

/// A malicious sender (plaintext v0 client): traversal name and a wrong hash.
#[test]
fn receiver_sanitizes_names_and_drops_corrupt_files() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().to_path_buf();
    let rec = Arc::new(Recorder::new(true));
    let rec2 = rec.clone();
    let server = thread::spawn(move || {
        let (stream, from) = listener.accept().unwrap();
        receive_one(stream, from, &dir, &local("rx", "R"), rec2.as_ref())
    });

    let mut s = std::net::TcpStream::connect(addr).unwrap();
    protocol::write_hello(&mut s).unwrap();
    let offer = Offer {
        sender: device("evil", "Evil"),
        files: vec![
            FileEntry {
                name: "../../escape.txt".into(),
                size: 2,
            },
            FileEntry {
                name: "bad.txt".into(),
                size: 3,
            },
        ],
    };
    protocol::write_json(&mut s, &offer).unwrap();
    let d: Decision = protocol::read_json(&mut s).unwrap();
    assert!(d.accept);
    s.write_all(b"ok").unwrap();
    s.write_all(&sha256(b"ok")).unwrap();
    s.write_all(b"bad").unwrap();
    s.write_all(&[0u8; 32]).unwrap();
    let result: protocol::TransferResult = protocol::read_json(&mut s).unwrap();
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);

    assert!(!result.ok);
    assert!(result.error.unwrap().contains("bad.txt"));
    assert!(server.join().unwrap().unwrap().is_err());
    assert_eq!(
        fs::read_to_string(out.path().join("escape.txt")).unwrap(),
        "ok"
    );
    assert!(!out.path().join("bad.txt").exists());
    assert!(!out.path().parent().unwrap().join("escape.txt").exists());
    // v0 → v1 receiver: accepted during the transition, flagged as not encrypted.
    assert_eq!(*rec.sessions.lock().unwrap(), [SessionInfo::plaintext()]);
}

#[test]
fn invalid_names_reject_the_offer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().to_path_buf();
    let rec = Arc::new(Recorder::new(true));
    let server = thread::spawn(move || {
        let (stream, from) = listener.accept().unwrap();
        receive_one(stream, from, &dir, &local("rx", "R"), rec.as_ref()).is_none()
    });
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    protocol::write_hello(&mut s).unwrap();
    let offer = Offer {
        sender: device("e", "E"),
        files: vec![FileEntry {
            name: "..".into(),
            size: 1,
        }],
    };
    protocol::write_json(&mut s, &offer).unwrap();
    let d: Decision = protocol::read_json(&mut s).unwrap();
    assert_eq!(d, Decision::reject("invalid_offer"));
    assert!(server.join().unwrap());
}

#[test]
fn discovery_finds_peer_over_unicast_seed() {
    let mut a_cfg = DiscoveryConfig::new(device("node-a", "Alpha"), 1111);
    a_cfg.bind_port = 0;
    a_cfg.use_multicast = false;
    let a = Discovery::start(a_cfg).unwrap();

    let mut b_cfg = DiscoveryConfig::new(device("node-b", "Beta"), 2222);
    b_cfg.bind_port = 0;
    b_cfg.use_multicast = false;
    b_cfg.seeds = vec![format!("127.0.0.1:{}", a.local_port()).parse().unwrap()];
    let b = Discovery::start(b_cfg).unwrap();

    let seen_by_b = b.wait_for_peers(1, Duration::from_secs(3));
    let seen_by_a = a.wait_for_peers(1, Duration::from_secs(3));
    assert_eq!(seen_by_b[0].device.name, "Alpha");
    assert_eq!(seen_by_b[0].addr.port(), 1111);
    assert_eq!(seen_by_a[0].device.name, "Beta");
    assert_eq!(seen_by_a[0].addr.port(), 2222);

    drop(b); // sends bye
    thread::sleep(Duration::from_millis(300));
    assert!(a.peers().is_empty());
}

/// Spec §2.1: a node that cannot use multicast (iOS without the entitlement) finds a
/// receiver with unicast queries only, the receiver lists it, and switching it to send-only
/// and back is seen at once (bye / announce to known peers).
#[test]
fn unicast_sweep_finds_and_is_found() {
    let mut a_cfg = DiscoveryConfig::new(device("node-a", "Alpha"), 1111);
    a_cfg.bind_port = 0;
    a_cfg.use_multicast = false;
    let a = Discovery::start(a_cfg).unwrap();

    let mut b_cfg = DiscoveryConfig::new(device("ipad", "iPad"), 2222);
    b_cfg.bind_port = 0;
    b_cfg.use_multicast = false;
    b_cfg.sweep = Sweep::Hosts(vec![SocketAddr::from(([127, 0, 0, 1], a.local_port()))]);
    let b = Discovery::start(b_cfg).unwrap();

    let seen_by_b = b.wait_for_peers(1, Duration::from_secs(3));
    assert_eq!(seen_by_b[0].device.name, "Alpha");
    let seen_by_a = a.wait_for_peers(1, Duration::from_secs(3));
    assert_eq!(seen_by_a[0].device.name, "iPad");
    assert_eq!(seen_by_a[0].addr.port(), 2222);

    b.set_transfer_port(0);
    assert_eq!(b.transfer_port(), 0);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !a.peers().is_empty() && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    assert!(a.peers().is_empty(), "bye reached the known peer");

    b.set_transfer_port(3333);
    let seen_by_a = a.wait_for_peers(1, Duration::from_secs(3));
    assert_eq!(seen_by_a[0].addr.port(), 3333);
    drop(b);
    thread::sleep(Duration::from_millis(300));
    assert!(a.peers().is_empty());
}

fn loopback_discovery(me: DeviceInfo, transfer_port: u16, seed: Option<u16>) -> Discovery {
    let mut cfg = DiscoveryConfig::new(me, transfer_port);
    cfg.bind_port = 0;
    cfg.use_multicast = false;
    cfg.seeds = seed
        .map(|p| vec![SocketAddr::from(([127, 0, 0, 1], p))])
        .unwrap_or_default();
    Discovery::start(cfg).unwrap()
}

/// A port nothing listens on, so a direct connect is refused (stands in for a firewall).
fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Spec §6: the receiver's TCP port is unreachable, so the sender asks it to connect back.
#[test]
fn callback_delivers_when_direct_connect_fails() {
    let (addr, out, rec, r_local) = start_receiver_as(true, local("rx", "Receiver"));
    let r_disc = loopback_discovery(r_local.device.clone(), addr.port(), None);
    let handler: Arc<dyn ReceiveHandler> = rec.clone();
    accept_callbacks(&r_disc, out.path().to_path_buf(), r_local.clone(), handler);

    let me = local("tx", "Sender");
    let s_disc = loopback_discovery(me.device.clone(), 0, Some(r_disc.local_port()));
    let mut peer = s_disc.wait_for_peers(1, Duration::from_secs(3)).remove(0);
    assert_eq!(peer.device.id, "rx");
    peer.addr.set_port(closed_port());

    let src = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 253) as u8).collect();
    fs::write(src.path().join("via callback.bin"), &data).unwrap();
    assert_eq!(peer.crypto, Some(1), "v1 announce");
    let mut sas = None;
    let outcome = send_to_peer(
        &peer,
        &me,
        Some(&s_disc),
        &[src.path().join("via callback.bin")],
        &mut |p| {
            if let Progress::Session(s) = p {
                sas = s.sas;
            }
        },
    )
    .unwrap();
    assert_eq!(outcome, SendOutcome::Delivered);
    assert_eq!(wait_finished(&rec).unwrap().len(), 1);
    assert_eq!(fs::read(out.path().join("via callback.bin")).unwrap(), data);
    // Encrypted over the callback connection too, same SAS on both ends.
    let seen = rec.sessions.lock().unwrap()[0].clone();
    assert!(seen.encrypted);
    assert_eq!(seen.sas, sas);
    assert_eq!(seen.peer.unwrap().fingerprint, me.fingerprint());
}

#[test]
fn direct_failure_without_discovery_is_unreachable() {
    let src = tempfile::tempdir().unwrap();
    fs::write(src.path().join("x.txt"), "x").unwrap();
    let peer = Peer {
        device: device("rx", "R"),
        addr: SocketAddr::from(([127, 0, 0, 1], closed_port())),
        crypto: Some(1),
    };
    let err = send_to_peer(
        &peer,
        &local("tx", "S"),
        None,
        &[src.path().join("x.txt")],
        &mut |_| {},
    );
    assert!(matches!(err, Err(Error::Unreachable(_))), "{err:?}");
}

/// Sender side: only a connection with the right token and receiver id is used. The fake
/// receiver is a v0 node (no `crypto` field), so the session falls back to plaintext.
#[test]
fn callback_sender_verifies_token_and_id() {
    let fake_r = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    fake_r
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let me = local("tx", "Sender");
    let s_disc = loopback_discovery(
        me.device.clone(),
        0,
        Some(fake_r.local_addr().unwrap().port()),
    );

    // Answer the sender's query like a (v0) receiver whose TCP port is closed.
    let mut buf = [0u8; 2048];
    let (_, s_udp) = fake_r.recv_from(&mut buf).unwrap();
    let announce = DiscoveryMessage {
        crypto: None,
        ..DiscoveryMessage::new(MessageKind::Announce, &device("rx", "R"), closed_port())
    };
    fake_r.send_to(&announce.encode(), s_udp).unwrap();
    let peer = s_disc.wait_for_peers(1, Duration::from_secs(3)).remove(0);

    let src = tempfile::tempdir().unwrap();
    fs::write(src.path().join("x.txt"), "x").unwrap();
    let path = src.path().join("x.txt");
    let sender = thread::spawn(move || {
        let r = send_to_peer(&peer, &me, Some(&s_disc), &[path], &mut |_| {});
        drop(s_disc);
        r
    });

    let callback = loop {
        let (n, from) = fake_r.recv_from(&mut buf).unwrap();
        let Some(m) = DiscoveryMessage::decode(&buf[..n]) else {
            continue;
        };
        if m.kind == MessageKind::Callback {
            assert_eq!(from, s_udp, "sent from the discovery socket");
            break m;
        }
    };
    assert_eq!(callback.to.as_deref(), Some("rx"));
    assert_eq!(callback.id, "tx");
    let token = callback.token.clone().unwrap();
    let target = SocketAddr::from(([127, 0, 0, 1], callback.port));

    for (id, tok) in [("rx", "0".repeat(32)), ("someone-else", token.clone())] {
        let mut s = std::net::TcpStream::connect(target).unwrap();
        let hello = CallbackHello {
            id: id.into(),
            token: tok,
            crypto: None,
        };
        protocol::write_callback_hello(&mut s, &hello).unwrap();
        let mut rest = Vec::new();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = s.read_to_end(&mut rest);
        assert!(
            rest.is_empty(),
            "sender must close a connection with a bad hello"
        );
    }

    let mut s = std::net::TcpStream::connect(target).unwrap();
    let hello = CallbackHello {
        id: "rx".into(),
        token,
        crypto: None,
    };
    protocol::write_callback_hello(&mut s, &hello).unwrap();
    assert_eq!(
        protocol::read_hello(&mut s).unwrap(),
        protocol::VERSION,
        "v0 peer"
    );
    let offer: Offer = protocol::read_json(&mut s).unwrap();
    assert_eq!(offer.files[0].name, "x.txt");
    protocol::write_json(&mut s, &Decision::reject("declined")).unwrap();
    let outcome = sender.join().unwrap().unwrap();
    assert_eq!(outcome, SendOutcome::Declined(Some("declined".into())));
}

/// A TCP connection from a specific loopback source IP (127.0.0.2 stands in for another
/// LAN host). `None` where the OS has no 127/8 beyond 127.0.0.1 (macOS by default).
fn connect_from(src: [u8; 4], to: SocketAddr) -> Option<std::net::TcpStream> {
    use socket2::{Domain, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
    s.bind(&SocketAddr::from((src, 0)).into()).ok()?;
    s.connect(&to.into()).ok()?;
    Some(s.into())
}

/// Spec §6 sender steps 2–3: while a callback is pending, only the receiver's known IP may
/// speak for its id (announces from elsewhere are ignored, connections from elsewhere are
/// closed unread), and the hello must arrive whole within 3 s and fit in 1 KiB.
#[test]
fn callback_sender_ignores_other_ips_and_slow_hellos() {
    use std::time::Instant;
    let fake_r = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    fake_r
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let me = local("tx", "Sender");
    let s_disc = loopback_discovery(
        me.device.clone(),
        0,
        Some(fake_r.local_addr().unwrap().port()),
    );
    let mut buf = [0u8; 2048];
    let (_, s_udp) = fake_r.recv_from(&mut buf).unwrap();
    let rx = device("rx", "R");
    let announce = DiscoveryMessage {
        crypto: None,
        ..DiscoveryMessage::new(MessageKind::Announce, &rx, closed_port())
    };
    fake_r.send_to(&announce.encode(), s_udp).unwrap();
    let peer = s_disc.wait_for_peers(1, Duration::from_secs(3)).remove(0);

    let src = tempfile::tempdir().unwrap();
    fs::write(src.path().join("x.txt"), "x").unwrap();
    let path = src.path().join("x.txt");
    let sender = thread::spawn(move || {
        let r = send_to_peer(&peer, &me, Some(&s_disc), &[path], &mut |_| {});
        drop(s_disc);
        r
    });
    let next_callback = |sock: &std::net::UdpSocket| {
        let mut buf = [0u8; 2048];
        loop {
            let (n, _) = sock.recv_from(&mut buf).ok()?;
            if let Some(m) = DiscoveryMessage::decode(&buf[..n])
                && m.kind == MessageKind::Callback
            {
                return Some(m);
            }
        }
    };
    let callback = next_callback(&fake_r).expect("callback datagram");
    let token = callback.token.clone().unwrap();
    let target = SocketAddr::from(([127, 0, 0, 1], callback.port));

    if let Ok(spoof) = std::net::UdpSocket::bind("127.0.0.2:0") {
        // Someone else claims the receiver's id: no token for them, re-sends stay put.
        spoof
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        for _ in 0..3 {
            spoof.send_to(&announce.encode(), s_udp).unwrap();
            assert!(next_callback(&spoof).is_none(), "token sent to a spoofer");
        }
        let again = next_callback(&fake_r).expect("re-send still reaches the real receiver");
        assert_eq!(again.token.as_deref(), Some(token.as_str()));
    } else {
        eprintln!("skipped spoofed-announce part: cannot bind 127.0.0.2");
    }

    if let Some(mut s) = connect_from([127, 0, 0, 2], target) {
        // Right token, wrong IP: closed before the hello is even read.
        let hello = CallbackHello {
            id: "rx".into(),
            token: token.clone(),
            crypto: None,
        };
        let _ = protocol::write_callback_hello(&mut s, &hello);
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut rest = Vec::new();
        let _ = s.read_to_end(&mut rest);
        assert!(
            rest.is_empty(),
            "connection from another IP must be dropped"
        );
    } else {
        eprintln!("skipped wrong-IP connection part: cannot bind 127.0.0.2");
    }

    // Oversized hello frame: closed without waiting for the body.
    let mut s = std::net::TcpStream::connect(target).unwrap();
    s.write_all(b"HOPC\0\0\0\x04\x01").unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let t0 = Instant::now();
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);
    assert!(
        rest.is_empty() && t0.elapsed() < Duration::from_secs(2),
        "oversized hello"
    );

    // Slow-loris: one byte every 400 ms never finishes a hello; cut off after ~3 s total.
    let mut s = std::net::TcpStream::connect(target).unwrap();
    let t0 = Instant::now();
    let slow = b"HOPC\0\0\0\0\x58{\"id\":\"rx\",\"token\":\"";
    let mut closed_after = None;
    for &b in slow.iter().cycle().take(40) {
        if s.write_all(&[b]).is_err() {
            closed_after = Some(t0.elapsed());
            break;
        }
        thread::sleep(Duration::from_millis(400));
    }
    let closed_after = closed_after.expect("slow hello must be cut off");
    assert!(closed_after < Duration::from_secs(6), "{closed_after:?}");

    let mut s = std::net::TcpStream::connect(target).unwrap();
    let hello = CallbackHello {
        id: "rx".into(),
        token,
        crypto: None,
    };
    protocol::write_callback_hello(&mut s, &hello).unwrap();
    protocol::read_hello(&mut s).unwrap();
    let _: Offer = protocol::read_json(&mut s).unwrap();
    protocol::write_json(&mut s, &Decision::reject("declined")).unwrap();
    let outcome = sender.join().unwrap().unwrap();
    assert_eq!(outcome, SendOutcome::Declined(Some("declined".into())));
}

/// Receiver side: ignores callbacks for other ids, connects back once per token.
#[test]
fn callback_receiver_filters_and_dedupes() {
    let out = tempfile::tempdir().unwrap();
    let rec = Arc::new(Recorder::new(false));
    let r_disc = loopback_discovery(device("rx", "R"), 4242, None);
    let handler: Arc<dyn ReceiveHandler> = rec.clone();
    accept_callbacks(&r_disc, out.path().to_path_buf(), local("rx", "R"), handler);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let r_addr = SocketAddr::from(([127, 0, 0, 1], r_disc.local_port()));
    let s_dev = device("tx", "S");
    let token = protocol::new_token();
    let accept_within = |ms: u64| {
        let end = std::time::Instant::now() + Duration::from_millis(ms);
        while std::time::Instant::now() < end {
            if let Ok((s, _)) = listener.accept() {
                return Some(s);
            }
            thread::sleep(Duration::from_millis(20));
        }
        None
    };

    let other = DiscoveryMessage::callback(&s_dev, port, &token, "not-rx");
    udp.send_to(&other.encode(), r_addr).unwrap();
    assert!(accept_within(500).is_none(), "callback for another id");

    // A connect-back that fails (nothing listens yet) does not burn the token: the
    // sender's next re-send with the same token is answered. Re-send like a real sender
    // until a deadline: on Windows a connect to a closed loopback port is not refused at
    // once but retried for ~2 s (up to CALLBACK_CONNECT_TIMEOUT), and re-sends arriving
    // while that attempt is outstanding are ignored by design.
    let refused = DiscoveryMessage::callback(&s_dev, closed_port(), &token, "rx");
    udp.send_to(&refused.encode(), r_addr).unwrap();
    thread::sleep(Duration::from_millis(200));

    let good = DiscoveryMessage::callback(&s_dev, port, &token, "rx");
    let end = std::time::Instant::now() + Duration::from_secs(15);
    let mut s = loop {
        udp.send_to(&good.encode(), r_addr).unwrap();
        if let Some(s) = accept_within(300) {
            break s;
        }
        assert!(
            std::time::Instant::now() < end,
            "receiver never connected back after the failed attempt"
        );
    };
    s.set_nonblocking(false).unwrap();
    let hello = protocol::read_callback_hello(&mut s).unwrap();
    assert_eq!(
        hello,
        CallbackHello {
            id: "rx".into(),
            token: token.clone(),
            crypto: Some(1),
        }
    );

    // The rest is plain spec §3 (a v0 sender), and the receiver still decides (declines).
    protocol::write_hello(&mut s).unwrap();
    let offer = Offer {
        sender: s_dev.clone(),
        files: vec![FileEntry {
            name: "x.txt".into(),
            size: 1,
        }],
    };
    protocol::write_json(&mut s, &offer).unwrap();
    let d: Decision = protocol::read_json(&mut s).unwrap();
    assert_eq!(d, Decision::reject("declined"));
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);
    assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);

    // The token connected back, so a later re-send (e.g. after our next announce) is ignored.
    thread::sleep(Duration::from_millis(300)); // let the receiver release this source IP
    udp.send_to(&good.encode(), r_addr).unwrap();
    assert!(accept_within(1000).is_none(), "duplicate token");
}

fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).into()
}

// ------------------------------------------------------------------ protocol v1 (spec §7)

fn small_file(name: &str, content: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join(name);
    fs::write(&path, content).unwrap();
    (src, path)
}

/// Sends `path` to `peer` and returns the outcome and the session the sender saw.
fn send_and_watch(
    peer: &Peer,
    me: &Local,
    path: &std::path::Path,
) -> (Result<SendOutcome, Error>, Option<SessionInfo>) {
    let mut session = None;
    let r = send_to_peer(peer, me, None, &[path.to_path_buf()], &mut |p| {
        if let Progress::Session(s) = p {
            session = Some(s);
        }
    });
    (r, session)
}

/// Both ends of an encrypted session see each other's real key and the same SAS; the first
/// accepted transfer pins the keys (New → Known), verifying turns them Verified.
#[test]
fn encrypted_session_same_sas_and_tofu() {
    let (addr, out, rec, r_local) = start_receiver_as(true, local("rx", "Receiver"));
    let me = local("tx", "Sender");
    let (_src, path) = small_file("a.txt", b"secret stuff");
    let peer = peer_at(&r_local, addr);

    let (outcome, seen_by_sender) = send_and_watch(&peer, &me, &path);
    assert_eq!(outcome.unwrap(), SendOutcome::Delivered);
    wait_finished(&rec).unwrap();
    let s = seen_by_sender.unwrap();
    let r = rec.sessions.lock().unwrap()[0].clone();
    assert!(s.encrypted && r.encrypted);
    assert_eq!(
        (s.trust, r.trust),
        (Some(TrustState::New), Some(TrustState::New))
    );
    assert_eq!(s.peer.as_ref().unwrap().fingerprint, r_local.fingerprint());
    assert_eq!(r.peer.as_ref().unwrap().fingerprint, me.fingerprint());
    assert_eq!(r.peer.as_ref().unwrap().id, "tx");
    let (ss, rs) = (s.sas.clone().unwrap(), r.sas.clone().unwrap());
    assert_eq!(ss, rs, "same code on both screens");
    assert_eq!(
        ss.text.chars().count(),
        4,
        "4 emoji, the only form of the code"
    );
    assert_eq!(fs::read(out.path().join("a.txt")).unwrap(), b"secret stuff");

    // TOFU on both sides after the accepted transfer.
    assert_eq!(
        me.trust.state("rx", &r_local.fingerprint()),
        TrustState::Known
    );
    assert_eq!(
        r_local.trust.state("tx", &me.fingerprint()),
        TrustState::Known
    );
    // The users compared the code and pressed "They match".
    me.trust.mark_verified(s.peer.as_ref().unwrap()).unwrap();
    r_local
        .trust
        .mark_verified(r.peer.as_ref().unwrap())
        .unwrap();
    let (outcome, s2) = send_and_watch(&peer, &me, &path);
    assert_eq!(outcome.unwrap(), SendOutcome::Delivered);
    wait_finished(&rec).unwrap();
    assert_eq!(s2.unwrap().trust, Some(TrustState::Verified));
    assert_eq!(
        rec.sessions.lock().unwrap()[1].trust,
        Some(TrustState::Verified)
    );
    assert_ne!(
        rec.sessions.lock().unwrap()[1].sas,
        Some(rs),
        "fresh SAS per session"
    );
}

/// A declined first contact is not remembered (TOFU pins only accepted transfers).
#[test]
fn declined_first_contact_is_not_pinned() {
    let (addr, _out, _rec, r_local) = start_receiver_as(false, local("rx", "Receiver"));
    let me = local("tx", "Sender");
    let (_src, path) = small_file("a.txt", b"x");
    let (outcome, _) = send_and_watch(&peer_at(&r_local, addr), &me, &path);
    assert_eq!(
        outcome.unwrap(),
        SendOutcome::Declined(Some("declined".into()))
    );
    assert_eq!(me.trust.get("rx"), None);
    assert_eq!(r_local.trust.get("tx"), None);
}

/// Man in the middle: terminates the sender's handshake with its own key (claiming to be
/// the receiver) and opens its own session to the real receiver (claiming to be the
/// sender), relaying the decrypted stream. Everything "works", but the two screens show
/// different codes.
fn start_mitm(target: SocketAddr, impersonate: DeviceInfo) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for client in listener.incoming() {
            let Ok(client) = client else { return };
            let impersonate = impersonate.clone();
            thread::spawn(move || {
                let _ = mitm_one(client, target, &impersonate);
            });
        }
    });
    addr
}

fn mitm_one(client: TcpStream, target: SocketAddr, as_receiver: &DeviceInfo) -> Result<(), Error> {
    let key = DeviceKey::generate();
    let mut c = &client;
    let mut hello = [0u8; 5];
    c.read_exact(&mut hello)?;
    let mut resp = Responder::new(&key)?;
    resp.read_msg1(&read_frame(&mut c, MAX_HANDSHAKE_MESSAGE)?.unwrap())?;
    write_frame(&mut c, &resp.write_msg2(as_receiver)?)?;
    let sender = resp.read_msg3(&read_frame(&mut c, MAX_HANDSHAKE_MESSAGE)?.unwrap())?;
    let to_client = resp.finish()?;

    let server = TcpStream::connect(target)?;
    let mut sv = &server;
    let mut init = Initiator::new(&key)?;
    protocol::write_hello_v1(&mut sv)?;
    write_frame(&mut sv, &init.write_msg1()?)?;
    init.read_msg2(&read_frame(&mut sv, MAX_HANDSHAKE_MESSAGE)?.unwrap())?;
    write_frame(&mut sv, &init.write_msg3(&sender.device)?)?;
    let to_server = init.finish()?;

    let relay = |from: TcpStream, from_c, to: TcpStream, to_c| {
        thread::spawn(move || {
            let mut r = NoiseReader::new(from.try_clone().unwrap(), from_c);
            let mut w = NoiseWriter::new(to.try_clone().unwrap(), to_c);
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 || w.write_all(&buf[..n]).and_then(|()| w.flush()).is_err() {
                    break;
                }
            }
            let _ = to.shutdown(Shutdown::Write);
        })
    };
    let a = relay(
        client.try_clone()?,
        to_client.clone(),
        server.try_clone()?,
        to_server.clone(),
    );
    let b = relay(server, to_server, client, to_client);
    let _ = (a.join(), b.join());
    Ok(())
}

#[test]
fn man_in_the_middle_shows_different_codes() {
    let (addr, out, rec, r_local) = start_receiver_as(true, local("rx", "Receiver"));
    let mitm = start_mitm(addr, r_local.device.clone());
    let me = local("tx", "Sender");
    let (_src, path) = small_file("a.txt", b"intercepted");
    let (outcome, s) = send_and_watch(&peer_at(&r_local, mitm), &me, &path);
    assert_eq!(outcome.unwrap(), SendOutcome::Delivered, "relayed");
    wait_finished(&rec).unwrap();
    assert_eq!(fs::read(out.path().join("a.txt")).unwrap(), b"intercepted");
    let s = s.unwrap();
    let r = rec.sessions.lock().unwrap()[0].clone();
    assert_ne!(s.sas, r.sas, "the users would see different codes");
    assert_ne!(s.peer.unwrap().fingerprint, r_local.fingerprint());
    assert_ne!(r.peer.unwrap().fingerprint, me.fingerprint());
}

/// After a verified (or just remembered) contact, a different key for the same id blocks
/// the send before the offer goes out.
#[test]
fn key_changed_blocks_the_sender() {
    let (addr, _out, rec, r_local) = start_receiver_as(true, local("rx", "Receiver"));
    let me = local("tx", "Sender");
    let (_src, path) = small_file("a.txt", b"x");
    // We remember another key for "rx" (e.g. the real device; now an impostor answers).
    me.trust
        .pin(&PeerKey::new(
            &r_local.device,
            DeviceKey::generate().fingerprint(),
        ))
        .unwrap();
    let (outcome, session) = send_and_watch(&peer_at(&r_local, addr), &me, &path);
    assert!(matches!(outcome, Err(Error::KeyChanged(_))), "{outcome:?}");
    assert!(session.is_none(), "nothing shown as a session");
    thread::sleep(Duration::from_millis(200));
    assert!(
        rec.sessions.lock().unwrap().is_empty(),
        "no offer reached the receiver"
    );

    // Through a man in the middle too, once the real receiver is remembered.
    let me = local("tx2", "Sender 2");
    me.trust
        .pin(&PeerKey::new(&r_local.device, r_local.fingerprint()))
        .unwrap();
    let mitm = start_mitm(addr, r_local.device.clone());
    let (outcome, _) = send_and_watch(&peer_at(&r_local, mitm), &me, &path);
    assert!(matches!(outcome, Err(Error::KeyChanged(_))), "{outcome:?}");

    // "Forget this device": the next contact is New again and goes through.
    assert!(me.trust.forget("rx").unwrap());
    let (outcome, s) = send_and_watch(&peer_at(&r_local, addr), &me, &path);
    assert_eq!(outcome.unwrap(), SendOutcome::Delivered);
    assert_eq!(s.unwrap().trust, Some(TrustState::New));
}

#[test]
fn key_changed_is_declined_by_the_receiver_with_a_warning() {
    let (addr, out, rec, r_local) = start_receiver_as(true, local("rx", "Receiver"));
    r_local
        .trust
        .pin(&PeerKey::new(
            &device("tx", "Sender"),
            DeviceKey::generate().fingerprint(),
        ))
        .unwrap();
    let me = local("tx", "Sender");
    let (_src, path) = small_file("a.txt", b"x");
    let (outcome, _) = send_and_watch(&peer_at(&r_local, addr), &me, &path);
    // The sender only hears "declined"; the receiver's user gets the specific warning.
    assert_eq!(
        outcome.unwrap(),
        SendOutcome::Declined(Some("declined".into()))
    );
    thread::sleep(Duration::from_millis(100));
    assert_eq!(*rec.blocked.lock().unwrap(), ["key_changed"]);
    assert!(rec.sessions.lock().unwrap().is_empty(), "never asked");
    assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
}

/// A v0 receiver (old app) closes the connection on `HOPY 0x01`; a sender that does not know
/// the peer (typed address) reconnects in plaintext and says so.
#[test]
fn v0_receiver_gets_plaintext_flagged() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut versions = Vec::new();
        for _ in 0..2 {
            let (mut s, _) = listener.accept().unwrap();
            let mut hello = [0u8; 5];
            s.read_exact(&mut hello).unwrap();
            versions.push(hello[4]);
            if hello[4] != 0 {
                continue; // v0 implementations reject other versions by closing
            }
            let offer: Offer = protocol::read_json(&mut s).unwrap();
            assert_eq!(offer.files[0].name, "old.txt");
            protocol::write_json(&mut s, &Decision::reject("declined")).unwrap();
        }
        versions
    });
    let (_src, path) = small_file("old.txt", b"x");
    let mut sessions = Vec::new();
    let me = local("tx", "S");
    let outcome = send_files(addr, &me, std::slice::from_ref(&path), &mut |p| {
        if let Progress::Session(s) = p {
            sessions.push(s);
        }
    })
    .unwrap();
    assert_eq!(outcome, SendOutcome::Declined(Some("declined".into())));
    assert_eq!(server.join().unwrap(), [1, 0], "tried v1 first, then v0");
    assert_eq!(sessions, [SessionInfo::plaintext()]);

    // A discovered v0 peer (no `crypto` in its announce) gets plaintext right away, but a
    // device whose key we hold never does (downgrade): the send fails instead.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for s in listener.incoming() {
            let mut s = s.unwrap();
            let mut hello = [0u8; 5];
            let _ = s.read_exact(&mut hello); // closes: pretends to be v0
        }
    });
    let pinned = Peer {
        device: device("rx", "R"),
        addr,
        crypto: Some(0),
    };
    me.trust
        .pin(&PeerKey::new(
            &pinned.device,
            DeviceKey::generate().fingerprint(),
        ))
        .unwrap();
    let (outcome, _) = send_and_watch(&pinned, &me, &path);
    assert!(matches!(outcome, Err(Error::Unencrypted(_))), "{outcome:?}");
}

/// Plaintext offers claiming the id of a device we hold a key for are refused.
#[test]
fn plaintext_from_a_known_device_is_refused() {
    let (addr, _out, rec, r_local) = start_receiver_as(true, local("rx", "Receiver"));
    r_local
        .trust
        .pin(&PeerKey::new(&device("tx", "S"), "SOMEKEY".into()))
        .unwrap();
    let mut s = TcpStream::connect(addr).unwrap();
    protocol::write_hello(&mut s).unwrap();
    let offer = Offer {
        sender: device("tx", "S"),
        files: vec![FileEntry {
            name: "x.txt".into(),
            size: 1,
        }],
    };
    protocol::write_json(&mut s, &offer).unwrap();
    let d: Decision = protocol::read_json(&mut s).unwrap();
    assert_eq!(
        d,
        Decision::reject("declined"),
        "the specific reason stays local"
    );
    thread::sleep(Duration::from_millis(100));
    assert_eq!(*rec.blocked.lock().unwrap(), ["encryption_required"]);
}

/// A byte flipped on the wire after the handshake: the receiver stops at the next message,
/// keeps nothing, and the sender gets an error.
#[test]
fn tampered_ciphertext_is_rejected() {
    let (addr, out, rec, r_local) = start_receiver_as(true, local("rx", "Receiver"));
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let paddr = proxy.local_addr().unwrap();
    thread::spawn(move || {
        let (client, _) = proxy.accept().unwrap();
        let server = TcpStream::connect(addr).unwrap();
        let (c2, s2) = (client.try_clone().unwrap(), server.try_clone().unwrap());
        thread::spawn(move || {
            let (mut from, mut to) = (s2, c2);
            let _ = std::io::copy(&mut from, &mut to);
            let _ = to.shutdown(Shutdown::Write);
        });
        let (mut from, mut to) = (client, server);
        let mut buf = [0u8; 8192];
        let mut seen = 0usize;
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 {
                break;
            }
            const FLIP_AT: usize = 150_000; // inside the file data
            if seen <= FLIP_AT && FLIP_AT < seen + n {
                buf[FLIP_AT - seen] ^= 0x40;
            }
            seen += n;
            if to.write_all(&buf[..n]).is_err() {
                break;
            }
        }
        let _ = to.shutdown(Shutdown::Write);
    });
    let data: Vec<u8> = (0..400_000u32).map(|i| (i % 249) as u8).collect();
    let (_src, path) = small_file("big.bin", &data);
    let (outcome, _) = send_and_watch(&peer_at(&r_local, paddr), &local("tx", "S"), &path);
    assert!(outcome.is_err(), "{outcome:?}");
    let err = wait_finished(&rec).unwrap_err();
    assert!(err.contains("authentication"), "{err}");
    assert_eq!(
        fs::read_dir(out.path()).unwrap().count(),
        0,
        "nothing published, partial file deleted"
    );
}

/// Verification is local only (spec §7.6): "Coincide" marks the other device verified on
/// this device and sends nothing. A first encrypted frame that is not an Offer (e.g. the
/// `{"verify":…}` request of earlier builds) is an invalid offer (spec §7.4): it is
/// refused, the user is not asked and nothing is marked verified.
#[test]
fn verification_is_local_and_non_offer_first_frame_is_invalid() {
    let (addr, _out, rec, desktop) = start_receiver_as(true, local("pc", "Desktop"));
    let phone = local("phone", "Phone");
    let peer = peer_at(&desktop, addr);
    let (_src, path) = small_file("a.txt", b"x");
    let (outcome, s) = send_and_watch(&peer, &phone, &path);
    assert_eq!(outcome.unwrap(), SendOutcome::Delivered);
    wait_finished(&rec).unwrap();
    // "Coincide" on the phone: verified there, nothing changes on the desktop.
    phone
        .trust
        .mark_verified(s.unwrap().peer.as_ref().unwrap())
        .unwrap();
    assert_eq!(
        phone.trust.state("pc", &desktop.fingerprint()),
        TrustState::Verified
    );
    assert_eq!(
        desktop.trust.state("phone", &phone.fingerprint()),
        TrustState::Known
    );

    // A raw v1 session whose first frame is the old verification request.
    let stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut s = &stream;
    let mut init = Initiator::new(&phone.key).unwrap();
    protocol::write_hello_v1(&mut s).unwrap();
    write_frame(&mut s, &init.write_msg1().unwrap()).unwrap();
    init.read_msg2(&read_frame(&mut s, MAX_HANDSHAKE_MESSAGE).unwrap().unwrap())
        .unwrap();
    write_frame(&mut s, &init.write_msg3(&phone.device).unwrap()).unwrap();
    let cipher = init.finish().unwrap();
    let mut w = NoiseWriter::new(stream.try_clone().unwrap(), cipher.clone());
    protocol::write_json(&mut w, &serde_json::json!({"verify": "0".repeat(32)})).unwrap();
    w.flush().unwrap();
    let mut r = NoiseReader::new(stream.try_clone().unwrap(), cipher);
    let d: Decision = protocol::read_json(&mut r).unwrap();
    assert_eq!(d, Decision::reject("invalid_offer"));
    assert_eq!(
        rec.sessions.lock().unwrap().len(),
        1,
        "the user was not asked"
    );
    assert_eq!(
        desktop.trust.state("phone", &phone.fingerprint()),
        TrustState::Known
    );
}

// ------------------------------------------------------------------ v1 hardening

/// Plays a man in the middle that grinds codes: completes handshake message 2 with the
/// receiver and walks away. Returns `Some(msg2)` or `None` if refused ("busy" frame).
fn abort_after_msg2(addr: SocketAddr) -> Option<Vec<u8>> {
    let key = DeviceKey::generate();
    let mut s = TcpStream::connect(addr).unwrap();
    let mut init = Initiator::new(&key).unwrap();
    protocol::write_hello_v1(&mut s).unwrap();
    write_frame(&mut s, &init.write_msg1().unwrap()).unwrap();
    let msg2 = read_frame(&mut s, MAX_HANDSHAKE_MESSAGE).unwrap().unwrap();
    (!msg2.is_empty()).then_some(msg2) // and close without message 3
}

/// Spec §7.3: handshakes abandoned after message 2 are capped per source IP; the receiver
/// warns its user and answers "busy" (never something a sender could take for a v0 node).
#[test]
fn aborted_handshakes_are_limited_and_reported() {
    let (addr, _out, rec, r_local) = start_receiver_as(true, local("rx", "Receiver"));
    for i in 0..hoppy_core::guard::MAX_ABORTS_PER_IP {
        assert!(abort_after_msg2(addr).is_some(), "attempt {i} served");
        thread::sleep(Duration::from_millis(30)); // let the receiver record the abort
    }
    thread::sleep(Duration::from_millis(100));
    assert!(
        abort_after_msg2(addr).is_none(),
        "refused with a busy frame"
    );
    let warned = rec.warnings.lock().unwrap().clone();
    assert!(
        matches!(warned.first(), Some(Warning::HandshakeAborts { recent }) if *recent >= 5),
        "{warned:?}"
    );
    assert_eq!(warned.len(), 1, "warnings are rate-limited");
    // A real sender from the same address now gets a clear "busy", not a plaintext fallback.
    let (_src, path) = small_file("a.txt", b"x");
    let err = send_files(
        addr,
        &local("tx", "S"),
        std::slice::from_ref(&path),
        &mut |_| {},
    );
    assert!(
        matches!(&err, Err(Error::Remote(m)) if m.starts_with("busy")),
        "{err:?}"
    );
    let err = send_and_watch(&peer_at(&r_local, addr), &local("tx", "S"), &path).0;
    assert!(
        matches!(&err, Err(Error::Remote(m)) if m.starts_with("busy")),
        "{err:?}"
    );
}

/// A fake receiver that completes the handshake, then misbehaves while the sender waits for
/// the Decision: `trickle` = one valid byte at a time (each would reset a per-read timeout),
/// else an empty transport message.
fn stalling_receiver(trickle: bool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        let key = DeviceKey::generate();
        let mut r = &s;
        let mut hello = [0u8; 5];
        r.read_exact(&mut hello).unwrap();
        let mut resp = Responder::new(&key).unwrap();
        resp.read_msg1(&read_frame(&mut r, MAX_HANDSHAKE_MESSAGE).unwrap().unwrap())
            .unwrap();
        write_frame(&mut r, &resp.write_msg2(&device("rx", "R")).unwrap()).unwrap();
        resp.read_msg3(&read_frame(&mut r, MAX_HANDSHAKE_MESSAGE).unwrap().unwrap())
            .unwrap();
        let cipher = resp.finish().unwrap();
        if trickle {
            let mut w = NoiseWriter::new(s.try_clone().unwrap(), cipher);
            for b in b"\0\0\0\x0f{\"accept\":true}".iter().cycle() {
                if w.write_all(&[*b]).and_then(|()| w.flush()).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(150));
            }
        } else {
            let empty = cipher.seal(0, b"").unwrap();
            let _ = write_frame(&mut r, &empty);
            thread::sleep(Duration::from_secs(5));
        }
    });
    addr
}

#[test]
fn sender_deadline_and_empty_messages() {
    let (_src, path) = small_file("a.txt", b"x");
    let me = local("tx", "S").with_session_deadline(Duration::from_secs(1));
    let t0 = std::time::Instant::now();
    let err = send_files(
        stalling_receiver(true),
        &me,
        std::slice::from_ref(&path),
        &mut |_| {},
    );
    assert!(err.is_err(), "{err:?}");
    assert!(
        t0.elapsed() < Duration::from_secs(4),
        "trickled bytes cannot extend the deadline: {:?}",
        t0.elapsed()
    );
    let t0 = std::time::Instant::now();
    let err = send_files(
        stalling_receiver(false),
        &local("tx", "S"),
        &[path],
        &mut |_| {},
    );
    assert!(
        matches!(&err, Err(Error::Io(e)) if e.to_string().contains("empty")),
        "{err:?}"
    );
    assert!(t0.elapsed() < Duration::from_secs(3), "rejected at once");
}

/// A new id using the name of a remembered device is flagged; an automatic accept
/// (`--yes`) does not remember the key; a typed address where a device encrypted before
/// never falls back to plaintext.
#[test]
fn name_collision_auto_accept_and_address_memory() {
    let r_local = local("rx", "Receiver");
    r_local
        .trust
        .pin(&PeerKey::new(&device("old-id", "Sender"), "OLDKEY".into()))
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let out = tempfile::tempdir().unwrap();
    let rec = Arc::new(Recorder {
        auto: true,
        ..Recorder::new(true)
    });
    let handler: Arc<dyn ReceiveHandler> = rec.clone();
    let (dir, r2) = (out.path().to_path_buf(), r_local.clone());
    thread::spawn(move || serve(listener, dir, r2, handler));

    let me = local("tx", "Sender");
    let (_src, path) = small_file("a.txt", b"x");
    let (outcome, _) = send_and_watch(&Peer::at(addr), &me, &path);
    assert_eq!(outcome.unwrap(), SendOutcome::Delivered);
    wait_finished(&rec).unwrap();
    assert!(rec.sessions.lock().unwrap()[0].name_matches_known);
    assert_eq!(r_local.trust.get("tx"), None, "auto-accept never pins");
    // The sender pinned the receiver together with the address it typed.
    let entry = me.trust.get("rx").unwrap();
    assert_eq!(entry.addr.as_deref(), Some(addr.to_string().as_str()));

    // Later an "old app" answers at that same address: refused instead of plaintext.
    let v0 = TcpListener::bind("127.0.0.1:0").unwrap();
    let v0_addr = v0.local_addr().unwrap();
    thread::spawn(move || {
        for s in v0.incoming() {
            let mut hello = [0u8; 5];
            let _ = s.unwrap().read_exact(&mut hello); // and close, like a v0 node
        }
    });
    me.trust
        .pin_at(
            &PeerKey::new(&device("rx", "R"), entry.fingerprint.clone()),
            Some(v0_addr),
        )
        .unwrap();
    let err = send_files(v0_addr, &me, &[path], &mut |_| {});
    assert!(matches!(err, Err(Error::Unencrypted(_))), "{err:?}");
}
