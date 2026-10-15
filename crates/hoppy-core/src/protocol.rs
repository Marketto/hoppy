//! Wire format: v0 framing (spec §1–6) and the v1 additions (hello version 1, `crypto`
//! fields, spec §7). Spec: `docs/protocol.md`. Keep in lockstep with
//! `apps/android/.../net/Protocol.kt` and `tools/hoppy_ref.py`.

use std::io::{Read, Write};
use std::net::Ipv4Addr;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::Error;

pub const MAGIC: [u8; 4] = *b"HOPY";
/// Magic of the callback hello sent by a receiver that connects back (spec §6).
pub const CALLBACK_MAGIC: [u8; 4] = *b"HOPC";
/// Version of discovery datagrams (`"hoppy":0`, unchanged in v1: additive fields only) and
/// hello byte of an unencrypted (v0) session.
pub const VERSION: u8 = 0;
/// Hello byte of an encrypted session (`HOPY 0x01`, spec §7.2).
pub const VERSION_V1: u8 = 1;
/// Value of the `crypto` field in discovery datagrams and callback hellos of v1 nodes.
pub const CRYPTO_VERSION: u8 = 1;
pub const DISCOVERY_PORT: u16 = 47470;
pub const TRANSFER_PORT: u16 = 47471;
pub const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 72, 80);
pub const MAX_JSON_FRAME: u32 = 1 << 20;
/// Max JSON frame of the callback hello (spec §6); the real one is under 200 bytes.
pub const MAX_CALLBACK_HELLO_FRAME: u32 = 1024;
pub const MAX_FILES: usize = 10_000;
pub const HASH_LEN: usize = 32;
/// Callback token: 128 random bits as 32 lowercase hex characters (spec §6).
pub const TOKEN_HEX_LEN: usize = 32;

/// Identity of a device as shown to other devices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
}

impl DeviceInfo {
    /// Platform string for the current build target.
    pub fn current_platform() -> &'static str {
        if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "android") {
            "android"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "ios") {
            "ios"
        } else {
            "linux"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub sender: DeviceInfo,
    pub files: Vec<FileEntry>,
}

impl Offer {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub accept: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Decision {
    pub fn accept() -> Self {
        Self {
            accept: true,
            reason: None,
        }
    }
    pub fn reject(reason: &str) -> Self {
        Self {
            accept: false,
            reason: Some(reason.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Discovery message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    Announce,
    Query,
    Bye,
    /// "Please connect back to me" (spec §6). `port` is the sender's callback listener.
    Callback,
}

/// One discovery datagram.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryMessage {
    pub hoppy: u8,
    #[serde(rename = "type")]
    pub kind: MessageKind,
    pub id: String,
    pub name: String,
    pub platform: String,
    pub port: u16,
    /// `callback` only: one-time token the receiver echoes in the callback hello.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// `callback` only: id of the receiver that should connect back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// v1 nodes: `1` = speaks the encrypted session (spec §7.1). Absent on v0 nodes.
    /// Parsed leniently: a value of another type never makes the datagram invalid.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_crypto"
    )]
    pub crypto: Option<u8>,
}

impl DiscoveryMessage {
    pub fn new(kind: MessageKind, me: &DeviceInfo, port: u16) -> Self {
        Self {
            hoppy: VERSION,
            kind,
            id: me.id.clone(),
            name: me.name.clone(),
            platform: me.platform.clone(),
            port,
            token: None,
            to: None,
            crypto: Some(CRYPTO_VERSION),
        }
    }

    /// A `callback` datagram asking peer `to` to connect to our `port` (spec §6).
    pub fn callback(me: &DeviceInfo, port: u16, token: &str, to: &str) -> Self {
        Self {
            token: Some(token.to_string()),
            to: Some(to.to_string()),
            ..Self::new(MessageKind::Callback, me, port)
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("discovery message always serializes")
    }

    /// Returns `None` for anything that is not a valid v0 message (spec §2 rule 5).
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let msg: Self = serde_json::from_slice(bytes).ok()?;
        if msg.hoppy != VERSION || msg.id.is_empty() {
            return None;
        }
        if msg.kind == MessageKind::Callback {
            let token_ok = msg.token.as_deref().is_some_and(is_valid_token);
            let to_ok = msg.to.as_deref().is_some_and(|t| !t.is_empty());
            if !token_ok || !to_ok || msg.port == 0 {
                return None;
            }
        }
        Some(msg)
    }

