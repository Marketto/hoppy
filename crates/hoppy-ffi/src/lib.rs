//! C ABI over `hoppy-core` for the iPhone/iPad app (`apps/ios`, ADR 0009).
//!
//! One opaque [`Node`] per process: discovery (unicast sweep, spec §2.1 — iOS apps without
//! the multicast entitlement can neither send nor hear multicast/broadcast), an optional
//! TCP receiver, sends, and the receiver's accept question. Everything crosses the boundary
//! as UTF-8 JSON strings, so the Swift side is a thin `Codable` wrapper
//! (`apps/ios/Shared/HoppyCore.swift`) and the header (`include/hoppy.h`) stays tiny.
//!
//! Events reach Swift through one C callback, called on Rust threads (never re-entrantly
//! from a `hoppy_*` call). The callback must return quickly and must not call
//! `hoppy_node_stop` synchronously: Swift hops to the main queue first. After
//! `hoppy_node_stop` returns, the callback is never called again, so `ctx` may be freed.
//!
//! Event JSON (`type` field):
//! - `peers`: `{"peers":[{"id","name","platform","addr","encrypted","trust"}]}` whenever the
//!   list changes. `encrypted`: the device speaks protocol v1; `trust`: `"new"` (never
//!   seen), `"known"` or `"verified"` from the trust store (by id; the key itself is checked
//!   in each session).
//! - `receiving`: `{"on":bool,"port":u16}` after `hoppy_node_set_receiving`.
//! - `offer`: `{"offer":u64,"from":{"name","platform"},"files":[{"name","size"}],"count","total",
//!   "security":SECURITY}` — answer with `hoppy_node_decide` within `decision_timeout_secs`
//!   (default 110 s, 1–115) or it is declined (invariant 3: never auto-accept). Names are
//!   already display-safe.
//! - `offer_closed`: `{"offer":u64}` — timed out or the node stopped.
//! - `blocked`: `{"from":{"id","name","platform"},"reason":"key_changed"|"encryption_required"}`
//!   — an offer declined without asking: warn the user (spec §7.5).
//! - `verified`: `{"peer":{"id","name","platform","fingerprint"}}` — a device scanned our QR
//!   code (spec §7.7): show ✓, close the QR code.
//! - `handshake_aborts`: `{"from":"<ip>","recent":n}` — repeated interrupted encrypted
//!   handshakes: someone may be trying to guess codes (spec §7.3); warn, rate-limited.
//! - `trust_not_saved`: `{"error"}` — the trust store could not be written.
//! - `send_warning`: `{"transfer","kind":"trust_not_saved","error"}` (sender side).
//! - `receive_progress`: `{"done","total"}` (throttled).
//! - `received`: `{"ok":true,"from","files":[path…]}` or `{"ok":false,"from","error"}`.
//! - `send_session`: `{"transfer","security":SECURITY}` — before the offer goes out: show the
//!   code while waiting for the receiver.
//! - `send_progress`: `{"transfer","done","total"}` (throttled); `send_fallback`: `{"transfer"}`.
//! - `send_done`: `{"transfer","outcome":"delivered"|"declined"|"unreachable"|"key_changed"|
//!   "unencrypted"|"error","message"?}`.
//! - `qr_done`: `{"scan","ok","local","remote","error"?}` after `hoppy_node_qr_scan`: `local`
//!   = this device marked the other verified, `remote` = the other device confirmed too.
//!
//! SECURITY (spec §7): `{"encrypted":false}` for an old (v0) peer — show "non cifrato" —
//! else `{"encrypted":true,"trust":"new"|"known"|"verified","name_matches_known":bool,
//! "peer":PEER,"sas":{"value","emoji":[4 indices],"text","digits","it":[4 names],
//! "en":[4 names]}}` (`name_matches_known`: a New device using the name of a known one —
//! warn, it may be an impostor), with
//! PEER = `{"id","name","platform","fingerprint"}` (pass it back to
//! `hoppy_node_trust_verify` after "They match", or to `hoppy_node_qr_scan` as the live peer).
//! Keys: `key_file` / `trust_file` in the config (the app's private container); without them
//! the node uses a throw-away key and remembers nothing (tests only).

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use hoppy_core::protocol::{DISCOVERY_PORT, TRANSFER_PORT};
use hoppy_core::sanitize::{display_text, sanitize_file_name};
use hoppy_core::transfer::receive_one;
use hoppy_core::{
    DeviceInfo, DeviceKey, Discovery, DiscoveryConfig, Error, Local, Offer, Peer, PeerKey,
    Progress, ReceiveHandler, SendOutcome, SessionInfo, Sweep, Trust, TrustState, TrustStore,
    Warning, accept_callbacks, confirm_qr, send_to_peer,
};
use serde::Deserialize;
use serde_json::{Value, json};

