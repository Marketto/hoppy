//! Protocol v1 building blocks (spec §7, ADR 0010): the device's static X25519 key, key
//! fingerprints and the short authentication string (SAS: 4 emoji). The handshake itself lives in `session.rs`, the trust
//! store in `trust.rs`.
//!
//! Every value here has a vector in `docs/test-vectors.json` (`v1`), shared with the
//! Python reference (`tools/hoppy_ref.py`) and the Android core.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use curve25519_dalek::montgomery::MontgomeryPoint;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::Error;

/// Length of an unpadded RFC 4648 base32 SHA-256 (the whole hash, 256 bits → 52 chars).
pub const FINGERPRINT_LEN: usize = 52;
/// Longest device id accepted in a handshake (bytes).
pub const MAX_ID_BYTES: usize = 128;

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const SAS_LABEL: &[u8] = b"hoppy-sas-v1";
pub(crate) const COMMIT_LABEL: &[u8] = b"hoppy-commit-v1";

/// The device's long-term X25519 key pair (its identity in v1). Generated once on first run
/// and stored privately by the platform ([`DeviceKey::load_or_create`] on desktops). Not
/// `Clone` (share it through an `Arc`); the private key is wiped from memory on drop.
pub struct DeviceKey {
    secret: [u8; 32],
    public: [u8; 32],
}

impl Drop for DeviceKey {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl std::fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceKey")
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

/// On-disk form: `{"version":1,"x25519_private":"<64 hex>"}`.
#[derive(Serialize, Deserialize)]
struct KeyFile {
    version: u8,
    x25519_private: String,
}

impl DeviceKey {
    /// A fresh key from the OS CSPRNG.
    pub fn generate() -> Self {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).expect("OS random number generator unavailable");
        Self::from_secret(secret)
    }

    /// The key pair for a stored private key (RFC 7748 clamping is applied when used).
    pub fn from_secret(secret: [u8; 32]) -> Self {
        let public = MontgomeryPoint::mul_base_clamped(secret).to_bytes();
        Self { secret, public }
    }

    pub fn public(&self) -> [u8; 32] {
        self.public
    }