    pub fn device(&self) -> DeviceInfo {
        DeviceInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            platform: self.platform.clone(),
        }
    }
}

/// `crypto` (spec §7.1): a non-negative integer (capped at 255); anything else counts as
/// absent instead of rejecting the whole message (a future version may send more).
fn lenient_crypto<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u8>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_u64().map(|n| n.min(255) as u8))
}

/// Spec §6: exactly 32 lowercase hex characters.
pub fn is_valid_token(token: &str) -> bool {
    token.len() == TOKEN_HEX_LEN
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A fresh callback token from the OS CSPRNG.
pub fn new_token() -> String {
    let mut bytes = [0u8; TOKEN_HEX_LEN / 2];
    getrandom::fill(&mut bytes).expect("OS random number generator unavailable");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares tokens without an early exit, so timing does not leak a prefix match.
pub fn tokens_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// JSON frame that follows `HOPC 0x00` (spec §6). Field order is part of the vectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallbackHello {
    /// Id of the receiver that is connecting back.
    pub id: String,
    pub token: String,
    /// v1 receivers: `1` (spec §7.1). Absent from v0 receivers. Parsed leniently.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_crypto"
    )]
    pub crypto: Option<u8>,
}

/// Written by the receiver right after it connects back to a callback listener.
pub fn write_callback_hello<W: Write>(w: &mut W, hello: &CallbackHello) -> Result<(), Error> {
    w.write_all(&CALLBACK_MAGIC)?;
    w.write_all(&[VERSION])?;
    write_json(w, hello)
}

/// Read by the sender on its callback listener. Does not check the token. The frame is
/// capped at [`MAX_CALLBACK_HELLO_FRAME`]; the caller bounds the time (spec §6 step 3).
pub fn read_callback_hello<R: Read>(r: &mut R) -> Result<CallbackHello, Error> {
    let mut buf = [0u8; 5];
    r.read_exact(&mut buf)?;
    if buf[..4] != CALLBACK_MAGIC {
        return Err(Error::Protocol("bad callback magic".into()));
    }
    if buf[4] != VERSION {
        return Err(Error::Protocol(format!("unsupported version {}", buf[4])));
    }
    read_json_limited(r, MAX_CALLBACK_HELLO_FRAME)
}

pub fn write_hello<W: Write>(w: &mut W) -> Result<(), Error> {
    w.write_all(&MAGIC)?;
    w.write_all(&[VERSION])?;
    Ok(())
}

/// `HOPY 0x01`: an encrypted (v1) session follows (spec §7.2).
pub fn write_hello_v1<W: Write>(w: &mut W) -> Result<(), Error> {
    w.write_all(&MAGIC)?;
    w.write_all(&[VERSION_V1])?;
    Ok(())
}

/// Reads the session hello; returns its version ([`VERSION`] or [`VERSION_V1`]).
pub fn read_hello<R: Read>(r: &mut R) -> Result<u8, Error> {
    let mut buf = [0u8; 5];
    r.read_exact(&mut buf)?;
    if buf[..4] != MAGIC {
        return Err(Error::Protocol("bad magic".into()));
    }
    if buf[4] != VERSION && buf[4] != VERSION_V1 {
        return Err(Error::Protocol(format!("unsupported version {}", buf[4])));
    }
    Ok(buf[4])
}

pub fn write_json<W: Write + ?Sized, T: Serialize>(w: &mut W, value: &T) -> Result<(), Error> {
    let body = serde_json::to_vec(value).map_err(|e| Error::Protocol(e.to_string()))?;
    let len = u32::try_from(body.len()).map_err(|_| Error::Protocol("frame too large".into()))?;
    if len > MAX_JSON_FRAME {
        return Err(Error::Protocol("frame too large".into()));
    }
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&body)?;
    Ok(())
}

pub fn read_json<R: Read + ?Sized, T: DeserializeOwned>(r: &mut R) -> Result<T, Error> {
    read_json_limited(r, MAX_JSON_FRAME)
}