/// `void (*)(void *ctx, const char *event_json)`; the string is valid during the call only.
pub type EventCallback = extern "C" fn(ctx: *mut c_void, event_json: *const c_char);

const PEER_POLL: Duration = Duration::from_millis(300);
const PROGRESS_EVERY: Duration = Duration::from_millis(150);
/// Files listed in an `offer` event (the count and total are always complete).
const OFFER_FILES_SHOWN: usize = 50;
const NAME_MAX: usize = 64;
const FILE_NAME_MAX: usize = 120;
/// Own identity limits (bytes): ids are UUIDs, platforms short words.
const ID_MAX_BYTES: usize = 64;
const PLATFORM_MAX_BYTES: usize = 16;
/// The sender waits 120 s for the decision (core): answer (or decline) before that.
const DECISION_TIMEOUT_MAX: u64 = 115;

#[derive(Debug, Deserialize)]
struct Config {
    id: String,
    name: String,
    #[serde(default = "default_platform")]
    platform: String,
    /// Where received files are published (the app's Documents folder).
    #[serde(default)]
    receive_dir: Option<PathBuf>,
    #[serde(default = "default_udp")]
    udp_port: u16,
    #[serde(default = "default_tcp")]
    tcp_port: u16,
    /// Start as a receiver (needs `receive_dir`).
    #[serde(default)]
    receive: bool,
    /// iOS without the multicast entitlement: false.
    #[serde(default)]
    multicast: bool,
    /// `"subnets"` (default), `"off"`, or a list of `"ip:port"` (tests).
    #[serde(default)]
    sweep: Option<Value>,
    /// Unicast targets: `"ip"` (discovery port) or `"ip:port"`; invalid entries are skipped.
    #[serde(default)]
    seeds: Vec<String>,
    #[serde(default = "default_decision_timeout")]
    decision_timeout_secs: u64,
    /// Device key file (spec §7.5), created on first use, owner-only. Absent: throw-away key.
    #[serde(default)]
    key_file: Option<PathBuf>,
    /// Trust store JSON file. Absent: in memory only.
    #[serde(default)]
    trust_file: Option<PathBuf>,
}

fn default_platform() -> String {
    "ios".into()
}
fn default_udp() -> u16 {
    DISCOVERY_PORT
}
fn default_tcp() -> u16 {
    TRANSFER_PORT
}
fn default_decision_timeout() -> u64 {
    110
}

fn parse_addr(s: &str, default_port: u16) -> Option<SocketAddr> {
    let s = s.trim();
    s.parse::<SocketAddr>().ok().or_else(|| {
        s.parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, default_port))
    })
}

