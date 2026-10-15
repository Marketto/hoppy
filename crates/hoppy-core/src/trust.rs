//! Trust on first use (spec §7.5) and device verification (§7.6: comparing 4 emoji).
//!
//! [`TrustStore`] is pure data (`id → fingerprint, name, first_seen, verified, addr`), saved
//! as JSON by the platform. [`Trust`] wraps it for a running node: thread-safe, optionally
//! backed by a file that every change re-reads, merges and rewrites under a lock file, so two
//! processes of the same user (tray and "Send to") do not undo each other.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::crypto;
use crate::protocol::DeviceInfo;

/// A lock file older than this belongs to a crashed process and is removed.
const STALE_LOCK: Duration = Duration::from_secs(10);
/// How long a change waits for another process's lock.
const LOCK_WAIT: Duration = Duration::from_secs(3);

/// What the UI shows about the other device of a session (spec §7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustState {
    /// First contact: never seen. Show the SAS (4 emoji), offer "They match" / "Later".
    New,
    /// Seen before with this key (TOFU), not verified. Same as New in the UI.
    Known,
    /// The user compared the emoji and confirmed "They match": show ✓.
    Verified,
    /// Seen before with a DIFFERENT key: block (refuse to send, auto-decline) and warn.
    KeyChanged,
}

/// The other device of an encrypted session, as authenticated by the handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerKey {
    pub id: String,
    pub name: String,
    pub platform: String,
    /// [`crypto::fingerprint`] of its static key.
    pub fingerprint: String,
}

impl PeerKey {
    pub fn new(device: &DeviceInfo, fingerprint: String) -> Self {
        Self {
            id: device.id.clone(),
            name: device.name.clone(),
            platform: device.platform.clone(),
            fingerprint,
        }
    }
}

/// Security of one session, for the UIs: the receiver's `decide` gets it with the offer,
/// the sender gets it as `Progress::Session` before the offer goes out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionInfo {
    /// `false`: v0 peer (old app), nothing is encrypted — show "non cifrato" / "not
    /// encrypted". The other fields are then `None`.
    pub encrypted: bool,
    pub peer: Option<PeerKey>,
    pub trust: Option<TrustState>,
    /// Same value on both screens of an honest session.
    pub sas: Option<crypto::Sas>,
    /// A New device that uses the name of a device already remembered under another id:
    /// show a warning ("another device calls itself X"), it may be an impostor.
    pub name_matches_known: bool,
}

