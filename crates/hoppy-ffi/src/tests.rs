//! FFI tests through the C entry points (run on Linux; the Swift twin runs on the iOS
//! simulator in CI: apps/ios/Tests/HoppyCoreTests.swift).

use super::*;
use std::fs;
use std::sync::Mutex;

/// Events of one node, collected by [`collect`] (the `ctx` pointer).
#[derive(Default)]
struct Events(Mutex<Vec<Value>>);

extern "C" fn collect(ctx: *mut c_void, event: *const c_char) {
    // SAFETY: ctx is a live `Events` for the node's lifetime; event is a C string.
    let (events, text) = unsafe { (&*(ctx as *const Events), CStr::from_ptr(event)) };
    let value: Value = serde_json::from_str(text.to_str().unwrap()).unwrap();
    events.0.lock().unwrap().push(value);
}

impl Events {
    fn wait(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        self.wait_after(0, what, pred)
    }

    /// Like `wait`, ignoring the first `skip` events.
    fn wait_after(&self, skip: usize, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(v) = self.0.lock().unwrap().iter().skip(skip).find(|v| pred(v)) {
                return v.clone();
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

fn start(config: Value, events: &Events) -> *mut Node {
    let config = CString::new(config.to_string()).unwrap();
    let mut err: *mut c_char = ptr::null_mut();
    // SAFETY: valid strings; events outlives the node in every test.
    let node = unsafe {
        hoppy_node_start(
            config.as_ptr(),
            Some(collect),
            events as *const Events as *mut c_void,
            &mut err,
        )
    };
    if node.is_null() {
        // SAFETY: set by hoppy_node_start on failure.
        let msg = unsafe { CStr::from_ptr(err) }
            .to_string_lossy()
            .into_owned();
        unsafe { hoppy_string_free(err) };
        panic!("start failed: {msg}");
    }
    node
}

fn take(s: *mut c_char) -> Value {
    assert!(!s.is_null());
    // SAFETY: returned by the library.
    let v = serde_json::from_str(unsafe { CStr::from_ptr(s) }.to_str().unwrap()).unwrap();
    unsafe { hoppy_string_free(s) };
    v
}

fn send(node: *mut Node, peer: &str, paths: &[PathBuf]) -> u64 {
    let peer = CString::new(peer).unwrap();
    let paths = CString::new(serde_json::to_string(paths).unwrap()).unwrap();
    // SAFETY: live node, valid strings.
    unsafe { hoppy_node_send(node, peer.as_ptr(), paths.as_ptr()) }
}

/// Two in-process nodes on loopback: the "iPad" (send-only, finds the receiver with the
/// unicast sweep) delivers a file, the receiver asks (event), accepts; then a decline.
#[test]
fn sweep_send_accept_and_decline() {
    let rx_dir = tempfile::tempdir().unwrap();
    let (rx_events, tx_events) = (Events::default(), Events::default());
    let rx = start(
        json!({"id": "rx-1", "name": "Receiver\u{202E}gpj", "platform": "linux",
               "receive_dir": rx_dir.path(), "udp_port": 0, "tcp_port": 0, "receive": true,
               "sweep": "off"}),
        &rx_events,
    );
    // SAFETY: live node.
    let info = take(unsafe { hoppy_node_info(rx) });
    let rx_udp = info["udp_port"].as_u64().unwrap();
    assert_ne!(info["tcp_port"].as_u64().unwrap(), 0, "receiving");
    rx_events.wait("receiving", |v| v["type"] == "receiving" && v["on"] == true);

    let tx = start(
        json!({"id": "ipad-1", "name": "iPad", "udp_port": 0,
               "sweep": [format!("127.0.0.1:{rx_udp}")]}),
        &tx_events,
    );
    let peers = tx_events.wait("peer list", |v| {
        v["type"] == "peers" && v["peers"].as_array().is_some_and(|p| !p.is_empty())
    });
    assert_eq!(peers["peers"][0]["id"], "rx-1");
    assert_eq!(
        peers["peers"][0]["name"], "Receivergpj",
        "bidi override stripped"
    );
    // SAFETY: live node.
    let listed = take(unsafe { hoppy_node_peers(tx) });
    assert_eq!(listed[0]["addr"], "127.0.0.1");

    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("foto mare.jpg");
    fs::write(&file, b"hello hoppy").unwrap();

    // 1. Accepted.
    let id = send(tx, "rx-1", std::slice::from_ref(&file));
    assert_ne!(id, 0);
    let offer = rx_events.wait("offer", |v| v["type"] == "offer");
    assert_eq!(offer["from"]["name"], "iPad");
    assert_eq!(offer["from"]["platform"], "ios");
    assert_eq!(offer["files"][0]["name"], "foto mare.jpg");
    assert_eq!(offer["count"], 1);
    assert_eq!(offer["total"], 11);
    let offer_id = offer["offer"].as_u64().unwrap();
    // SAFETY: live node.
    assert!(unsafe { hoppy_node_decide(rx, offer_id, true) });
    assert!(
        !unsafe { hoppy_node_decide(rx, offer_id, true) },
        "answered once"
    );
    let got = rx_events.wait("received", |v| v["type"] == "received");
    assert_eq!(got["ok"], true, "{got}");
    assert_eq!(got["from"], "iPad");
    let saved = PathBuf::from(got["files"][0].as_str().unwrap());
    assert_eq!(saved, rx_dir.path().join("foto mare.jpg"));
    assert_eq!(fs::read(&saved).unwrap(), b"hello hoppy");
    let done = tx_events.wait("send_done", |v| {
        v["type"] == "send_done" && v["transfer"] == id
    });
    assert_eq!(done["outcome"], "delivered", "{done}");
    tx_events.wait("send_progress", |v| {
        v["type"] == "send_progress" && v["done"] == 11 && v["total"] == 11
    });

    // 2. Declined.
    let id = send(tx, "rx-1", &[file]);
    let offer = rx_events.wait("second offer", |v| {
        v["type"] == "offer" && v["offer"].as_u64() != Some(offer_id)
    });
    // SAFETY: live node.
    assert!(unsafe { hoppy_node_decide(rx, offer["offer"].as_u64().unwrap(), false) });
    let done = tx_events.wait("declined", |v| {
        v["type"] == "send_done" && v["transfer"] == id
    });
    assert_eq!(done["outcome"], "declined");
    assert_eq!(
        fs::read_dir(rx_dir.path()).unwrap().count(),
        1,
        "nothing new saved"
    );

    // 3. Unknown device.
    let id = send(tx, "nobody", &[src.path().join("foto mare.jpg")]);
    let done = tx_events.wait("unknown", |v| {
        v["type"] == "send_done" && v["transfer"] == id
    });
    assert_eq!(done["outcome"], "unreachable");

    // SAFETY: live nodes, not used afterwards.
    unsafe {
        hoppy_node_stop(tx);
        hoppy_node_stop(rx);
    }
    let (a, b) = (rx_events.count(), tx_events.count());
    thread::sleep(Duration::from_millis(700));
    assert_eq!(
        (rx_events.count(), tx_events.count()),
        (a, b),
        "no events after stop"
    );
}

/// No answer within the timeout = declined (invariant 3), and the UI hears `offer_closed`.
#[test]
fn unanswered_offer_is_declined() {
    let rx_dir = tempfile::tempdir().unwrap();
    let (rx_events, tx_events) = (Events::default(), Events::default());
    let rx = start(
        json!({"id": "rx-2", "name": "R", "receive_dir": rx_dir.path(), "udp_port": 0,
               "tcp_port": 0, "receive": true, "sweep": "off", "decision_timeout_secs": 1}),
        &rx_events,
    );
    let src_dir = tempfile::tempdir().unwrap();
    let rx_udp = take(unsafe { hoppy_node_info(rx) })["udp_port"]
        .as_u64()
        .unwrap();
    let tx = start(
        // Also a receiver, so the receiver learns it from its queries and can say bye.
        json!({"id": "tx-2", "name": "T", "udp_port": 0, "tcp_port": 0, "sweep": "off",
               "receive_dir": src_dir.path(), "receive": true,
               "seeds": [format!("127.0.0.1:{rx_udp}")]}),
        &tx_events,
    );
    tx_events.wait("peer", |v| {
        v["type"] == "peers" && v["peers"].as_array().is_some_and(|p| !p.is_empty())
    });
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    fs::write(&file, b"x").unwrap();
    let id = send(tx, "rx-2", &[file]);
    let offer = rx_events.wait("offer", |v| v["type"] == "offer");
    rx_events.wait("closed", |v| {
        v["type"] == "offer_closed" && v["offer"] == offer["offer"]
    });
    let done = tx_events.wait("declined", |v| {
        v["type"] == "send_done" && v["transfer"] == id
    });
    assert_eq!(done["outcome"], "declined");
    assert!(!unsafe { hoppy_node_decide(rx, offer["offer"].as_u64().unwrap(), true) });
    assert_eq!(fs::read_dir(rx_dir.path()).unwrap().count(), 0);

    // Receiving off: the peer drops us (bye) and the port is closed.
    let mark = tx_events.count();
    assert_eq!(unsafe { hoppy_node_set_receiving(rx, false) }, 0);
    tx_events.wait_after(mark, "bye", |v| {
        v["type"] == "peers" && v["peers"].as_array().is_some_and(|p| p.is_empty())
    });
    assert_eq!(take(unsafe { hoppy_node_info(rx) })["tcp_port"], 0);
    unsafe {
        hoppy_node_stop(tx);
        hoppy_node_stop(rx);
    }
}

#[test]
fn bad_input_is_rejected_not_crashed() {
    let events = Events::default();
    let long_id = format!(r#"{{"id":"{}","name":"x"}}"#, "a".repeat(65));
    for bad in [
        "{",
        r#"{"name":"x"}"#,
        r#"{"id":"","name":"x"}"#,
        r#"{"id":"a","name":"x","sweep":3}"#,
        long_id.as_str(),
        r#"{"id":"a","name":"x","platform":"a-very-long-platform"}"#,
        r#"{"id":"a","name":"x","platform":""}"#,
    ] {
        let config = CString::new(bad).unwrap();
        let mut err: *mut c_char = ptr::null_mut();
        let node = unsafe {
            hoppy_node_start(
                config.as_ptr(),
                Some(collect),
                &events as *const _ as *mut c_void,
                &mut err,
            )
        };
        assert!(node.is_null(), "{bad}");
        assert!(!err.is_null());
        unsafe { hoppy_string_free(err) };
    }
    let config = CString::new(r#"{"id":"a","name":"x"}"#).unwrap();
    assert!(
        unsafe { hoppy_node_start(config.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) }
            .is_null()
    );

    let node = start(
        json!({"id": "n", "name": "N", "udp_port": 0, "sweep": "off"}),
        &events,
    );
    assert_eq!(
        unsafe { hoppy_node_set_receiving(node, true) },
        -1,
        "no receive_dir"
    );
    let peer = CString::new("p").unwrap();
    for paths in ["[]", "nope", "{}"] {
        let p = CString::new(paths).unwrap();
        assert_eq!(
            unsafe { hoppy_node_send(node, peer.as_ptr(), p.as_ptr()) },
            0,
            "{paths}"
        );
    }
    assert_eq!(
        unsafe { hoppy_node_send(node, ptr::null(), ptr::null()) },
        0
    );
    unsafe {
        hoppy_node_stop(node);
        hoppy_node_stop(ptr::null_mut());
        hoppy_string_free(ptr::null_mut());
    }
    let v = unsafe { CStr::from_ptr(hoppy_version()) }.to_str().unwrap();
    assert_eq!(v, env!("CARGO_PKG_VERSION"));
}

#[test]
fn config_parsing() {
    assert_eq!(parse_sweep(None).unwrap(), Sweep::LocalSubnets);
    assert_eq!(parse_sweep(Some(&json!("off"))).unwrap(), Sweep::Off);
    assert_eq!(
        parse_sweep(Some(&json!(["10.0.0.2", "10.0.0.3:9"]))).unwrap(),
        Sweep::Hosts(vec![
            "10.0.0.2:47470".parse().unwrap(),
            "10.0.0.3:9".parse().unwrap()
        ])
    );
    assert!(parse_sweep(Some(&json!(["nope"]))).is_err());
    assert_eq!(
        parse_addr(" 192.168.1.4 ", 1),
        Some("192.168.1.4:1".parse().unwrap())
    );
    assert_eq!(parse_addr("host", 1), None);
}

/// include/hoppy.h declares exactly the exported functions.
#[test]
fn header_matches_exports() {
    let names = |text: &str, prefix: &str| -> Vec<String> {
        let mut v: Vec<String> = text
            .match_indices(prefix)
            .filter_map(|(i, _)| {
                let rest = &text[i + prefix.len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                rest[name.len()..]
                    .starts_with('(')
                    .then(|| format!("hoppy_{name}"))
            })
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let header = include_str!("../include/hoppy.h");
    let source = include_str!("lib.rs");
    let exported = names(source, "extern \"C\" fn hoppy_");
    assert_eq!(
        names(header, " hoppy_")
            .into_iter()
            .chain(names(header, "*hoppy_"))
            .collect::<std::collections::BTreeSet<_>>(),
        exported
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    );
    assert_eq!(exported.len(), 16, "{exported:?}");
}

/// The Swift simulator test's flow (apps/ios/Tests/HoppyCoreTests.swift): the "iPad" sweeps
/// the receiver (no seeds) and also receives; the receiver (sweep off, no multicast) turns
/// receiving off and must tell the iPad `bye` by unicast.
#[test]
fn receiver_off_says_bye_to_sweeping_ipad() {
    let (rx_dir, tx_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (rx_events, tx_events) = (Events::default(), Events::default());
    let rx = start(
        json!({"id": "rx-3", "name": "Receiver", "platform": "linux", "receive_dir": rx_dir.path(),
               "udp_port": 0, "tcp_port": 0, "receive": true, "sweep": "off"}),
        &rx_events,
    );
    let rx_udp = take(unsafe { hoppy_node_info(rx) })["udp_port"]
        .as_u64()
        .unwrap();
    let tx = start(
        json!({"id": "tx-3", "name": "iPad", "udp_port": 0, "tcp_port": 0,
               "receive_dir": tx_dir.path(), "receive": true,
               "sweep": [format!("127.0.0.1:{rx_udp}")]}),
        &tx_events,
    );
    tx_events.wait("peer", |v| {
        v["type"] == "peers" && v["peers"].as_array().is_some_and(|p| !p.is_empty())
    });
    // Same steps as the Swift test before the bye: one accepted and one declined send.
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    fs::write(&file, b"hello hoppy").unwrap();
    for accept in [true, false] {
        let seen = rx_events.count();
        let id = send(tx, "rx-3", std::slice::from_ref(&file));
        let offer = rx_events.wait_after(seen, "offer", |v| v["type"] == "offer");
        assert!(unsafe { hoppy_node_decide(rx, offer["offer"].as_u64().unwrap(), accept) });
        tx_events.wait("done", |v| v["type"] == "send_done" && v["transfer"] == id);
    }
    let mark = tx_events.count();
    assert_eq!(unsafe { hoppy_node_set_receiving(rx, false) }, 0);
    tx_events.wait_after(mark, "bye", |v| {
        v["type"] == "peers" && v["peers"].as_array().is_some_and(|p| p.is_empty())
    });
    unsafe {
        hoppy_node_stop(tx);
        hoppy_node_stop(rx);
    }
}

fn c_string(s: *mut c_char) -> String {
    assert!(!s.is_null());
    // SAFETY: returned by the library.
    let v = unsafe { CStr::from_ptr(s) }.to_str().unwrap().to_owned();
    unsafe { hoppy_string_free(s) };
    v
}

/// Protocol v1 through the C API: offer and send_session carry the same code, "They match"
/// verifies, a QR scan verifies both sides, forget works, and the key persists in key_file.
#[test]
fn security_events_trust_and_qr() {
    let data = tempfile::tempdir().unwrap();
    let rx_dir = tempfile::tempdir().unwrap();
    let (rx_events, tx_events) = (Events::default(), Events::default());
    let rx_config = json!({"id": "rx-9", "name": "Desk", "platform": "linux",
        "receive_dir": rx_dir.path(), "udp_port": 0, "tcp_port": 0, "receive": true,
        "sweep": "off", "key_file": data.path().join("rx.key"),
        "trust_file": data.path().join("rx-trust.json")});
    let rx = start(rx_config.clone(), &rx_events);
    let info = take(unsafe { hoppy_node_info(rx) });
    let rx_fp = info["fingerprint"].as_str().unwrap().to_owned();
    let tx = start(
        json!({"id": "tx-9", "name": "Phone", "udp_port": 0,
               "sweep": [format!("127.0.0.1:{}", info["udp_port"])],
               "key_file": data.path().join("tx.key"),
               "trust_file": data.path().join("tx-trust.json")}),
        &tx_events,
    );
    let peers = tx_events.wait("peer", |v| {
        v["type"] == "peers" && v["peers"].as_array().is_some_and(|p| !p.is_empty())
    });
    assert_eq!(peers["peers"][0]["encrypted"], true);
    assert_eq!(peers["peers"][0]["trust"], "new");

    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    fs::write(&file, b"hi").unwrap();
    let id = send(tx, "rx-9", std::slice::from_ref(&file));
    let offer = rx_events.wait("offer", |v| v["type"] == "offer");
    let sec = &offer["security"];
    assert_eq!(sec["encrypted"], true);
    assert_eq!(sec["trust"], "new");
    assert_eq!(sec["peer"]["id"], "tx-9");
    assert_eq!(sec["sas"]["emoji"].as_array().unwrap().len(), 4);
    let session = tx_events.wait("send_session", |v| {
        v["type"] == "send_session" && v["transfer"] == id
    });
    assert_eq!(
        session["security"]["sas"], sec["sas"],
        "same code on both screens"
    );
    assert_eq!(session["security"]["peer"]["fingerprint"], rx_fp.as_str());
    assert!(unsafe { hoppy_node_decide(rx, offer["offer"].as_u64().unwrap(), true) });
    tx_events.wait("delivered", |v| {
        v["type"] == "send_done" && v["outcome"] == "delivered"
    });

    // "They match" on the receiver.
    let peer = CString::new(sec["peer"].to_string()).unwrap();
    assert!(unsafe { hoppy_node_trust_verify(rx, peer.as_ptr()) });
    let bad = CString::new(r#"{"id":"tx-9"}"#).unwrap();
    assert!(!unsafe { hoppy_node_trust_verify(rx, bad.as_ptr()) });
    let list = take(unsafe { hoppy_node_trust_list(rx) });
    assert_eq!(list["peers"][0]["id"], "tx-9");
    assert_eq!(list["peers"][0]["verified"], true);
    assert_eq!(list["me"]["fingerprint"], rx_fp.as_str());

    // The phone scans the desk's QR code: both sides verified.
    let payload = c_string(unsafe { hoppy_node_qr_begin(rx) });
    assert!(payload.starts_with("hoppy:verify:1:rx-9:"));
    let p = CString::new(payload).unwrap();
    let scan = unsafe { hoppy_node_qr_scan(tx, p.as_ptr(), ptr::null()) };
    assert_ne!(scan, 0);
    let done = tx_events.wait("qr_done", |v| v["type"] == "qr_done" && v["scan"] == scan);
    assert_eq!(done["ok"], true, "{done}");
    let verified = rx_events.wait("verified", |v| v["type"] == "verified");
    assert_eq!(verified["peer"]["id"], "tx-9");
    let list = take(unsafe { hoppy_node_trust_list(tx) });
    assert_eq!(list["peers"][0]["verified"], true);
    let junk = CString::new("not a code").unwrap();
    assert_eq!(
        unsafe { hoppy_node_qr_scan(tx, junk.as_ptr(), ptr::null()) },
        0
    );
    unsafe { hoppy_node_qr_end(rx) };

    // Forget, then the key file survives a restart.
    let id_c = CString::new("rx-9").unwrap();
    assert!(unsafe { hoppy_node_trust_forget(tx, id_c.as_ptr()) });
    assert!(!unsafe { hoppy_node_trust_forget(tx, id_c.as_ptr()) });
    unsafe {
        hoppy_node_stop(tx);
        hoppy_node_stop(rx);
    }
    let again = Events::default();
    let rx = start(rx_config, &again);
    assert_eq!(
        take(unsafe { hoppy_node_info(rx) })["fingerprint"],
        rx_fp.as_str()
    );
    unsafe { hoppy_node_stop(rx) };
}