fn parse_sweep(v: Option<&Value>) -> Result<Sweep, String> {
    match v {
        None => Ok(Sweep::LocalSubnets),
        Some(Value::String(s)) if s == "subnets" => Ok(Sweep::LocalSubnets),
        Some(Value::String(s)) if s == "off" => Ok(Sweep::Off),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| {
                i.as_str()
                    .and_then(|s| parse_addr(s, DISCOVERY_PORT))
                    .ok_or_else(|| format!("invalid sweep target {i}"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Sweep::Hosts),
        Some(other) => Err(format!("invalid sweep {other}")),
    }
}

/// Where events go. Cleared by `hoppy_node_stop`; emitting holds the lock, so once
/// cleared no callback is running or will run.
struct Emitter(Mutex<Option<(EventCallback, usize)>>);

impl Emitter {
    fn emit(&self, event: Value) {
        let guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((cb, ctx)) = *guard
            && let Ok(text) = CString::new(event.to_string())
        {
            cb(ctx as *mut c_void, text.as_ptr());
        }
    }

    fn clear(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// The one offer being asked (a second one meanwhile is declined as busy).
#[derive(Default)]
struct Asking {
    current: Option<(u64, Option<bool>)>,
    closed: bool,
}

struct Shared {
    events: Emitter,
    asking: Mutex<Asking>,
    answered: Condvar,
    next_offer: AtomicU64,
    decision_timeout: Duration,
    /// Display name of the sender of each accepted connection (for `received`).
    senders: Mutex<HashMap<SocketAddr, String>>,
    last_progress: Mutex<HashMap<SocketAddr, Instant>>,
}

impl Shared {
    fn answer(&self, offer: u64, accept: bool) -> bool {
        let mut a = self.asking.lock().unwrap_or_else(|e| e.into_inner());
        match &mut a.current {
            Some((id, answer)) if *id == offer && answer.is_none() => {
                *answer = Some(accept);
                self.answered.notify_all();
                true
            }
            _ => false,
        }
    }

    fn close(&self) {
        self.asking.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.answered.notify_all();
    }
}

fn show_name(raw: &str) -> String {
    let s = display_text(raw, NAME_MAX);
    if s.is_empty() { "?".into() } else { s }
}

fn trust_name(t: TrustState) -> &'static str {
    match t {
        TrustState::New => "new",
        TrustState::Known => "known",
        TrustState::Verified => "verified",
        TrustState::KeyChanged => "key_changed",
    }
}

/// Ids come from the network: cleaned like names before they reach the app.
fn show_id(raw: &str) -> String {
    display_text(raw, ID_MAX_BYTES)
}

fn peer_key_json(p: &PeerKey) -> Value {
    json!({
        "id": show_id(&p.id),
        "name": show_name(&p.name),
        "platform": show_name(&p.platform),
        "fingerprint": p.fingerprint,
    })
}

/// SECURITY object of the events (see the module docs).
fn security_json(session: &SessionInfo) -> Value {
    match (&session.peer, session.trust, &session.sas) {
        (Some(peer), Some(trust), Some(sas)) if session.encrypted => json!({
            "encrypted": true,
            "trust": trust_name(trust),
            "name_matches_known": session.name_matches_known,
            "peer": peer_key_json(peer),
            "sas": {
                "value": sas.value, "emoji": sas.emoji, "text": sas.text, "digits": sas.digits,
                "it": sas.words(true), "en": sas.words(false),
            },
        }),
        _ => json!({ "encrypted": false }),
    }
}

/// A PEER object coming back from the app (`hoppy_node_trust_verify`, `qr_scan`).
fn parse_peer_key(text: &str) -> Option<PeerKey> {
    let v: Value = serde_json::from_str(text).ok()?;
    let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let key = PeerKey {
        id: field("id")?,
        name: field("name").unwrap_or_default(),
        platform: field("platform").unwrap_or_default(),
        fingerprint: field("fingerprint")?,
    };
    (!key.id.is_empty() && hoppy_core::crypto::is_fingerprint(&key.fingerprint)).then_some(key)
}

fn offer_event(id: u64, offer: &Offer, session: &SessionInfo) -> Value {
    let files: Vec<Value> = offer
        .files
        .iter()
        .take(OFFER_FILES_SHOWN)
        .map(|f| {
            let name = sanitize_file_name(&f.name)
                .map(|n| display_text(&n, FILE_NAME_MAX))
                .unwrap_or_default();
            let name = if name.is_empty() {
                "(?)".to_owned()
            } else {
                name
            };
            json!({ "name": name, "size": f.size })
        })
        .collect();
    json!({
        "type": "offer",
        "offer": id,
        "from": { "name": show_name(&offer.sender.name), "platform": show_name(&offer.sender.platform) },
        "files": files,
        "count": offer.files.len(),
        "total": offer.total_size(),
        "security": security_json(session),
    })
}

struct Handler(Arc<Shared>);

impl ReceiveHandler for Handler {
    fn decide(&self, offer: &Offer, from: SocketAddr, session: &SessionInfo) -> bool {
        let s = &self.0;
        let id = s.next_offer.fetch_add(1, Ordering::Relaxed);
        {
            let mut a = s.asking.lock().unwrap_or_else(|e| e.into_inner());
            if a.closed || a.current.is_some() {
                return false; // one question at a time; never accept unasked
            }
            a.current = Some((id, None));
        }
        s.events.emit(offer_event(id, offer, session));
        let deadline = Instant::now() + s.decision_timeout;
        let mut a = s.asking.lock().unwrap_or_else(|e| e.into_inner());
        let answer = loop {
            if let Some((_, Some(answer))) = a.current {
                break Some(answer);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if a.closed || left.is_zero() {
                break None;
            }
            a = s
                .answered
                .wait_timeout(a, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        };
        a.current = None;
        drop(a);
        if answer.is_none() {
            s.events
                .emit(json!({ "type": "offer_closed", "offer": id }));
        }
        let accept = answer == Some(true);
        if accept {
            s.senders
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(from, show_name(&offer.sender.name));
        }
        accept
    }

    fn blocked(&self, _from: SocketAddr, offer: &Offer, _session: &SessionInfo, reason: &str) {
        self.0.events.emit(json!({
            "type": "blocked",
            "from": {
                "id": show_id(&offer.sender.id),
                "name": show_name(&offer.sender.name),
                "platform": show_name(&offer.sender.platform),
            },
            "reason": reason,
        }));
    }

    fn warning(&self, from: SocketAddr, warning: &Warning) {
        self.0.events.emit(match warning {
            Warning::HandshakeAborts { recent } => json!({
                "type": "handshake_aborts", "from": from.ip().to_string(), "recent": recent,
            }),
            Warning::TrustNotSaved(e) => json!({ "type": "trust_not_saved", "error": e }),
        });
    }

    fn verified(&self, _from: SocketAddr, peer: &PeerKey) {
        self.0
            .events
            .emit(json!({ "type": "verified", "peer": peer_key_json(peer) }));
    }

    fn progress(&self, from: SocketAddr, event: Progress) {
        if let Progress::Bytes { done, total } = event
            && throttle(&self.0.last_progress, from, done == total)
        {
            self.0
                .events
                .emit(json!({ "type": "receive_progress", "done": done, "total": total }));
        }
    }

    fn finished(&self, from: SocketAddr, result: &Result<Vec<PathBuf>, Error>) {
        let s = &self.0;
        let name = s
            .senders
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&from)
            .unwrap_or_else(|| "?".into());
        s.last_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&from);
        s.events.emit(match result {
            Ok(paths) => json!({
                "type": "received", "ok": true, "from": name,
                "files": paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
            }),
            Err(e) => {
                json!({ "type": "received", "ok": false, "from": name, "error": e.to_string() })
            }
        });
    }
}

/// `true` when a progress event for `key` may go out now (or `last` forces it).
fn throttle<K: std::hash::Hash + Eq>(map: &Mutex<HashMap<K, Instant>>, key: K, last: bool) -> bool {
    let mut m = map.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    match m.get(&key) {
        Some(t) if !last && now.duration_since(*t) < PROGRESS_EVERY => false,
        _ => {
            m.insert(key, now);
            true
        }
    }
}

struct Receiver {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    port: u16,
}

/// A running Hoppy node (opaque to C).
pub struct Node {
    local: Local,
    /// A damaged trust store set aside at start (reported in `info`, not as an event: no
    /// callback may run inside `hoppy_node_start`).
    trust_warning: Mutex<Option<String>>,
    shared: Arc<Shared>,
    discovery: Arc<Discovery>,
    receive_dir: Option<PathBuf>,
    tcp_port: u16,
    receiver: Mutex<Option<Receiver>>,
    stop: Arc<AtomicBool>,
    poller: Mutex<Option<JoinHandle<()>>>,
    next_transfer: AtomicU64,
    send_progress: Arc<Mutex<HashMap<u64, Instant>>>,
}

fn peer_json(p: &Peer, trust: &TrustStore) -> Value {
    let state = match trust.peers.get(&p.device.id) {
        None => "new",
        Some(e) if e.verified => "verified",
        Some(_) => "known",
    };
    json!({
        "id": p.device.id,
        "name": show_name(&p.device.name),
        "platform": show_name(&p.device.platform),
        "addr": p.addr.ip().to_string(),
        "encrypted": p.crypto.unwrap_or(0) >= 1,
        "trust": state,
    })
}

impl Node {
    fn start(config: &str, callback: EventCallback, ctx: usize) -> Result<Node, String> {
        let cfg: Config = serde_json::from_str(config).map_err(|e| format!("config: {e}"))?;
        if cfg.id.trim().is_empty() || cfg.id.len() > ID_MAX_BYTES {
            return Err(format!("config: id must be 1..={ID_MAX_BYTES} bytes"));
        }
        if cfg.platform.is_empty() || cfg.platform.len() > PLATFORM_MAX_BYTES {
            return Err(format!(
                "config: platform must be 1..={PLATFORM_MAX_BYTES} bytes"
            ));
        }
        let me = DeviceInfo {
            id: cfg.id.clone(),
            name: cfg.name.chars().take(NAME_MAX).collect(),
            platform: cfg.platform.clone(),
        };
        let key = match &cfg.key_file {
            Some(path) => DeviceKey::load_or_create(path).map_err(|e| format!("key_file: {e}"))?,
            None => DeviceKey::generate(),
        };
        let trust = match &cfg.trust_file {
            Some(path) => Trust::open(path).map_err(|e| format!("trust_file: {e}"))?,
            None => Trust::in_memory(TrustStore::new()),
        };
        let local = Local::new(me.clone(), key, trust);
        let mut dcfg = DiscoveryConfig::new(me.clone(), 0);
        dcfg.bind_port = cfg.udp_port;
        dcfg.use_multicast = cfg.multicast;
        dcfg.seeds = cfg
            .seeds
            .iter()
            .filter_map(|s| parse_addr(s, DISCOVERY_PORT))
            .collect();
        dcfg.sweep = parse_sweep(cfg.sweep.as_ref())?;
        // Port taken (e.g. the Share extension while the app is alive): any port works for
        // our own queries; only unsolicited unicast to 47470 is missed meanwhile.
        let discovery = match Discovery::start(dcfg.clone()) {
            Ok(d) => d,
            Err(_) if dcfg.bind_port != 0 => {
                dcfg.bind_port = 0;
                Discovery::start(dcfg).map_err(|e| format!("discovery: {e}"))?
            }
            Err(e) => return Err(format!("discovery: {e}")),
        };
        let shared = Arc::new(Shared {
            events: Emitter(Mutex::new(Some((callback, ctx)))),
            asking: Mutex::new(Asking::default()),
            answered: Condvar::new(),
            next_offer: AtomicU64::new(1),
            decision_timeout: Duration::from_secs(
                cfg.decision_timeout_secs.clamp(1, DECISION_TIMEOUT_MAX),
            ),
            senders: Mutex::new(HashMap::new()),
            last_progress: Mutex::new(HashMap::new()),
        });
        let discovery = Arc::new(discovery);
        if let Some(dir) = &cfg.receive_dir {
            // Callbacks (spec §6) are answered only while receiving (transfer port != 0).
            accept_callbacks(
                &discovery,
                dir.clone(),
                local.clone(),
                Arc::new(Handler(shared.clone())),
            );
        }
        let trust_warning = Mutex::new(local.trust.take_warning());
        let node = Node {
            local,
            trust_warning,
            shared,
            discovery,
            receive_dir: cfg.receive_dir.clone(),
            tcp_port: cfg.tcp_port,
            receiver: Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
            poller: Mutex::new(None),
            next_transfer: AtomicU64::new(1),
            send_progress: Arc::new(Mutex::new(HashMap::new())),
        };
        node.start_poller();
        if cfg.receive {
            node.set_receiving(true)?;
        }
        Ok(node)
    }

    fn start_poller(&self) {
        let (discovery, shared, stop, trust) = (
            self.discovery.clone(),
            self.shared.clone(),
            self.stop.clone(),
            self.local.trust.clone(),
        );
        let handle = thread::Builder::new()
            .name("hoppy-ffi-peers".into())
            .spawn(move || {
                let mut last: Option<Vec<Value>> = None;
                while !stop.load(Ordering::Relaxed) {
                    let store = trust.snapshot();
                    let list: Vec<Value> = discovery
                        .peers()
                        .iter()
                        .map(|p| peer_json(p, &store))
                        .collect();
                    if last.as_ref() != Some(&list) {
                        shared
                            .events
                            .emit(json!({ "type": "peers", "peers": list.clone() }));
                        last = Some(list);
                    }
                    thread::sleep(PEER_POLL);
                }
            })
            .ok();
        *self.poller.lock().unwrap_or_else(|e| e.into_inner()) = handle;
    }

    fn info(&self) -> Value {
        json!({
            "id": self.local.device.id,
            "name": self.local.device.name,
            "platform": self.local.device.platform,
            "udp_port": self.discovery.local_port(),
            "tcp_port": self.discovery.transfer_port(),
            "fingerprint": self.local.fingerprint(),
            "trust_warning": self.trust_warning.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        })
    }

    fn peers(&self) -> Value {
        let store = self.local.trust.snapshot();
        Value::Array(
            self.discovery
                .peers()
                .iter()
                .map(|p| peer_json(p, &store))
                .collect(),
        )
    }

    fn trust_list(&self) -> Value {
        let store = self.local.trust.snapshot();
        let peers: Vec<Value> = store
            .peers
            .iter()
            .map(|(id, e)| {
                json!({
                    "id": show_id(id), "name": show_name(&e.name), "fingerprint": e.fingerprint,
                    "first_seen": e.first_seen, "verified": e.verified,
                })
            })
            .collect();
        json!({ "me": { "fingerprint": self.local.fingerprint() }, "peers": peers })
    }

    /// QR scan (spec §7.7), on a worker: local check, then tell the other device.
    fn qr_scan(&self, payload: String, live: Option<PeerKey>) -> u64 {
        let id = self.next_transfer.fetch_add(1, Ordering::Relaxed);
        let (discovery, shared, local) = (
            self.discovery.clone(),
            self.shared.clone(),
            self.local.clone(),
        );
        let spawned = thread::Builder::new()
            .name("hoppy-ffi-qr".into())
            .spawn(move || {
                let done = |local_ok: bool, remote: bool, error: Option<String>| {
                    shared.events.emit(json!({
                        "type": "qr_done", "scan": id, "ok": local_ok && remote,
                        "local": local_ok, "remote": remote, "error": error,
                    }));
                };
                let (qr, _) = match local.trust.check_qr(&payload, live.as_ref()) {
                    Ok(v) => v,
                    Err(e) => return done(false, false, Some(e.to_string())),
                };
                let Some(peer) = wait_for_peer(&discovery, &qr.id) else {
                    return done(true, false, Some("device not found".into()));
                };
                match confirm_qr(&peer, &local, Some(&discovery), &qr) {
                    Ok(()) => done(true, true, None),
                    Err(e) => done(true, false, Some(e.to_string())),
                }
            });
        if spawned.is_err() {
            return 0;
        }
        id
    }

    /// Turns the TCP receiver on/off; returns the port (0 = off).
    fn set_receiving(&self, on: bool) -> Result<u16, String> {
        let mut slot = self.receiver.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            if let Some(r) = slot.as_ref() {
                return Ok(r.port);
            }
            let dir = self
                .receive_dir
                .clone()
                .ok_or("receiving needs receive_dir")?;
            let listener = TcpListener::bind(("0.0.0.0", self.tcp_port))
                .or_else(|_| TcpListener::bind(("0.0.0.0", 0)))
                .map_err(|e| format!("cannot open a TCP port: {e}"))?;
            listener
                .set_nonblocking(true)
                .map_err(|e| format!("listener: {e}"))?;
            let port = listener.local_addr().map_err(|e| e.to_string())?.port();
            let stop = Arc::new(AtomicBool::new(false));
            let handler = Arc::new(Handler(self.shared.clone()));
            let stop2 = stop.clone();
            let local = self.local.clone();
            let thread = thread::Builder::new()
                .name("hoppy-ffi-accept".into())
                .spawn(move || accept_loop(listener, dir, local, handler, &stop2))
                .map_err(|e| e.to_string())?;
            *slot = Some(Receiver { stop, thread, port });
            self.discovery.set_transfer_port(port);
            drop(slot);
            self.shared
                .events
                .emit(json!({ "type": "receiving", "on": true, "port": port }));
            Ok(port)
        } else {
            self.discovery.set_transfer_port(0);
            if let Some(r) = slot.take() {
                r.stop.store(true, Ordering::Relaxed);
                let _ = r.thread.join();
            }
            drop(slot);
            self.shared
                .events
                .emit(json!({ "type": "receiving", "on": false, "port": 0 }));
            Ok(0)
        }
    }

    fn send(&self, peer_id: String, paths: Vec<PathBuf>) -> u64 {
        let id = self.next_transfer.fetch_add(1, Ordering::Relaxed);
        let (discovery, shared, local, progress_map) = (
            self.discovery.clone(),
            self.shared.clone(),
            self.local.clone(),
            self.send_progress.clone(),
        );
        let spawned = thread::Builder::new()
            .name("hoppy-ffi-send".into())
            .spawn(move || {
                let done = |outcome: &str, message: Option<String>| {
                    shared.events.emit(json!({
                        "type": "send_done", "transfer": id, "outcome": outcome, "message": message,
                    }));
                };
                // The peer may have just expired from the list: wait for it a little.
                let Some(peer) = wait_for_peer(&discovery, &peer_id) else {
                    return done("unreachable", Some("device not found".into()));
                };
                let mut progress = |p: Progress| match p {
                    Progress::Warning(Warning::TrustNotSaved(e)) => {
                        shared.events.emit(json!({
                            "type": "send_warning", "transfer": id,
                            "kind": "trust_not_saved", "error": e,
                        }));
                    }
                    Progress::Session(session) => {
                        shared.events.emit(json!({
                            "type": "send_session", "transfer": id,
                            "security": security_json(&session),
                        }));
                    }
                    Progress::Fallback => {
                        shared
                            .events
                            .emit(json!({ "type": "send_fallback", "transfer": id }));
                    }
                    Progress::Bytes { done, total }
                        if throttle(&progress_map, id, done == total) =>
                    {
                        shared.events.emit(json!({
                            "type": "send_progress", "transfer": id, "done": done, "total": total,
                        }));
                    }
                    _ => {}
                };
                let result = send_to_peer(&peer, &local, Some(&discovery), &paths, &mut progress);
                progress_map
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                match result {
                    Ok(SendOutcome::Delivered) => done("delivered", None),
                    Ok(SendOutcome::Declined(reason)) => done("declined", reason),
                    Err(Error::Unreachable(m)) => done("unreachable", Some(m)),
                    Err(e @ Error::KeyChanged(_)) => done("key_changed", Some(e.to_string())),
                    Err(e @ Error::Unencrypted(_)) => done("unencrypted", Some(e.to_string())),
                    Err(e) => done("error", Some(e.to_string())),
                }
            });
        if spawned.is_err() {
            self.shared.events.emit(json!({
                "type": "send_done", "transfer": id, "outcome": "error", "message": "no thread",
            }));
        }
        id
    }

    fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.shared.close();
        if let Some(r) = self
            .receiver
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            r.stop.store(true, Ordering::Relaxed);
            let _ = r.thread.join();
        }
        if let Some(t) = self.poller.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = t.join();
        }
        self.shared.events.clear();
    }
}