impl SessionInfo {
    pub fn plaintext() -> Self {
        Self {
            encrypted: false,
            peer: None,
            trust: None,
            sas: None,
            name_matches_known: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustEntry {
    pub fingerprint: String,
    /// Last name seen with this key (display only).
    pub name: String,
    /// Unix seconds.
    pub first_seen: u64,
    pub verified: bool,
    /// Transfer address (`ip:port`) where this device last completed an encrypted transfer
    /// to it: a typed address never falls back to plaintext there (spec §7.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
}

/// The persisted trust store: `{"version":1,"peers":{"<id>":{…TrustEntry}}}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustStore {
    #[serde(default = "store_version")]
    pub version: u8,
    #[serde(default)]
    pub peers: BTreeMap<String, TrustEntry>,
}

fn store_version() -> u8 {
    1
}

impl TrustStore {
    pub fn new() -> Self {
        Self {
            version: 1,
            peers: BTreeMap::new(),
        }
    }

    pub fn from_json(text: &str) -> Result<Self, Error> {
        serde_json::from_str(text).map_err(|e| Error::Input(format!("trust store: {e}")))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("trust store always serializes")
    }

    pub fn state(&self, id: &str, fingerprint: &str) -> TrustState {
        match self.peers.get(id) {
            None => TrustState::New,
            Some(e) if e.fingerprint != fingerprint => TrustState::KeyChanged,
            Some(e) if e.verified => TrustState::Verified,
            Some(_) => TrustState::Known,
        }
    }

    /// TOFU: remembers a first-seen key; refreshes the name of a known one. Never replaces
    /// a different key (that is [`Self::retrust`], an explicit user action). Returns whether
    /// anything changed.
    pub fn pin(&mut self, peer: &PeerKey, now: u64) -> bool {
        self.pin_at(peer, None, now)
    }

    /// [`Self::pin`], also remembering the transfer address it was reached at.
    pub fn pin_at(&mut self, peer: &PeerKey, addr: Option<String>, now: u64) -> bool {
        match self.peers.get_mut(&peer.id) {
            None => {
                self.peers.insert(
                    peer.id.clone(),
                    TrustEntry {
                        fingerprint: peer.fingerprint.clone(),
                        name: peer.name.clone(),
                        first_seen: now,
                        verified: false,
                        addr,
                    },
                );
                true
            }
            Some(e) if e.fingerprint == peer.fingerprint => {
                let mut changed = false;
                if e.name != peer.name {
                    e.name = peer.name.clone();
                    changed = true;
                }
                if addr.is_some() && e.addr != addr {
                    e.addr = addr;
                    changed = true;
                }
                changed
            }
            Some(_) => false,
        }
    }

    /// Some remembered device completed an encrypted transfer at `addr` (`ip:port`).
    pub fn encrypted_at(&self, addr: &str) -> bool {
        self.peers.values().any(|e| e.addr.as_deref() == Some(addr))
    }

    /// Another remembered id already uses this name (case-insensitive, trimmed).
    pub fn name_taken_by_other(&self, id: &str, name: &str) -> bool {
        let n = name.trim().to_lowercase();
        !n.is_empty()
            && self
                .peers
                .iter()
                .any(|(other, e)| other != id && e.name.trim().to_lowercase() == n)
    }

    /// Marks `peer`'s key verified (pinning it on first contact). Fails with
    /// [`Error::KeyChanged`] if another key is pinned for that id: forget it first.
    pub fn set_verified(&mut self, peer: &PeerKey, now: u64) -> Result<(), Error> {
        if self.state(&peer.id, &peer.fingerprint) == TrustState::KeyChanged {
            return Err(Error::KeyChanged(peer.name.clone()));
        }
        self.pin(peer, now);
        if let Some(e) = self.peers.get_mut(&peer.id) {
            e.verified = true;
        }
        Ok(())
    }

    /// Replaces whatever is pinned for `peer.id` with its current key, unverified (the user
    /// confirmed the device was reinstalled / reset).
    pub fn retrust(&mut self, peer: &PeerKey, now: u64) {
        self.peers.remove(&peer.id);
        self.pin(peer, now);
    }

    pub fn forget(&mut self, id: &str) -> bool {
        self.peers.remove(id).is_some()
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

struct Inner {
    store: TrustStore,
    path: Option<PathBuf>,
    /// Something the user should hear about (corrupt file set aside, …), taken once.
    warning: Option<String>,
}

/// Reads the store file: `Ok(None)` if missing; a file that is not a store is renamed to
/// `<path>.bad` and reported (the caller starts from what it has).
fn read_store(path: &Path) -> Result<(Option<TrustStore>, Option<String>), Error> {
    match fs::read_to_string(path) {
        Ok(text) => match TrustStore::from_json(&text) {
            Ok(store) => Ok((Some(store), None)),
            Err(e) => {
                let bad = path.with_extension("json.bad");
                let _ = fs::rename(path, &bad);
                Ok((
                    None,
                    Some(format!(
                        "the list of known devices was damaged ({e}); it was set aside as {}",
                        bad.display()
                    )),
                ))
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((None, None)),
        Err(e) => Err(e.into()),
    }
}

/// An exclusive lock file next to the store, removed on drop.
struct FileLock(PathBuf);

impl FileLock {
    fn acquire(store: &Path) -> Result<Self, Error> {
        let name = store
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let path = store.with_file_name(format!("{name}.lock"));
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let start = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > STALE_LOCK);
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if start.elapsed() > LOCK_WAIT {
                        return Err(Error::Io(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "the list of known devices is locked by another Hoppy process",
                        )));
                    }
                    thread::sleep(Duration::from_millis(15));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Inner {
    /// Picks up changes another process wrote (a damaged file is set aside: we keep ours).
    fn reload(&mut self) {
        if let Some(path) = self.path.clone()
            && let Ok((store, warning)) = read_store(&path)
        {
            if let Some(store) = store {
                self.store = store;
            }
            if warning.is_some() {
                self.warning = warning;
            }
        }
    }

    fn save(&self, store: &TrustStore) -> Result<(), Error> {
        match &self.path {
            Some(path) => crypto::write_private(path, store.to_json().as_bytes()),
            None => Ok(()),
        }
    }
}

/// Shared, thread-safe trust store of a node (cheap to clone).
#[derive(Clone)]
pub struct Trust {
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for Trust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trust").finish_non_exhaustive()
    }
}

impl Trust {
    /// Not persisted (tests, or a platform that saves [`Self::snapshot`] itself).
    pub fn in_memory(store: TrustStore) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                store,
                path: None,
                warning: None,
            })),
        }
    }