    pub(crate) fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    /// [`fingerprint`] of the public key.
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.public)
    }

    /// The key file content (holds the private key: wiped when dropped).
    pub fn to_json(&self) -> Zeroizing<String> {
        let mut file = KeyFile {
            version: 1,
            x25519_private: hex(&self.secret),
        };
        let text =
            Zeroizing::new(serde_json::to_string(&file).expect("key file always serializes"));
        file.x25519_private.zeroize();
        text
    }

    /// Parses a key file. A file of another `version` is an error (a newer app wrote it:
    /// never overwrite it).
    pub fn from_json(text: &str) -> Result<Self, Error> {
        let mut file: KeyFile =
            serde_json::from_str(text).map_err(|e| Error::Input(format!("key file: {e}")))?;
        if file.version != 1 {
            file.x25519_private.zeroize();
            return Err(Error::Unsupported(format!(
                "key file version {} (this app knows 1): update Hoppy",
                file.version
            )));
        }
        let bytes = Zeroizing::new(unhex(&file.x25519_private).unwrap_or_default());
        file.x25519_private.zeroize();
        if bytes.len() != 32 {
            return Err(Error::Input("key file: invalid private key".into()));
        }
        let mut secret = [0u8; 32];
        secret.copy_from_slice(&bytes);
        let key = Self::from_secret(secret);
        secret.zeroize();
        Ok(key)
    }

    /// Loads the key from `path`, or creates it (owner-only on Unix; the caller restricts it
    /// on Windows). Two processes starting at once end up with the same key: the file is
    /// published with an exclusive link, and the loser loads the winner's key. A file that
    /// is not JSON is kept as `<path>.bad` and replaced (peers then see a changed key);
    /// a key file of an unknown version is an error.
    pub fn load_or_create(path: &Path) -> Result<Self, Error> {
        match fs::read_to_string(path).map(Zeroizing::new) {
            Ok(text) => match Self::from_json(&text) {
                Ok(key) => return Ok(key),
                Err(e @ Error::Unsupported(_)) => return Err(e),
                Err(_) => {
                    let _ = fs::rename(path, path.with_extension("bad"));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let key = Self::generate();
        let tmp = temp_path(path);
        write_owner_only(&tmp, key.to_json().as_bytes())?;
        // hard_link fails if `path` exists: never replaces a key another process just wrote.
        let linked = fs::hard_link(&tmp, path);
        let _ = fs::remove_file(&tmp);
        match linked {
            Ok(()) => Ok(key),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let text = Zeroizing::new(fs::read_to_string(path)?);
                Self::from_json(&text)
            }
            // File systems without hard links: exclusive create.
            Err(_) => {
                let mut opts = owner_only_options();
                opts.create_new(true);
                match opts.open(path) {
                    Ok(mut f) => {
                        f.write_all(key.to_json().as_bytes())?;
                        f.sync_all()?;
                        Ok(key)
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        let text = Zeroizing::new(fs::read_to_string(path)?);
                        Self::from_json(&text)
                    }
                    Err(e) => Err(e.into()),
                }
            }
        }
    }
}

/// A temp file next to `path`, unique per process and call (no two writers share one).
fn temp_path(path: &Path) -> std::path::PathBuf {
    let mut r = [0u8; 8];
    getrandom::fill(&mut r).expect("OS random number generator unavailable");
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.{}.{}.tmp", std::process::id(), hex(&r)))
}

fn owner_only_options() -> OpenOptions {
    let mut opts = OpenOptions::new();
    opts.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts
}

/// Creates `path` (must not exist) owner-only and writes `data`.
fn write_owner_only(path: &Path, data: &[u8]) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut opts = owner_only_options();
    opts.create_new(true);
    let mut file = opts.open(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    Ok(())
}

/// Writes `data` atomically (unique temp file created owner-only on Unix, then rename).
pub fn write_private(path: &Path, data: &[u8]) -> Result<(), Error> {
    let tmp = temp_path(path);
    if let Err(e) = write_owner_only(&tmp, data).and_then(|()| Ok(fs::rename(&tmp, path)?)) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// `true` for an X25519 public key of small order (spec §7.3): multiplying by any clamped
/// scalar (a multiple of the cofactor 8) gives the all-zero point. Such keys would make a
/// Diffie-Hellman output predictable; v1 handshakes reject them.
pub fn is_low_order(public: &[u8]) -> bool {
    let Ok(bytes) = <[u8; 32]>::try_from(public) else {
        return true;
    };
    // Any fixed scalar works; clamping makes it a multiple of 8.
    MontgomeryPoint(bytes).mul_clamped([0x5a; 32]).to_bytes() == [0u8; 32]
}

/// Spec §7.5: unpadded RFC 4648 base32 (upper case) of SHA-256(static public key), 52 chars.
pub fn fingerprint(public: &[u8]) -> String {
    base32(&Sha256::digest(public))
}

/// `true` for a well-formed fingerprint (52 chars of the base32 alphabet, upper case).
pub fn is_fingerprint(s: &str) -> bool {
    s.len() == FINGERPRINT_LEN && s.bytes().all(|b| BASE32.contains(&b))
}

/// RFC 4648 base32, upper case, no padding.
pub fn base32(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub(crate) fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lower-case hex only (what every implementation writes).
pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2)
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// The 32-byte value of a 64-character lower-case hex string.
pub(crate) fn unhex32(s: &str) -> Option<[u8; 32]> {
    let v = unhex(s)?;
    v.try_into().ok()
}

/// Commitment to the initiator's SAS nonce (spec §7.3): SHA-256("hoppy-commit-v1" ‖ nonce).
pub(crate) fn commit(nonce: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(COMMIT_LABEL);
    h.update(nonce);
    h.finalize().into()
}

/// One entry of the SAS emoji table (spec §7.6): the emoji (one code point, default emoji
/// presentation, Unicode ≤ 9.0) and its Italian and English names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SasEmoji {
    pub emoji: &'static str,
    pub it: &'static str,
    pub en: &'static str,
}

const fn e(emoji: &'static str, it: &'static str, en: &'static str) -> SasEmoji {
    SasEmoji { emoji, it, en }
}

/// The 64 SAS emoji, index = 6-bit group value. Source of truth: `docs/protocol.md` §7.6 and
/// `docs/test-vectors.json` (`v1.sas_emoji`); a test keeps the three equal.
pub const SAS_EMOJI: [SasEmoji; 64] = [
    e("🐘", "elefante", "elephant"),
    e("🐢", "tartaruga", "turtle"),
    e("🐙", "polpo", "octopus"),
    e("🦋", "farfalla", "butterfly"),
    e("🐌", "lumaca", "snail"),
    e("🐝", "ape", "bee"),
    e("🐧", "pinguino", "penguin"),
    e("🐳", "balena", "whale"),
    e("🦀", "granchio", "crab"),
    e("🐍", "serpente", "snake"),
    e("🦉", "gufo", "owl"),
    e("🐪", "cammello", "camel"),
    e("🐇", "coniglio", "rabbit"),
    e("🐓", "gallo", "rooster"),
    e("🐞", "coccinella", "ladybug"),
    e("🐖", "maiale", "pig"),
    e("🐒", "scimmia", "monkey"),
    e("🍎", "mela", "apple"),
    e("🍌", "banana", "banana"),
    e("🍇", "uva", "grapes"),
    e("🥜", "arachidi", "peanuts"),
    e("🍋", "limone", "lemon"),
    e("🍍", "ananas", "pineapple"),
    e("🍿", "popcorn", "popcorn"),
    e("🥕", "carota", "carrot"),
    e("☕", "caffè", "coffee"),
    e("🍄", "fungo", "mushroom"),
    e("🥑", "avocado", "avocado"),
    e("🍕", "pizza", "pizza"),
    e("🍔", "hamburger", "hamburger"),
    e("🍩", "ciambella", "doughnut"),
    e("🍦", "gelato", "ice cream"),
    e("🎂", "torta", "cake"),
    e("🎲", "dado", "dice"),
    e("🎁", "regalo", "gift"),
    e("🔑", "chiave", "key"),
    e("🔔", "campana", "bell"),
    e("🎸", "chitarra", "guitar"),
    e("🎺", "tromba", "trumpet"),
    e("⚽", "pallone", "ball"),
    e("🚲", "bicicletta", "bicycle"),
    e("🚀", "razzo", "rocket"),
    e("⛵", "barca a vela", "sailboat"),
    e("⌛", "clessidra", "hourglass"),
    e("💡", "lampadina", "light bulb"),
    e("📷", "fotocamera", "camera"),
    e("🔨", "martello", "hammer"),
    e("⚓", "àncora", "anchor"),
    e("👑", "corona", "crown"),
    e("🏠", "casa", "house"),
    e("🌵", "cactus", "cactus"),
    e("🌻", "girasole", "sunflower"),
    e("🍀", "quadrifoglio", "four-leaf clover"),
    e("🌳", "albero", "tree"),
    e("🌴", "palma", "palm tree"),
    e("🍁", "foglia d'acero", "maple leaf"),
    e("⛄", "pupazzo di neve", "snowman"),
    e("⭐", "stella", "star"),
    e("⚡", "fulmine", "lightning"),
    e("🌈", "arcobaleno", "rainbow"),
    e("🌊", "onda", "wave"),
    e("🔥", "fuoco", "fire"),
    e("🌍", "mappamondo", "globe"),
    e("🌋", "vulcano", "volcano"),
];

/// The short authentication string of a session (spec §7.6): a 24-bit value shown as
/// 4 emoji (6 bits each, most significant first); the only form shown to users.
/// Both ends of an honest session show the same value; a man in the middle cannot make
/// them match except by a 1 in 2^24 chance (commit/reveal, spec §7.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sas {
    pub value: u32,
    /// Indices into [`SAS_EMOJI`].
    pub emoji: [u8; 4],
    /// The 4 emoji as one string.
    pub text: String,
}