/// Polls for `id` up to 3 s (a send right after the list refreshed).
fn wait_for_peer(discovery: &Discovery, id: &str) -> Option<Peer> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(p) = discovery.peers().into_iter().find(|p| p.device.id == id) {
            return Some(p);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn accept_loop(
    listener: TcpListener,
    dir: PathBuf,
    local: Local,
    handler: Arc<Handler>,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, from)) => {
                let _ = stream.set_nonblocking(false);
                let (dir, handler, local) = (dir.clone(), handler.clone(), local.clone());
                let _ = thread::Builder::new()
                    .name("hoppy-ffi-receive".into())
                    .spawn(move || {
                        if let Some(result) =
                            receive_one(stream, from, &dir, &local, handler.as_ref())
                        {
                            handler.finished(from, &result);
                        }
                    });
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(50)),
            Err(_) => thread::sleep(Duration::from_millis(200)),
        }
    }
}

// ------------------------------------------------------------------ C ABI (include/hoppy.h)

/// # Safety
/// `s` is NULL or a valid NUL-terminated string.
unsafe fn read_str(s: *const c_char) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: caller contract.
    unsafe { CStr::from_ptr(s) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

/// Runs an FFI body; a panic becomes `fallback` instead of unwinding into C/Swift.
fn guard<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).unwrap_or(fallback)
}

