//! Loopback end-to-end tests: discovery + transfer between two in-process nodes.

use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use hoppy_core::protocol::{self, Decision, FileEntry};
use hoppy_core::transfer::{receive_one, serve};
use hoppy_core::{
    DeviceInfo, Discovery, DiscoveryConfig, Error, Offer, ReceiveHandler, SendOutcome, send_files,
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

fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).into()
}
