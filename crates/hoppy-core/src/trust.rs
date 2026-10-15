//! Trust on first use (spec §7.5) and device verification (§7.6–7.7).
//!
//! [`TrustStore`] is pure data (`id → fingerprint, name, first_seen, verified`), saved as
//! JSON by the platform. [`Trust`] wraps it for a running node: thread-safe, optionally
//! backed by a file (re-read before every change so two processes of the same user — tray
//! and "Send to" — do not undo each other), plus the one-time codes of QR codes on screen.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::crypto::{self, DeviceKey, QrPayload};
use crate::protocol::DeviceInfo;

/// A QR code on screen accepts its one-time code this long.
pub const QR_CODE_TTL: Duration = Duration::from_secs(600);
/// QR codes remembered at once (older ones are dropped).
const MAX_QR_CODES: usize = 4;

/// What the UI shows about the other device of a session (spec §7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustState {
    /// First contact: never seen. Show the SAS and offer "Verify now" / "Later".
    New,
    /// Seen before with this key (TOFU), not verified. Show the SAS, offer "Verify".
    Known,
    /// The user verified this key (code compared or QR scanned): show ✓.
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
}

impl SessionInfo {
    pub fn plaintext() -> Self {
        Self {
            encrypted: false,
            peer: None,
            trust: None,
            sas: None,
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
        match self.peers.get_mut(&peer.id) {
            None => {
                self.peers.insert(
                    peer.id.clone(),
                    TrustEntry {
                        fingerprint: peer.fingerprint.clone(),
                        name: peer.name.clone(),
                        first_seen: now,
                        verified: false,
                    },
                );
                true
            }
            Some(e) if e.fingerprint == peer.fingerprint && e.name != peer.name => {
                e.name = peer.name.clone();
                true
            }
            Some(_) => false,
        }
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
    qr_codes: Vec<(String, Instant)>,
}

impl Inner {
    /// Picks up changes another process wrote (unreadable files are ignored: we keep ours).
    fn reload(&mut self) {
        if let Some(path) = &self.path
            && let Ok(text) = fs::read_to_string(path)
            && let Ok(store) = TrustStore::from_json(&text)
        {
            self.store = store;
        }
    }

    fn save(&self) -> Result<(), Error> {
        match &self.path {
            Some(path) => crypto::write_private(path, self.store.to_json().as_bytes()),
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
                qr_codes: Vec::new(),
            })),
        }
    }

    /// Backed by a JSON file (created on the first change, owner-only on Unix).
    pub fn open(path: &Path) -> Result<Self, Error> {
        let store = match fs::read_to_string(path) {
            Ok(text) => TrustStore::from_json(&text)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => TrustStore::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                store,
                path: Some(path.to_path_buf()),
                qr_codes: Vec::new(),
            })),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on the freshly re-read store and saves it if `f` says it changed.
    fn update<T>(&self, f: impl FnOnce(&mut TrustStore) -> (T, bool)) -> Result<T, Error> {
        let mut inner = self.lock();
        inner.reload();
        let (out, changed) = f(&mut inner.store);
        if changed {
            inner.save()?;
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

    /// TOFU pin after an accepted transfer (errors saving are ignored: best effort).
    pub fn pin(&self, peer: &PeerKey) {
        let _ = self.update(|s| ((), s.pin(peer, unix_now())));
    }

    /// "Coincide" / "They match" after comparing the SAS, or a QR scan that matched.
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

    /// Starts showing a QR code for this device (spec §7.7): returns the payload to render.
    /// Its one-time code is accepted by this node's receiver for [`QR_CODE_TTL`].
    pub fn begin_qr(&self, me: &DeviceInfo, key: &DeviceKey) -> String {
        let code = crypto::new_qr_code();
        let mut inner = self.lock();
        let now = Instant::now();
        inner
            .qr_codes
            .retain(|(_, t)| now.duration_since(*t) < QR_CODE_TTL);
        if inner.qr_codes.len() >= MAX_QR_CODES {
            inner.qr_codes.remove(0);
        }
        inner.qr_codes.push((code.clone(), now));
        QrPayload {
            id: me.id.clone(),
            fingerprint: key.fingerprint(),
            code,
        }
        .encode()
    }

    /// The QR code was closed: its code is no longer accepted.
    pub fn end_qr(&self) {
        self.lock().qr_codes.clear();
    }

    /// Consumes a live one-time code (constant-time compare). Used by the receiver when a
    /// scanner says "verified" (spec §7.7).
    pub(crate) fn take_qr_code(&self, code: &str) -> bool {
        let mut inner = self.lock();
        let now = Instant::now();
        inner
            .qr_codes
            .retain(|(_, t)| now.duration_since(*t) < QR_CODE_TTL);
        let found = inner
            .qr_codes
            .iter()
            .position(|(c, _)| crate::protocol::tokens_equal(c, code));
        if let Some(i) = found {
            inner.qr_codes.remove(i);
        }
        found.is_some()
    }

    /// Scanner side, local part of spec §7.7: the scanned fingerprint must equal the key
    /// this device knows for that id — from `live` (the session being shown, e.g. the
    /// accept dialog) when its id matches, else the pinned one. On a match the key is
    /// marked verified and the payload is returned, so the caller can tell the other side
    /// (`transfer::confirm_qr`). Mismatch → [`Error::VerificationFailed`].
    pub fn check_qr(
        &self,
        payload: &str,
        live: Option<&PeerKey>,
    ) -> Result<(QrPayload, PeerKey), Error> {
        let qr = QrPayload::parse(payload)
            .ok_or_else(|| Error::Input("not a Hoppy verification code".into()))?;
        let known = match live.filter(|p| p.id == qr.id) {
            Some(p) => p.clone(),
            None => {
                let e = self.get(&qr.id).ok_or_else(|| {
                    Error::VerificationFailed(
                        "unknown device: exchange a file with it first, then scan again".into(),
                    )
                })?;
                PeerKey {
                    id: qr.id.clone(),
                    name: e.name,
                    platform: String::new(),
                    fingerprint: e.fingerprint,
                }
            }
        };
        if known.fingerprint != qr.fingerprint {
            return Err(Error::VerificationFailed(format!(
                "the code does not match the key of {} (possible interception)",
                known.name
            )));
        }
        self.mark_verified(&known)?;
        Ok((qr, known))
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
        one.pin(&peer("a", "FP"));
        assert_eq!(
            two.state("a", "FP"),
            TrustState::Known,
            "re-read before use"
        );
        two.mark_verified(&peer("a", "FP")).unwrap();
        assert_eq!(one.state("a", "FP"), TrustState::Verified);
        assert!(one.forget("a").unwrap());
        assert_eq!(two.state("a", "FP"), TrustState::New);
        fs::write(&path, "not json").unwrap();
        assert!(Trust::open(&path).is_err());
    }

    #[test]
    fn qr_codes_are_one_time() {
        let trust = Trust::in_memory(TrustStore::new());
        let key = DeviceKey::generate();
        let me = DeviceInfo {
            id: "pc".into(),
            name: "PC".into(),
            platform: "windows".into(),
        };
        let payload = trust.begin_qr(&me, &key);
        let qr = QrPayload::parse(&payload).unwrap();
        assert_eq!(qr.fingerprint, key.fingerprint());
        assert!(!trust.take_qr_code(&"0".repeat(32)));
        assert!(trust.take_qr_code(&qr.code));
        assert!(!trust.take_qr_code(&qr.code), "used once");
        let again = QrPayload::parse(&trust.begin_qr(&me, &key)).unwrap();
        trust.end_qr();
        assert!(!trust.take_qr_code(&again.code), "closed");
    }

    #[test]
    fn scanner_checks_fingerprint() {
        let trust = Trust::in_memory(TrustStore::new());
        let key = DeviceKey::generate();
        let pc = DeviceInfo {
            id: "pc".into(),
            name: "PC".into(),
            platform: "windows".into(),
        };
        let shown = Trust::in_memory(TrustStore::new()).begin_qr(&pc, &key);
        // Never seen and no live session: cannot verify.
        assert!(matches!(
            trust.check_qr(&shown, None),
            Err(Error::VerificationFailed(_))
        ));
        // A live session with another key (man in the middle): refused, nothing stored.
        let mitm = PeerKey::new(&pc, DeviceKey::generate().fingerprint());
        assert!(matches!(
            trust.check_qr(&shown, Some(&mitm)),
            Err(Error::VerificationFailed(_))
        ));
        assert_eq!(trust.get("pc"), None);
        // The real key, from the live session: verified and pinned.
        let real = PeerKey::new(&pc, key.fingerprint());
        let (qr, who) = trust.check_qr(&shown, Some(&real)).unwrap();
        assert_eq!((qr.id.as_str(), who), ("pc", real.clone()));
        assert_eq!(trust.state("pc", &key.fingerprint()), TrustState::Verified);
        // Later, from the store alone.
        assert!(trust.check_qr(&shown, None).is_ok());
        assert!(matches!(
            trust.check_qr("hello", None),
            Err(Error::Input(_))
        ));
    }
}