fn to_c(s: String) -> *mut c_char {
    CString::new(s).map_or(ptr::null_mut(), CString::into_raw)
}

/// Starts a node. Returns NULL on failure and, if `error_out` is not NULL, stores a message
/// there (free it with `hoppy_string_free`).
///
/// # Safety
/// `config_json` is a valid NUL-terminated string; `error_out` is NULL or writable;
/// `ctx` stays valid until `hoppy_node_stop` returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_start(
    config_json: *const c_char,
    callback: Option<EventCallback>,
    ctx: *mut c_void,
    error_out: *mut *mut c_char,
) -> *mut Node {
    let fail = |msg: String| {
        if !error_out.is_null() {
            // SAFETY: caller contract.
            unsafe { *error_out = to_c(msg) };
        }
        ptr::null_mut()
    };
    // SAFETY: caller contract.
    let Some(config) = (unsafe { read_str(config_json) }) else {
        return fail("config: not UTF-8".into());
    };
    let Some(callback) = callback else {
        return fail("no callback".into());
    };
    match guard(Err("internal error".into()), || {
        Node::start(&config, callback, ctx as usize)
    }) {
        Ok(node) => Box::into_raw(Box::new(node)),
        Err(e) => fail(e),
    }
}

/// Stops the node (sends `bye`, closes the receiver, declines a pending question) and frees
/// it. No event is delivered after this returns. Transfers already running finish in the
/// background silently.
///
/// # Safety
/// `node` comes from `hoppy_node_start` and is not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_stop(node: *mut Node) {
    if node.is_null() {
        return;
    }
    // SAFETY: caller contract (ownership returns to Rust).
    let node = unsafe { Box::from_raw(node) };
    guard((), move || node.shutdown());
}