    /// Backed by a JSON file (created on the first change, owner-only on Unix). A damaged
    /// file does not stop the app: it is set aside as `<path>.bad`, the store starts empty
    /// and [`Self::take_warning`] says so.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let (store, warning) = read_store(path)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                store: store.unwrap_or_else(TrustStore::new),
                path: Some(path.to_path_buf()),
                warning,
            })),
        })
    }

    /// A problem worth telling the user about (e.g. a damaged file set aside), once.
    pub fn take_warning(&self) -> Option<String> {
        self.lock().warning.take()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Under the file lock: re-reads the store, runs `f`, saves if `f` says it changed.
    fn update<T>(&self, f: impl FnOnce(&mut TrustStore) -> (T, bool)) -> Result<T, Error> {
        let mut inner = self.lock();
        let _file_lock = match &inner.path {
            Some(path) => Some(FileLock::acquire(path)?),
            None => None,
        };
        inner.reload();
        // Change a copy; it replaces the store only once it is saved (a failed write leaves
        // memory and file in agreement).
        let mut next = inner.store.clone();
        let (out, changed) = f(&mut next);
        if changed {
            inner.save(&next)?;
            inner.store = next;
        }
        Ok(out)
    }

    pub fn snapshot(&self) -> TrustStore {
        let mut inner = self.lock();
        inner.reload();
        inner.store.clone()
    }

    pub fn state(&self, id: &str, fingerprint: &str) -> TrustState {
        self.snapshot().state(id, fingerprint)
    }

    pub fn get(&self, id: &str) -> Option<TrustEntry> {
        self.snapshot().peers.get(id).cloned()
    }

    /// TOFU pin after an accepted transfer. An error means it could not be saved: tell the
    /// user (the device will look new next time).
    pub fn pin(&self, peer: &PeerKey) -> Result<(), Error> {
        self.pin_at(peer, None)
    }

    /// [`Self::pin`] plus the transfer address the device was reached at (sender side).
    pub fn pin_at(&self, peer: &PeerKey, addr: Option<SocketAddr>) -> Result<(), Error> {
        let addr = addr.map(|a| SocketAddr::new(a.ip().to_canonical(), a.port()).to_string());
        self.update(|s| ((), s.pin_at(peer, addr, unix_now())))
    }

    /// A remembered device completed an encrypted transfer at this address before.
    pub fn encrypted_at(&self, addr: SocketAddr) -> bool {
        let addr = SocketAddr::new(addr.ip().to_canonical(), addr.port()).to_string();
        self.snapshot().encrypted_at(&addr)
    }

    /// See [`TrustStore::name_taken_by_other`].
    pub fn name_taken_by_other(&self, id: &str, name: &str) -> bool {
        self.snapshot().name_taken_by_other(id, name)
    }

    /// "Coincide" / "They match" after comparing the SAS (local only: nothing is sent).
    pub fn mark_verified(&self, peer: &PeerKey) -> Result<(), Error> {
        self.update(|s| (s.set_verified(peer, unix_now()), true))?
    }

    pub fn retrust(&self, peer: &PeerKey) -> Result<(), Error> {
        self.update(|s| {
            s.retrust(peer, unix_now());
            ((), true)
        })
    }

    /// "Forget this device": the next contact is New again. `false` if it was not known.
    pub fn forget(&self, id: &str) -> Result<bool, Error> {
        self.update(|s| {
            let removed = s.forget(id);
            (removed, removed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: &str, fp: &str) -> PeerKey {
        PeerKey {
            id: id.into(),
            name: format!("name-{id}"),
            platform: "linux".into(),
            fingerprint: fp.into(),
        }
    }

    #[test]
    fn tofu_states() {
        let mut s = TrustStore::new();
        let a = peer("a", "FP1");
        assert_eq!(s.state("a", "FP1"), TrustState::New);
        assert!(s.pin(&a, 10));
        assert!(!s.pin(&a, 11));
        assert_eq!(s.state("a", "FP1"), TrustState::Known);
        assert_eq!(s.state("a", "FP2"), TrustState::KeyChanged);
        assert!(!s.pin(&peer("a", "FP2"), 12), "pin never replaces a key");
        assert_eq!(s.peers["a"].fingerprint, "FP1");
        s.set_verified(&a, 13).unwrap();
        assert_eq!(s.state("a", "FP1"), TrustState::Verified);
        assert_eq!(s.peers["a"].first_seen, 10);
        assert!(matches!(
            s.set_verified(&peer("a", "FP2"), 14),
            Err(Error::KeyChanged(_))
        ));
        s.retrust(&peer("a", "FP2"), 15);
        assert_eq!(s.state("a", "FP2"), TrustState::Known);
        assert!(s.forget("a"));
        assert!(!s.forget("a"));
        assert_eq!(s.state("a", "FP2"), TrustState::New);
        s.set_verified(&peer("b", "X"), 1).unwrap();
        assert_eq!(
            s.state("b", "X"),
            TrustState::Verified,
            "verify pins on first contact"
        );
        let back = TrustStore::from_json(&s.to_json()).unwrap();
        assert_eq!(back, s);
        assert_eq!(TrustStore::from_json("{}").unwrap().version, 1);
    }

    #[test]
    fn file_store_is_shared_between_handles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");
        let one = Trust::open(&path).unwrap();
        let two = Trust::open(&path).unwrap();
        one.pin(&peer("a", "FP")).unwrap();
        assert_eq!(
            two.state("a", "FP"),
            TrustState::Known,
            "re-read before use"
        );
        two.mark_verified(&peer("a", "FP")).unwrap();
        assert_eq!(one.state("a", "FP"), TrustState::Verified);
        assert!(one.forget("a").unwrap());
        assert_eq!(two.state("a", "FP"), TrustState::New);

        // Concurrent writers from several handles: nobody's change is lost.
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let t = Trust::open(&path).unwrap();
                thread::spawn(move || t.pin(&peer(&format!("p{i}"), "FP")).unwrap())
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(one.snapshot().peers.len(), 8);
        assert!(!path.with_file_name("trust.json.lock").exists());

        // A damaged file is set aside, not fatal.
        fs::write(&path, "not json").unwrap();
        let t = Trust::open(&path).unwrap();
        assert!(t.snapshot().peers.is_empty());
        assert!(t.take_warning().unwrap().contains("damaged"));
        assert!(t.take_warning().is_none());
        assert!(path.with_extension("json.bad").exists());
        t.pin(&peer("a", "FP")).unwrap();
        assert_eq!(
            Trust::open(&path).unwrap().state("a", "FP"),
            TrustState::Known
        );
        // A stale lock (crashed process) does not block.
        fs::write(path.with_file_name("trust.json.lock"), "").unwrap();
        let old = SystemTime::now() - Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(path.with_file_name("trust.json.lock"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        t.pin(&peer("b", "FP")).unwrap();
    }

    #[test]
    fn addresses_and_name_collisions() {
        let t = Trust::in_memory(TrustStore::new());
        let addr: SocketAddr = "192.168.1.5:47471".parse().unwrap();
        assert!(!t.encrypted_at(addr));
        t.pin_at(&peer("a", "FP"), Some(addr)).unwrap();
        assert!(t.encrypted_at(addr));
        assert!(t.encrypted_at("[::ffff:192.168.1.5]:47471".parse().unwrap()));
        assert!(!t.encrypted_at("192.168.1.5:47472".parse().unwrap()));
        assert!(t.name_taken_by_other("b", " NAME-A "));
        assert!(!t.name_taken_by_other("a", "name-a"), "its own name");
        assert!(!t.name_taken_by_other("b", "other"));
    }
}
