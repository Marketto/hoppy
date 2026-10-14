//! Wire format of protocol v0. Spec: `docs/protocol.md`. Keep in lockstep with
//! `apps/android/.../net/Protocol.kt` and `tools/hoppy_ref.py`.

use std::io::{Read, Write};
use std::net::Ipv4Addr;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::Error;

pub const MAGIC: [u8; 4] = *b"HOPY";
pub const VERSION: u8 = 0;
pub const DISCOVERY_PORT: u16 = 47470;
pub const TRANSFER_PORT: u16 = 47471;
pub const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 72, 80);
pub const MAX_JSON_FRAME: u32 = 1 << 20;
pub const MAX_FILES: usize = 10_000;
pub const HASH_LEN: usize = 32;

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
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("discovery message always serializes")
    }

    /// Returns `None` for anything that is not a valid v0 message (spec §2 rule 5).
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let msg: Self = serde_json::from_slice(bytes).ok()?;
        (msg.hoppy == VERSION && !msg.id.is_empty()).then_some(msg)
    }

    pub fn device(&self) -> DeviceInfo {
        DeviceInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            platform: self.platform.clone(),
        }
    }
}

pub fn write_hello<W: Write>(w: &mut W) -> Result<(), Error> {
    w.write_all(&MAGIC)?;
    w.write_all(&[VERSION])?;
    Ok(())
}

pub fn read_hello<R: Read>(r: &mut R) -> Result<(), Error> {
    let mut buf = [0u8; 5];
    r.read_exact(&mut buf)?;
    if buf[..4] != MAGIC {
        return Err(Error::Protocol("bad magic".into()));
    }
    if buf[4] != VERSION {
        return Err(Error::Protocol(format!("unsupported version {}", buf[4])));
    }
    Ok(())
}

pub fn write_json<W: Write, T: Serialize>(w: &mut W, value: &T) -> Result<(), Error> {
    let body = serde_json::to_vec(value).map_err(|e| Error::Protocol(e.to_string()))?;
    let len = u32::try_from(body.len()).map_err(|_| Error::Protocol("frame too large".into()))?;
    if len > MAX_JSON_FRAME {
        return Err(Error::Protocol("frame too large".into()));
    }
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&body)?;
    Ok(())
}

pub fn read_json<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<T, Error> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len);
    if len > MAX_JSON_FRAME {
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
        assert!(read_hello(&mut ok.as_slice()).is_ok());
        assert!(read_hello(&mut &b"HOPX\0"[..]).is_err());
        assert!(read_hello(&mut &b"HOPY\x01"[..]).is_err());
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
    }

    #[test]
    fn decision_omits_empty_reason() {
        assert_eq!(
            serde_json::to_string(&Decision::accept()).unwrap(),
            r#"{"accept":true}"#
        );
    }
}