/// `{"id","name","platform","udp_port","tcp_port"}` (tcp_port 0 = not receiving).
///
/// # Safety
/// `node` is a live node.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_info(node: *const Node) -> *mut c_char {
    // SAFETY: caller contract.
    let node = unsafe { node.as_ref() };
    guard(ptr::null_mut(), || {
        node.map_or(ptr::null_mut(), |n| to_c(n.info().to_string()))
    })
}

/// Current peers as a JSON array (same items as the `peers` event).
///
/// # Safety
/// `node` is a live node.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_peers(node: *const Node) -> *mut c_char {
    // SAFETY: caller contract.
    let node = unsafe { node.as_ref() };
    guard(ptr::null_mut(), || {
        node.map_or(ptr::null_mut(), |n| to_c(n.peers().to_string()))
    })
}

/// Queries everyone again and re-sweeps the subnet (rate-limited).
///
/// # Safety
/// `node` is a live node.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_refresh(node: *const Node) {
    // SAFETY: caller contract.
    if let Some(n) = unsafe { node.as_ref() } {
        guard((), || n.discovery.refresh());
    }
}

/// Turns receiving on/off. Returns the TCP port when on, 0 when off, -1 on error.
///
/// # Safety
/// `node` is a live node.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_set_receiving(node: *const Node, on: bool) -> i32 {
    // SAFETY: caller contract.
    let node = unsafe { node.as_ref() };
    guard(-1, || match node.map(|n| n.set_receiving(on)) {
        Some(Ok(port)) => i32::from(port),
        _ => -1,
    })
}