/// [`read_json`] with a smaller frame cap (checked before allocating).
pub fn read_json_limited<R: Read + ?Sized, T: DeserializeOwned>(
    r: &mut R,
    max: u32,
) -> Result<T, Error> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len);
    if len > max.min(MAX_JSON_FRAME) {
        return Err(Error::Protocol(format!(
            "frame of {len} bytes exceeds limit"
        )));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|e| Error::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> DeviceInfo {
        DeviceInfo {
            id: "id-1".into(),
            name: "PC".into(),
            platform: "windows".into(),
        }
    }

    #[test]
    fn json_frame_roundtrip() {
        let offer = Offer {
            sender: me(),
            files: vec![FileEntry {
                name: "a.txt".into(),
                size: 5,
            }],
        };
        let mut buf = Vec::new();
        write_json(&mut buf, &offer).unwrap();
        assert_eq!(
            u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize,
            buf.len() - 4
        );
        let back: Offer = read_json(&mut buf.as_slice()).unwrap();
        assert_eq!(back, offer);
    }

    #[test]
    fn rejects_oversized_frame() {
        let mut buf = (MAX_JSON_FRAME + 1).to_be_bytes().to_vec();
        buf.extend_from_slice(b"{}");
        assert!(read_json::<_, Decision>(&mut buf.as_slice()).is_err());
    }

    #[test]
    fn hello_checks_magic_and_version() {
        let mut ok = Vec::new();
        write_hello(&mut ok).unwrap();
        assert_eq!(ok, b"HOPY\0");
        assert_eq!(read_hello(&mut ok.as_slice()).unwrap(), VERSION);
        let mut v1 = Vec::new();
        write_hello_v1(&mut v1).unwrap();
        assert_eq!(v1, b"HOPY\x01");
        assert_eq!(read_hello(&mut v1.as_slice()).unwrap(), VERSION_V1);
        assert!(read_hello(&mut &b"HOPX\0"[..]).is_err());
        assert!(read_hello(&mut &b"HOPY\x02"[..]).is_err());
    }

    #[test]
    fn discovery_ignores_unknown_and_foreign() {
        let msg = DiscoveryMessage::new(MessageKind::Announce, &me(), 47471);
        assert_eq!(DiscoveryMessage::decode(&msg.encode()), Some(msg));
        let extra = br#"{"hoppy":0,"type":"announce","id":"x","name":"n","platform":"linux","port":1,"future":true}"#;
        assert!(DiscoveryMessage::decode(extra).is_some());
        assert!(
            DiscoveryMessage::decode(
                br#"{"hoppy":1,"type":"announce","id":"x","name":"n","platform":"linux","port":1}"#
            )
            .is_none()
        );
        assert!(
            DiscoveryMessage::decode(
                br#"{"hoppy":0,"type":"wave","id":"x","name":"n","platform":"linux","port":1}"#
            )
            .is_none()
        );
        assert!(DiscoveryMessage::decode(b"garbage").is_none());
        // `crypto` of an unexpected type or size never hides the peer.
        for (raw, want) in [
            (r#""crypto":"1""#, None),
            (r#""crypto":-1"#, None),
            (r#""crypto":1.5"#, None),
            (r#""crypto":null"#, None),
            (r#""crypto":{"v":2}"#, None),
            (r#""crypto":300"#, Some(255)),
            (r#""crypto":2"#, Some(2)),
        ] {
            let text = format!(
                r#"{{"hoppy":0,"type":"announce","id":"x","name":"n","platform":"linux","port":1,{raw}}}"#
            );
            let msg = DiscoveryMessage::decode(text.as_bytes()).expect(raw);
            assert_eq!(msg.crypto, want, "{raw}");
        }
    }

    fn vectors() -> serde_json::Value {
        serde_json::from_str(include_str!("../../../docs/test-vectors.json")).unwrap()
    }

    #[test]
    fn callback_datagram_matches_vectors() {
        let v = &vectors()["callback_example"];
        let raw = v["datagram_json"].as_str().unwrap();
        let msg = DiscoveryMessage::decode(raw.as_bytes()).expect("valid callback");
        assert_eq!(msg.kind, MessageKind::Callback);
        assert_eq!(msg.port, 50123);
        assert_eq!(msg.crypto, None, "v0 datagram");
        let me = msg.device();
        let ours = DiscoveryMessage {
            crypto: None,
            ..DiscoveryMessage::callback(
                &me,
                msg.port,
                msg.token.as_deref().unwrap(),
                msg.to.as_deref().unwrap(),
            )
        };
        assert_eq!(ours, msg);
        assert_eq!(String::from_utf8(ours.encode()).unwrap(), raw);
        for bad in v["invalid_datagrams"].as_array().unwrap() {
            let bad = bad.as_str().unwrap();
            assert!(DiscoveryMessage::decode(bad.as_bytes()).is_none(), "{bad}");
        }
        // Announces carry no token/to fields on the wire, and v1 adds `crypto` only.
        let a = DiscoveryMessage::new(MessageKind::Announce, &me, 1).encode();
        assert!(!String::from_utf8(a).unwrap().contains("token"));
        let v1 = &vectors()["v1"]["discovery"];
        let raw = v1["announce_json"].as_str().unwrap();
        let msg = DiscoveryMessage::decode(raw.as_bytes()).expect("v1 announce");
        assert_eq!(msg.crypto, Some(CRYPTO_VERSION));
        let ours = DiscoveryMessage::new(MessageKind::Announce, &msg.device(), msg.port);
        assert_eq!(String::from_utf8(ours.encode()).unwrap(), raw);
        let raw = v1["callback_json"].as_str().unwrap();
        let msg = DiscoveryMessage::decode(raw.as_bytes()).expect("v1 callback");
        let ours = DiscoveryMessage::callback(
            &msg.device(),
            msg.port,
            msg.token.as_deref().unwrap(),
            msg.to.as_deref().unwrap(),
        );
        assert_eq!(String::from_utf8(ours.encode()).unwrap(), raw);
    }

    #[test]
    fn callback_hello_matches_vector_bytes() {
        let v = &vectors()["callback_example"];
        let hello: CallbackHello = serde_json::from_str(v["hello_json"].as_str().unwrap()).unwrap();
        let mut buf = Vec::new();
        write_callback_hello(&mut buf, &hello).unwrap();
        let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, v["hello_hex"].as_str().unwrap());
        assert_eq!(read_callback_hello(&mut buf.as_slice()).unwrap(), hello);
        assert_eq!(hello.crypto, None, "v0 hello");
        let v1: CallbackHello = serde_json::from_str(
            vectors()["v1"]["discovery"]["callback_hello_json"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v1.crypto, Some(CRYPTO_VERSION));
        let mut buf = Vec::new();
        write_callback_hello(&mut buf, &v1).unwrap();
        let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            vectors()["v1"]["discovery"]["callback_hello_hex"]
                .as_str()
                .unwrap()
        );
        assert!(read_callback_hello(&mut &b"HOPY\0\0\0\0\x02{}"[..]).is_err());
        // Over 1 KiB (checked before reading the body) and non-object JSON are rejected.
        let mut big = b"HOPC\0".to_vec();
        big.extend_from_slice(&(MAX_CALLBACK_HELLO_FRAME + 1).to_be_bytes());
        assert!(read_callback_hello(&mut big.as_slice()).is_err());
        for body in [&b"[1,2]"[..], b"\"x\"", b"{\"id\":1,\"token\":\"t\"}"] {
            let mut frame = b"HOPC\0".to_vec();
            frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
            frame.extend_from_slice(body);
            assert!(read_callback_hello(&mut frame.as_slice()).is_err());
        }
    }

    #[test]
    fn tokens() {
        let t = new_token();
        assert!(is_valid_token(&t));
        assert_ne!(t, new_token());
        assert!(!is_valid_token(
            &t.to_uppercase().replace(char::is_numeric, "A")
        ));
        assert!(!is_valid_token(&t[1..]));
        assert!(tokens_equal(&t, &t.clone()));
        assert!(!tokens_equal(&t, &new_token()));
        assert!(!tokens_equal(&t, &t[1..]));
    }

    #[test]
    fn decision_omits_empty_reason() {
        assert_eq!(
            serde_json::to_string(&Decision::accept()).unwrap(),
            r#"{"accept":true}"#
        );
    }
}