impl Sas {
    /// `value` is masked to 24 bits.
    pub fn from_value(value: u32) -> Self {
        let value = value & 0x00FF_FFFF;
        let emoji = [18, 12, 6, 0].map(|shift| ((value >> shift) & 63) as u8);
        let text = emoji
            .iter()
            .map(|&i| SAS_EMOJI[usize::from(i)].emoji)
            .collect();
        Self { value, emoji, text }
    }

    /// Spec §7.6: first 3 bytes (big-endian) of
    /// SHA-256("hoppy-sas-v1" ‖ h2 ‖ initiator nonce ‖ responder nonce), where `h2` is the
    /// Noise handshake hash right after message 2.
    pub fn derive(h2: &[u8], initiator_nonce: &[u8; 32], responder_nonce: &[u8; 32]) -> Self {
        let mut h = Sha256::new();
        h.update(SAS_LABEL);
        h.update(h2);
        h.update(initiator_nonce);
        h.update(responder_nonce);
        let d = h.finalize();
        Self::from_value(u32::from_be_bytes([0, d[0], d[1], d[2]]))
    }

    /// Emoji names for reading aloud / screen readers, in Italian or English.
    pub fn words(&self, italian: bool) -> Vec<&'static str> {
        self.emoji
            .iter()
            .map(|&i| {
                let e = &SAS_EMOJI[usize::from(i)];
                if italian { e.it } else { e.en }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v1() -> serde_json::Value {
        let all: serde_json::Value =
            serde_json::from_str(include_str!("../../../docs/test-vectors.json")).unwrap();
        all["v1"].clone()
    }

    #[test]
    fn emoji_table_matches_vectors_and_rules() {
        let table = v1()["sas_emoji"].as_array().unwrap().clone();
        assert_eq!(table.len(), 64);
        let mut seen = std::collections::HashSet::new();
        for (i, (row, ours)) in table.iter().zip(SAS_EMOJI.iter()).enumerate() {
            assert_eq!(row["index"].as_u64(), Some(i as u64));
            assert_eq!(row["emoji"].as_str(), Some(ours.emoji), "{i}");
            assert_eq!(row["it"].as_str(), Some(ours.it), "{i}");
            assert_eq!(row["en"].as_str(), Some(ours.en), "{i}");
            let cps: Vec<String> = ours
                .emoji
                .chars()
                .map(|c| format!("U+{:04X}", c as u32))
                .collect();
            assert_eq!(cps.len(), 1, "one code point, no ZWJ/VS/skin tone: {i}");
            assert_eq!(row["codepoints"].as_str(), Some(cps[0].as_str()), "{i}");
            assert!(seen.insert(ours.emoji), "duplicate {}", ours.emoji);
        }
        let it: std::collections::HashSet<_> = SAS_EMOJI.iter().map(|e| e.it).collect();
        let en: std::collections::HashSet<_> = SAS_EMOJI.iter().map(|e| e.en).collect();
        assert_eq!((it.len(), en.len()), (64, 64), "names are unique");
    }

    #[test]
    fn sas_rendering_matches_vectors() {
        for case in v1()["sas_values"].as_array().unwrap() {
            let sas = Sas::from_value(case["value"].as_u64().unwrap() as u32);
            let idx: Vec<u64> = case["emoji"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap())
                .collect();
            assert_eq!(sas.emoji.map(u64::from).to_vec(), idx);
            assert_eq!(sas.text, case["text"].as_str().unwrap());
        }
        assert_eq!(Sas::from_value(0x1FF_FFFF).value, 0xFF_FFFF);
        assert_eq!(Sas::from_value(0).words(true), ["elefante"; 4]);
    }

    #[test]
    fn fingerprints_and_keys_match_vectors() {
        let v = v1();
        for who in ["initiator", "responder"] {
            let k = &v["keys"][who];
            let secret = unhex32(k["static_private"].as_str().unwrap()).unwrap();
            let key = DeviceKey::from_secret(secret);
            assert_eq!(hex(&key.public()), k["static_public"].as_str().unwrap());
            assert_eq!(key.fingerprint(), k["fingerprint"].as_str().unwrap());
            assert!(is_fingerprint(&key.fingerprint()));
            let back = DeviceKey::from_json(&key.to_json()).unwrap();
            assert_eq!(back.public(), key.public());
            assert!(!is_low_order(&key.public()));
        }
        assert_eq!(base32(b""), "");
        assert_eq!(base32(b"f"), "MY");
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI"); // RFC 4648 §10, unpadded
        assert_eq!(fingerprint(b"").len(), FINGERPRINT_LEN);
    }

    #[test]
    fn key_file_is_private_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("identity.key");
        let a = DeviceKey::load_or_create(&path).unwrap();
        let b = DeviceKey::load_or_create(&path).unwrap();
        assert_eq!(a.public(), b.public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::write(&path, "garbage").unwrap();
        let c = DeviceKey::load_or_create(&path).unwrap();
        assert_ne!(c.public(), a.public());
        assert!(path.with_extension("bad").exists());
        // A newer key file is never overwritten.
        fs::write(&path, r#"{"version":2,"x25519_private":"00"}"#).unwrap();
        assert!(matches!(
            DeviceKey::load_or_create(&path),
            Err(Error::Unsupported(_))
        ));
        assert!(fs::read_to_string(&path).unwrap().contains("\"version\":2"));
        let leftovers = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0, "no temp files left");
    }

    #[test]
    fn concurrent_creation_yields_one_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.key");
        let keys: Vec<[u8; 32]> = (0..8)
            .map(|_| {
                let p = path.clone();
                std::thread::spawn(move || DeviceKey::load_or_create(&p).unwrap().public())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        assert!(
            keys.windows(2).all(|w| w[0] == w[1]),
            "everyone got the same key"
        );
    }

    #[test]
    fn low_order_points_are_rejected() {
        for p in v1()["negative"]["low_order_points"].as_array().unwrap() {
            let b = unhex(p.as_str().unwrap()).unwrap();
            assert!(is_low_order(&b), "{p}");
        }
        assert!(is_low_order(&[1u8; 31]), "wrong length");
    }
}
