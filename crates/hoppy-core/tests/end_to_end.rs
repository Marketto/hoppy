//! Loopback end-to-end tests: discovery + transfer between two in-process nodes.

use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use hoppy_core::protocol::{
    self, CallbackHello, Decision, DiscoveryMessage, FileEntry, MessageKind,
};
use hoppy_core::transfer::{receive_one, serve};
use hoppy_core::{
    DeviceInfo, Discovery, DiscoveryConfig, Error, Offer, Peer, ReceiveHandler, SendOutcome, Sweep,
    accept_callbacks, send_files, send_to_peer,
};

fn device(id: &str, name: &str) -> DeviceInfo {
    DeviceInfo {
        id: id.into(),
        name: name.into(),
        platform: "linux".into(),
    }
}

struct Recorder {
    accept: bool,
    finished: Mutex<Vec<Result<Vec<PathBuf>, String>>>,
}

impl ReceiveHandler for Recorder {
    fn decide(&self, _offer: &Offer, _from: SocketAddr) -> bool {
        self.accept
    }
    fn finished(&self, _from: SocketAddr, result: &Result<Vec<PathBuf>, Error>) {
        let r = result.as_ref().cloned().map_err(ToString::to_string);
        self.finished.lock().unwrap().push(r);
    }
}

fn start_receiver(accept: bool) -> (SocketAddr, tempfile::TempDir, Arc<Recorder>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder {
        accept,
        finished: Mutex::new(Vec::new()),
    });
    let handler: Arc<dyn ReceiveHandler> = recorder.clone();
    let out = dir.path().to_path_buf();
    thread::spawn(move || serve(listener, out, handler));
    (addr, dir, recorder)
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
    let outcome = send_files(addr, &device("a", "A"), &paths, &mut |p| {
        if let hoppy_core::Progress::Bytes { done, .. } = p {
            last_done = done;
        }
    })
    .unwrap();

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
        &device("a", "A"),
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
        &device("a", "A"),
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
        &device("a", "A"),
        &[src.path().to_path_buf()],
        &mut |_| {},
    );
    assert!(matches!(err, Err(Error::Input(_))));
}

/// A malicious sender: traversal name and a wrong hash.
#[test]
fn receiver_sanitizes_names_and_drops_corrupt_files() {
    use std::io::{Read, Write};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().to_path_buf();
    let rec = Arc::new(Recorder {
        accept: true,
        finished: Mutex::new(Vec::new()),
    });
    let rec2 = rec.clone();
    let server = thread::spawn(move || {
        let (stream, from) = listener.accept().unwrap();
        receive_one(stream, from, &dir, rec2.as_ref())
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
}

#[test]
fn invalid_names_reject_the_offer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().to_path_buf();
    let rec = Arc::new(Recorder {
        accept: true,
        finished: Mutex::new(Vec::new()),
    });
    let server = thread::spawn(move || {
        let (stream, from) = listener.accept().unwrap();
        receive_one(stream, from, &dir, rec.as_ref()).is_none()
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
    let (addr, out, rec) = start_receiver(true);
    let r_disc = loopback_discovery(device("rx", "Receiver"), addr.port(), None);
    let handler: Arc<dyn ReceiveHandler> = rec.clone();
    accept_callbacks(&r_disc, out.path().to_path_buf(), handler);

    let me = device("tx", "Sender");
    let s_disc = loopback_discovery(me.clone(), 0, Some(r_disc.local_port()));
    let mut peer = s_disc.wait_for_peers(1, Duration::from_secs(3)).remove(0);
    assert_eq!(peer.device.id, "rx");
    peer.addr.set_port(closed_port());

    let src = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 253) as u8).collect();
    fs::write(src.path().join("via callback.bin"), &data).unwrap();
    let outcome = send_to_peer(
        &peer,
        &me,
        Some(&s_disc),
        &[src.path().join("via callback.bin")],
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(outcome, SendOutcome::Delivered);
    assert_eq!(wait_finished(&rec).unwrap().len(), 1);
    assert_eq!(fs::read(out.path().join("via callback.bin")).unwrap(), data);
}

#[test]
fn direct_failure_without_discovery_is_unreachable() {
    let src = tempfile::tempdir().unwrap();
    fs::write(src.path().join("x.txt"), "x").unwrap();
    let peer = Peer {
        device: device("rx", "R"),
        addr: SocketAddr::from(([127, 0, 0, 1], closed_port())),
    };
    let err = send_to_peer(
        &peer,
        &device("tx", "S"),
        None,
        &[src.path().join("x.txt")],
        &mut |_| {},
    );
    assert!(matches!(err, Err(Error::Unreachable(_))), "{err:?}");
}

/// Sender side: only a connection with the right token and receiver id is used.
#[test]
fn callback_sender_verifies_token_and_id() {
    use std::io::Read;
    let fake_r = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    fake_r
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let me = device("tx", "Sender");
    let s_disc = loopback_discovery(me.clone(), 0, Some(fake_r.local_addr().unwrap().port()));

    // Answer the sender's query like a receiver whose TCP port is closed.
    let mut buf = [0u8; 2048];
    let (_, s_udp) = fake_r.recv_from(&mut buf).unwrap();
    let announce = DiscoveryMessage::new(MessageKind::Announce, &device("rx", "R"), closed_port());
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
    };
    protocol::write_callback_hello(&mut s, &hello).unwrap();
    protocol::read_hello(&mut s).unwrap();
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
    use std::io::{Read, Write};
    use std::time::Instant;
    let fake_r = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    fake_r
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let me = device("tx", "Sender");
    let s_disc = loopback_discovery(me.clone(), 0, Some(fake_r.local_addr().unwrap().port()));
    let mut buf = [0u8; 2048];
    let (_, s_udp) = fake_r.recv_from(&mut buf).unwrap();
    let rx = device("rx", "R");
    let announce = DiscoveryMessage::new(MessageKind::Announce, &rx, closed_port());
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
    use std::io::Read;
    let out = tempfile::tempdir().unwrap();
    let rec = Arc::new(Recorder {
        accept: false,
        finished: Mutex::new(Vec::new()),
    });
    let r_disc = loopback_discovery(device("rx", "R"), 4242, None);
    let handler: Arc<dyn ReceiveHandler> = rec.clone();
    accept_callbacks(&r_disc, out.path().to_path_buf(), handler);

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
            token: token.clone()
        }
    );

    // The rest is plain spec §3, and the receiver still decides (here: declines).
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