/// Sends files (JSON array of absolute paths) to the peer with `peer_id`. Returns the
/// transfer id used by the `send_*` events, or 0 if the arguments are invalid.
///
/// # Safety
/// `node` is a live node; the strings are valid NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_send(
    node: *const Node,
    peer_id: *const c_char,
    paths_json: *const c_char,
) -> u64 {
    // SAFETY: caller contract.
    let (Some(n), Some(peer), Some(paths)) = (
        unsafe { node.as_ref() },
        unsafe { read_str(peer_id) },
        unsafe { read_str(paths_json) },
    ) else {
        return 0;
    };
    guard(0, || {
        let Ok(paths) = serde_json::from_str::<Vec<PathBuf>>(&paths) else {
            return 0;
        };
        if paths.is_empty() {
            return 0;
        }
        n.send(peer, paths)
    })
}

/// Answers the `offer` event `offer`. Returns false if it is no longer pending.
///
/// # Safety
/// `node` is a live node.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_decide(node: *const Node, offer: u64, accept: bool) -> bool {
    // SAFETY: caller contract.
    let node = unsafe { node.as_ref() };
    guard(false, || {
        node.is_some_and(|n| n.shared.answer(offer, accept))
    })
}

/// `{"me":{"fingerprint"},"peers":[{"id","name","fingerprint","first_seen","verified"}]}`.
///
/// # Safety
/// `node` is a live node.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_trust_list(node: *const Node) -> *mut c_char {
    // SAFETY: caller contract.
    let node = unsafe { node.as_ref() };
    guard(ptr::null_mut(), || {
        node.map_or(ptr::null_mut(), |n| to_c(n.trust_list().to_string()))
    })
}

/// "Forget this device": its next contact is new again (re-trust after a reinstall).
/// Returns false if it was not remembered or the store could not be saved.
///
/// # Safety
/// `node` is a live node; `peer_id` a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_trust_forget(
    node: *const Node,
    peer_id: *const c_char,
) -> bool {
    // SAFETY: caller contract.
    let (Some(n), Some(id)) = (unsafe { node.as_ref() }, unsafe { read_str(peer_id) }) else {
        return false;
    };
    guard(false, || n.local.trust.forget(&id).unwrap_or(false))
}

/// "They match": marks the PEER of an `offer` / `send_session` event verified (the users
/// compared the code). False if that id is remembered with another key (forget it first),
/// or the JSON is not a PEER.
///
/// # Safety
/// `node` is a live node; `peer_json` a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_trust_verify(
    node: *const Node,
    peer_json: *const c_char,
) -> bool {
    // SAFETY: caller contract.
    let (Some(n), Some(text)) = (unsafe { node.as_ref() }, unsafe { read_str(peer_json) }) else {
        return false;
    };
    guard(false, || {
        parse_peer_key(&text).is_some_and(|p| n.local.trust.mark_verified(&p).is_ok())
    })
}

/// Starts showing this device's QR code: returns the payload to render (a plain string,
/// free it with `hoppy_string_free`). Its one-time code is accepted for 10 minutes.
///
/// # Safety
/// `node` is a live node.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_qr_begin(node: *const Node) -> *mut c_char {
    // SAFETY: caller contract.
    let node = unsafe { node.as_ref() };
    guard(ptr::null_mut(), || {
        node.map_or(ptr::null_mut(), |n| to_c(n.local.begin_qr()))
    })
}

/// The QR code was closed: its code is no longer accepted.
///
/// # Safety
/// `node` is a live node.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_qr_end(node: *const Node) {
    // SAFETY: caller contract.
    if let Some(n) = unsafe { node.as_ref() } {
        guard((), || n.local.trust.end_qr());
    }
}

/// A scanned QR payload: checks it against the key known for that device (`live_peer_json`
/// = the PEER of the session on screen, or NULL to use the remembered key), marks it
/// verified, then tells that device (event `qr_done`). Returns the scan id, 0 if invalid.
///
/// # Safety
/// `node` is a live node; `payload` a valid string; `live_peer_json` NULL or a valid string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_node_qr_scan(
    node: *const Node,
    payload: *const c_char,
    live_peer_json: *const c_char,
) -> u64 {
    // SAFETY: caller contract.
    let (Some(n), Some(payload)) = (unsafe { node.as_ref() }, unsafe { read_str(payload) }) else {
        return 0;
    };
    // SAFETY: caller contract (NULL allowed).
    let live = unsafe { read_str(live_peer_json) }.and_then(|t| parse_peer_key(&t));
    guard(0, || {
        if hoppy_core::QrPayload::parse(&payload).is_none() {
            return 0;
        }
        n.qr_scan(payload, live)
    })
}

/// Frees a string returned by this library.
///
/// # Safety
/// `s` is NULL or came from this library and is not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hoppy_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: caller contract.
        drop(unsafe { CString::from_raw(s) });
    }
}

/// Library version (static string, do not free).
#[unsafe(no_mangle)]
pub extern "C" fn hoppy_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

#[cfg(test)]
mod tests;
