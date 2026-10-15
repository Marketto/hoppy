//! Protocol v1 session layer (spec §7.2–7.5): the `Noise_XX_25519_ChaChaPoly_SHA256`
//! handshake with the SAS commit/reveal in its payloads, and the encrypted byte stream that
//! carries the unchanged v0 exchange (Offer, Decision, file bytes, hashes, Result).
//!
//! Wire framing: every Noise message (handshake or transport) is a big-endian `u16 length`
//! followed by `length` bytes. Transport messages carry at most [`MAX_PLAINTEXT`] bytes of
//! a byte stream; message boundaries mean nothing.

use std::io::{self, ErrorKind, Read, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use snow::{HandshakeState, StatelessTransportState};

use crate::Error;
use crate::crypto::{self, DeviceKey, MAX_ID_BYTES, Sas, hex, is_low_order, unhex32};
use crate::protocol::{DeviceInfo, MAGIC, VERSION_V1};
use crate::sanitize::display_text;

pub const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_SHA256";
/// Largest Noise message (spec, and the u16 length prefix).
pub const MAX_NOISE_MESSAGE: usize = 65535;
pub const TAG_LEN: usize = 16;
/// Largest plaintext in one transport message.
pub const MAX_PLAINTEXT: usize = MAX_NOISE_MESSAGE - TAG_LEN;
/// Receivers reject longer handshake messages (the real ones are < 400 bytes).
pub const MAX_HANDSHAKE_MESSAGE: usize = 2048;
/// Largest handshake payload (JSON) a v1 implementation writes.
pub const MAX_HANDSHAKE_PAYLOAD: usize = 1024;
/// Device names / platforms learned in a handshake are cut to this many characters.
pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_PLATFORM_CHARS: usize = 16;
const DH_LEN: usize = 32;
const MIN_MSG2: usize = DH_LEN + DH_LEN + TAG_LEN + TAG_LEN;
const MIN_MSG3: usize = DH_LEN + TAG_LEN + TAG_LEN;

/// Noise prologue: the 5 hello bytes, so a changed hello breaks the handshake.
pub fn prologue() -> [u8; 5] {
    let mut p = [0u8; 5];
    p[..4].copy_from_slice(&MAGIC);
    p[4] = VERSION_V1;
    p
}

fn snow_err(e: snow::Error) -> Error {
    Error::Crypto(e.to_string())
}

/// Payload of handshake message 1 (initiator → responder, in clear).
#[derive(Serialize, Deserialize)]
struct CommitPayload {
    commit: String,
}

/// Payload of messages 2 and 3 (encrypted): who I am + my SAS nonce. Field order is part
/// of the vectors (Rust and Python write it in this order; readers must not depend on it).
#[derive(Serialize, Deserialize)]
struct IdentityPayload {
    id: String,
    name: String,
    platform: String,
    nonce: String,
}

/// What a handshake learned about the other side.
#[derive(Debug, Clone)]
pub struct Remote {
    /// As the remote described itself, inside the encrypted handshake.
    pub device: DeviceInfo,
    /// Its static X25519 public key (authenticated by the handshake).
    pub public: [u8; 32],
    pub sas: Sas,
}

impl Remote {
    pub fn fingerprint(&self) -> String {
        crypto::fingerprint(&self.public)
    }
}

fn builder<'a>(
    key: &'a DeviceKey,
    prologue: &'a [u8],
    ephemeral: Option<&'a [u8; 32]>,
) -> Result<snow::Builder<'a>, Error> {
    let params = NOISE_PARAMS.parse().map_err(snow_err)?;
    let mut b = snow::Builder::new(params)
        .local_private_key(key.secret())
        .map_err(snow_err)?
        .prologue(prologue)
        .map_err(snow_err)?;
    if let Some(e) = ephemeral {
        b = b.fixed_ephemeral_key_for_testing_only(e);
    }
    Ok(b)
}

fn random_nonce() -> [u8; 32] {
    let mut n = [0u8; 32];
    getrandom::fill(&mut n).expect("OS random number generator unavailable");
    n
}

fn identity_payload(me: &DeviceInfo, nonce: &[u8; 32]) -> Result<Vec<u8>, Error> {
    let body = serde_json::to_vec(&IdentityPayload {
        id: me.id.clone(),
        name: me.name.clone(),
        platform: me.platform.clone(),
        nonce: hex(nonce),
    })
    .map_err(|e| Error::Protocol(e.to_string()))?;
    if body.len() > MAX_HANDSHAKE_PAYLOAD {
        return Err(Error::Input(
            "device name too long for the handshake".into(),
        ));
    }
    Ok(body)
}

/// Parses a message 2/3 payload. The id must be 1–128 bytes without control or bidi
/// characters (it is stored and shown); name and platform are cleaned and cut.
fn parse_identity(payload: &[u8]) -> Result<(DeviceInfo, [u8; 32]), Error> {
    let p: IdentityPayload = serde_json::from_slice(payload)
        .map_err(|e| Error::Protocol(format!("handshake payload: {e}")))?;
    let nonce = unhex32(&p.nonce).ok_or_else(|| Error::Protocol("bad handshake nonce".into()))?;
    if p.id.is_empty() || p.id.len() > MAX_ID_BYTES || display_text(&p.id, MAX_ID_BYTES) != p.id {
        return Err(Error::Protocol("bad device id in handshake".into()));
    }
    Ok((
        DeviceInfo {
            id: p.id,
            name: display_text(&p.name, MAX_NAME_CHARS),
            platform: display_text(&p.platform, MAX_PLATFORM_CHARS),
        },
        nonce,
    ))
}

fn reject_low_order(public: &[u8]) -> Result<(), Error> {
    if is_low_order(public) {
        return Err(Error::Crypto("low-order public key".into()));
    }
    Ok(())
}

fn remote_static(hs: &HandshakeState) -> Result<[u8; 32], Error> {
    let key = hs
        .get_remote_static()
        .and_then(|k| <[u8; 32]>::try_from(k).ok())
        .ok_or_else(|| Error::Crypto("no remote static key".into()))?;
    reject_low_order(&key)?;
    Ok(key)
}

/// Initiator (the sender, also on callback connections) of the v1 handshake.
pub struct Initiator {
    hs: HandshakeState,
    nonce: [u8; 32],
}

impl Initiator {
    pub fn new(key: &DeviceKey) -> Result<Self, Error> {
        Self::build(key, None, random_nonce())
    }

    /// Fixed ephemeral key and nonce: test vectors only.
    #[cfg(test)]
    pub(crate) fn with(
        key: &DeviceKey,
        ephemeral: Option<&[u8; 32]>,
        nonce: [u8; 32],
    ) -> Result<Self, Error> {
        Self::build(key, ephemeral, nonce)
    }

    fn build(
        key: &DeviceKey,
        ephemeral: Option<&[u8; 32]>,
        nonce: [u8; 32],
    ) -> Result<Self, Error> {
        let prologue = prologue();
        let hs = builder(key, &prologue, ephemeral)?
            .build_initiator()
            .map_err(snow_err)?;
        Ok(Self { hs, nonce })
    }

    /// `-> e` with payload `{"commit":hex(SHA-256("hoppy-commit-v1" ‖ nonce))}`.
    pub fn write_msg1(&mut self) -> Result<Vec<u8>, Error> {
        let payload = serde_json::to_vec(&CommitPayload {
            commit: hex(&crypto::commit(&self.nonce)),
        })
        .map_err(|e| Error::Protocol(e.to_string()))?;
        let mut out = vec![0u8; MAX_HANDSHAKE_MESSAGE];
        let n = self
            .hs
            .write_message(&payload, &mut out)
            .map_err(snow_err)?;
        out.truncate(n);
        Ok(out)
    }

    /// `<- e, ee, s, es` + the responder's identity and nonce. Fixes the SAS.
    pub fn read_msg2(&mut self, msg: &[u8]) -> Result<Remote, Error> {
        if msg.len() < MIN_MSG2 {
            return Err(Error::Crypto("handshake message 2 too short".into()));
        }
        reject_low_order(&msg[..DH_LEN])?;
        let mut payload = vec![0u8; MAX_HANDSHAKE_MESSAGE];
        let n = self.hs.read_message(msg, &mut payload).map_err(snow_err)?;
        let (device, responder_nonce) = parse_identity(&payload[..n])?;
        let h2 = self.hs.get_handshake_hash().to_vec();
        Ok(Remote {
            device,
            public: remote_static(&self.hs)?,
            sas: Sas::derive(&h2, &self.nonce, &responder_nonce),
        })
    }

    /// `-> s, se` + our identity and the revealed nonce.
    pub fn write_msg3(&mut self, me: &DeviceInfo) -> Result<Vec<u8>, Error> {
        let payload = identity_payload(me, &self.nonce)?;
        let mut out = vec![0u8; MAX_HANDSHAKE_MESSAGE];
        let n = self
            .hs
            .write_message(&payload, &mut out)
            .map_err(snow_err)?;
        out.truncate(n);
        Ok(out)
    }

    pub fn handshake_hash(&self) -> Vec<u8> {
        self.hs.get_handshake_hash().to_vec()
    }

    pub fn finish(self) -> Result<Cipher, Error> {
        Ok(Cipher(Arc::new(
            self.hs.into_stateless_transport_mode().map_err(snow_err)?,
        )))
    }
}

/// Responder (the receiver) of the v1 handshake.
pub struct Responder {
    hs: HandshakeState,
    nonce: [u8; 32],
    commit: Option<[u8; 32]>,
    h2: Vec<u8>,
}

impl Responder {
    pub fn new(key: &DeviceKey) -> Result<Self, Error> {
        Self::build(key, None, random_nonce())
    }

    /// Fixed ephemeral key and nonce: test vectors only.
    #[cfg(test)]
    pub(crate) fn with(
        key: &DeviceKey,
        ephemeral: Option<&[u8; 32]>,
        nonce: [u8; 32],
    ) -> Result<Self, Error> {
        Self::build(key, ephemeral, nonce)
    }

    fn build(
        key: &DeviceKey,
        ephemeral: Option<&[u8; 32]>,
        nonce: [u8; 32],
    ) -> Result<Self, Error> {
        let prologue = prologue();
        let hs = builder(key, &prologue, ephemeral)?
            .build_responder()
            .map_err(snow_err)?;
        Ok(Self {
            hs,
            nonce,
            commit: None,
            h2: Vec::new(),
        })
    }

    pub fn read_msg1(&mut self, msg: &[u8]) -> Result<(), Error> {
        if msg.len() < DH_LEN {
            return Err(Error::Crypto("handshake message 1 too short".into()));
        }
        reject_low_order(&msg[..DH_LEN])?;
        let mut payload = vec![0u8; MAX_HANDSHAKE_MESSAGE];
        let n = self.hs.read_message(msg, &mut payload).map_err(snow_err)?;
        let p: CommitPayload = serde_json::from_slice(&payload[..n])
            .map_err(|e| Error::Protocol(format!("handshake payload: {e}")))?;
        self.commit =
            Some(unhex32(&p.commit).ok_or_else(|| Error::Protocol("bad commitment".into()))?);
        Ok(())
    }

    pub fn write_msg2(&mut self, me: &DeviceInfo) -> Result<Vec<u8>, Error> {
        let payload = identity_payload(me, &self.nonce)?;
        let mut out = vec![0u8; MAX_HANDSHAKE_MESSAGE];
        let n = self
            .hs
            .write_message(&payload, &mut out)
            .map_err(snow_err)?;
        out.truncate(n);
        self.h2 = self.hs.get_handshake_hash().to_vec();
        Ok(out)
    }

    /// Reads the initiator's identity and checks its nonce against the commitment.
    pub fn read_msg3(&mut self, msg: &[u8]) -> Result<Remote, Error> {
        if msg.len() < MIN_MSG3 {
            return Err(Error::Crypto("handshake message 3 too short".into()));
        }
        let mut payload = vec![0u8; MAX_HANDSHAKE_MESSAGE];
        let n = self.hs.read_message(msg, &mut payload).map_err(snow_err)?;
        let (device, initiator_nonce) = parse_identity(&payload[..n])?;
        if Some(crypto::commit(&initiator_nonce)) != self.commit {
            return Err(Error::Crypto("SAS commitment mismatch".into()));
        }
        Ok(Remote {
            device,
            public: remote_static(&self.hs)?,
            sas: Sas::derive(&self.h2, &initiator_nonce, &self.nonce),
        })
    }

    pub fn handshake_hash(&self) -> Vec<u8> {
        self.hs.get_handshake_hash().to_vec()
    }

    pub fn finish(self) -> Result<Cipher, Error> {
        Ok(Cipher(Arc::new(
            self.hs.into_stateless_transport_mode().map_err(snow_err)?,
        )))
    }
}

/// Transport keys of an established session, shared by the reading and writing halves
/// (each keeps its own message counter).
#[derive(Clone)]
pub struct Cipher(Arc<StatelessTransportState>);

impl Cipher {
    /// Encrypts one transport message (no length prefix).
    pub fn seal(&self, nonce: u64, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
        let mut out = vec![0u8; plaintext.len() + TAG_LEN];
        let n = self
            .0
            .write_message(nonce, plaintext, &mut out)
            .map_err(snow_err)?;
        out.truncate(n);
        Ok(out)
    }

    pub fn open(&self, nonce: u64, message: &[u8]) -> Result<Vec<u8>, Error> {
        let mut out = vec![0u8; message.len()];
        let n = self
            .0
            .read_message(nonce, message, &mut out)
            .map_err(snow_err)?;
        out.truncate(n);
        Ok(out)
    }
}

/// Writes one framed Noise message: `u16 length` + bytes.
pub fn write_frame<W: Write + ?Sized>(w: &mut W, msg: &[u8]) -> io::Result<()> {
    let len = u16::try_from(msg.len()).map_err(|_| io::Error::from(ErrorKind::InvalidInput))?;
    let mut buf = Vec::with_capacity(2 + msg.len());
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(msg);
    w.write_all(&buf)
}

/// Reads one framed Noise message of at most `max` bytes. `Ok(None)` on a clean EOF before
/// the first length byte.
pub fn read_frame<R: Read + ?Sized>(r: &mut R, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 2];
    let first = loop {
        match r.read(&mut len[..1]) {
            Ok(n) => break n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    };
    if first == 0 {
        return Ok(None);
    }
    r.read_exact(&mut len[1..])?;
    let len = usize::from(u16::from_be_bytes(len));
    if len > max {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("Noise message of {len} bytes exceeds {max}"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

/// The writing half of an encrypted stream: buffers up to [`MAX_PLAINTEXT`] bytes per
/// transport message; `flush` sends what is buffered.
pub struct NoiseWriter<W: Write> {
    inner: W,
    cipher: Cipher,
    nonce: u64,
    buf: Vec<u8>,
}

impl<W: Write> NoiseWriter<W> {
    pub fn new(inner: W, cipher: Cipher) -> Self {
        Self {
            inner,
            cipher,
            nonce: 0,
            buf: Vec::with_capacity(MAX_PLAINTEXT),
        }
    }

    fn send_buffered(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let msg = self
            .cipher
            .seal(self.nonce, &self.buf)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.nonce += 1;
        self.buf.clear();
        write_frame(&mut self.inner, &msg)
    }
}

impl<W: Write> Write for NoiseWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let room = MAX_PLAINTEXT - self.buf.len();
        let n = data.len().min(room);
        self.buf.extend_from_slice(&data[..n]);
        if self.buf.len() == MAX_PLAINTEXT {
            self.send_buffered()?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send_buffered()?;
        self.inner.flush()
    }
}

/// The reading half of an encrypted stream. A message that fails authentication (tampered,
/// replayed, reordered, truncated) or is empty (spec §7.2: senders never send one, so an
/// empty message can only stall the reader) is an `InvalidData` error; the session must end.
pub struct NoiseReader<R: Read> {
    inner: R,
    cipher: Cipher,
    nonce: u64,
    buf: Vec<u8>,
    pos: usize,
}

impl<R: Read> NoiseReader<R> {
    pub fn new(inner: R, cipher: Cipher) -> Self {
        Self {
            inner,
            cipher,
            nonce: 0,
            buf: Vec::new(),
            pos: 0,
        }
    }
}

impl<R: Read> Read for NoiseReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.pos == self.buf.len() {
            let Some(msg) = read_frame(&mut self.inner, MAX_NOISE_MESSAGE)? else {
                return Ok(0);
            };
            if msg.len() <= TAG_LEN {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "empty or short Noise transport message",
                ));
            }
            self.buf = self.cipher.open(self.nonce, &msg).map_err(|_| {
                io::Error::new(
                    ErrorKind::InvalidData,
                    "encrypted data failed authentication (tampered or corrupted)",
                )
            })?;
            self.nonce += 1;
            self.pos = 0;
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
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

    fn b32(v: &serde_json::Value) -> [u8; 32] {
        unhex32(v.as_str().unwrap()).unwrap()
    }

    fn dev(v: &serde_json::Value) -> DeviceInfo {
        DeviceInfo {
            id: v["id"].as_str().unwrap().into(),
            name: v["name"].as_str().unwrap().into(),
            platform: v["platform"].as_str().unwrap().into(),
        }
    }

    /// Same bytes as the Python reference (which produced docs/test-vectors.json).
    #[test]
    fn handshake_matches_vectors() {
        let v = v1();
        let (ki, kr) = (&v["keys"]["initiator"], &v["keys"]["responder"]);
        let hs = &v["handshake"];
        let i_key = DeviceKey::from_secret(b32(&ki["static_private"]));
        let r_key = DeviceKey::from_secret(b32(&kr["static_private"]));
        let (ie, re) = (b32(&ki["ephemeral_private"]), b32(&kr["ephemeral_private"]));
        let mut i = Initiator::with(&i_key, Some(&ie), b32(&ki["sas_nonce"])).unwrap();
        let mut r = Responder::with(&r_key, Some(&re), b32(&kr["sas_nonce"])).unwrap();
        assert_eq!(hex(&prologue()), hs["prologue_hex"].as_str().unwrap());

        let m1 = i.write_msg1().unwrap();
        assert_eq!(hex(&m1), hs["msg1_hex"].as_str().unwrap());
        r.read_msg1(&m1).unwrap();
        let m2 = r.write_msg2(&dev(&kr["device"])).unwrap();
        assert_eq!(hex(&m2), hs["msg2_hex"].as_str().unwrap());
        assert_eq!(hex(&r.handshake_hash()), hs["h2_hex"].as_str().unwrap());
        let seen_by_i = i.read_msg2(&m2).unwrap();
        assert_eq!(seen_by_i.device, dev(&kr["device"]));
        assert_eq!(seen_by_i.fingerprint(), kr["fingerprint"].as_str().unwrap());
        let m3 = i.write_msg3(&dev(&ki["device"])).unwrap();
        assert_eq!(hex(&m3), hs["msg3_hex"].as_str().unwrap());
        let seen_by_r = r.read_msg3(&m3).unwrap();
        assert_eq!(seen_by_r.fingerprint(), ki["fingerprint"].as_str().unwrap());
        assert_eq!(hex(&i.handshake_hash()), hs["h_hex"].as_str().unwrap());
        assert_eq!(hex(&r.handshake_hash()), hs["h_hex"].as_str().unwrap());

        let sas = &v["sas"];
        assert_eq!(seen_by_i.sas, seen_by_r.sas);
        assert_eq!(
            u64::from(seen_by_i.sas.value),
            sas["value"].as_u64().unwrap()
        );
        assert_eq!(seen_by_i.sas.digits, sas["digits"].as_str().unwrap());
        assert_eq!(seen_by_i.sas.text, sas["text"].as_str().unwrap());

        let (ci, cr) = (i.finish().unwrap(), r.finish().unwrap());
        for t in v["transport"].as_array().unwrap() {
            let plain = crypto::unhex(t["plaintext_hex"].as_str().unwrap()).unwrap();
            let nonce = t["nonce"].as_u64().unwrap();
            let (tx, rx) = if t["from"] == "initiator" {
                (&ci, &cr)
            } else {
                (&cr, &ci)
            };
            let sealed = tx.seal(nonce, &plain).unwrap();
            let mut framed = Vec::new();
            write_frame(&mut framed, &sealed).unwrap();
            assert_eq!(hex(&framed), t["message_hex"].as_str().unwrap());
            assert_eq!(rx.open(nonce, &sealed).unwrap(), plain);
            let mut tampered = sealed.clone();
            tampered[0] ^= 1;
            assert!(rx.open(nonce, &tampered).is_err());
            assert!(rx.open(nonce + 1, &sealed).is_err(), "wrong counter");
        }
    }

    /// Initiator and responder in the state of the vector handshake after message 1.
    fn vector_pair(v: &serde_json::Value) -> (Initiator, Responder, DeviceKey, DeviceKey) {
        let (ki, kr) = (&v["keys"]["initiator"], &v["keys"]["responder"]);
        let i_key = DeviceKey::from_secret(b32(&ki["static_private"]));
        let r_key = DeviceKey::from_secret(b32(&kr["static_private"]));
        let (ie, re) = (b32(&ki["ephemeral_private"]), b32(&kr["ephemeral_private"]));
        let mut i = Initiator::with(&i_key, Some(&ie), b32(&ki["sas_nonce"])).unwrap();
        let mut r = Responder::with(&r_key, Some(&re), b32(&kr["sas_nonce"])).unwrap();
        r.read_msg1(&i.write_msg1().unwrap()).unwrap();
        (i, r, i_key, r_key)
    }

    #[test]
    fn negative_vectors_are_rejected() {
        let v = v1();
        let neg = &v["negative"];
        let bytes = |k: &str| crypto::unhex(neg[k].as_str().unwrap()).unwrap();
        let resp_dev = dev(&v["keys"]["responder"]["device"]);

        let mut fresh = Responder::new(&DeviceKey::generate()).unwrap();
        assert!(fresh.read_msg1(&bytes("msg1_low_order_hex")).is_err());

        let (mut i, mut r, _, _) = vector_pair(&v);
        r.write_msg2(&resp_dev).unwrap();
        assert!(i.read_msg2(&bytes("msg2_tampered_hex")).is_err());

        for case in ["msg3_tampered_hex", "msg3_bad_commitment_hex"] {
            let (mut i, mut r, _, _) = vector_pair(&v);
            i.read_msg2(&r.write_msg2(&resp_dev).unwrap()).unwrap();
            assert!(r.read_msg3(&bytes(case)).is_err(), "{case}");
        }

        let mut long = bytes("handshake_frame_too_long_hex");
        long.extend(vec![0u8; 3000]);
        assert!(read_frame(&mut long.as_slice(), MAX_HANDSHAKE_MESSAGE).is_err());

        // Transport: the responder of the completed vector handshake.
        let (mut i, mut r, _, _) = vector_pair(&v);
        i.read_msg2(&r.write_msg2(&resp_dev).unwrap()).unwrap();
        r.read_msg3(
            &i.write_msg3(&dev(&v["keys"]["initiator"]["device"]))
                .unwrap(),
        )
        .unwrap();
        let cr = r.finish().unwrap();
        for case in ["transport_empty_hex", "transport_short_hex"] {
            let wire = bytes(case);
            let mut out = [0u8; 8];
            assert!(
                NoiseReader::new(wire.as_slice(), cr.clone())
                    .read(&mut out)
                    .is_err(),
                "{case}"
            );
        }
        assert_eq!(bytes("busy_reply_hex"), [0, 0]);
    }

    #[test]
    fn handshake_names_are_cleaned_and_ids_checked() {
        let nonce = "00".repeat(32);
        let ok = format!(
            r#"{{"id":"x","name":"Ev\u202eil\u0007 {}","platform":"pla\u200btform-too-long-x","nonce":"{nonce}"}}"#,
            "n".repeat(100)
        );
        let (d, _) = parse_identity(ok.as_bytes()).unwrap();
        assert_eq!(d.name.chars().count(), MAX_NAME_CHARS);
        assert!(d.name.starts_with("Evil "));
        assert_eq!(d.platform.chars().count(), MAX_PLATFORM_CHARS);
        for bad_id in [r#""x\u202e""#, r#"" x""#, r#""""#, r#"1"#] {
            let text = format!(r#"{{"id":{bad_id},"name":"n","platform":"p","nonce":"{nonce}"}}"#);
            assert!(parse_identity(text.as_bytes()).is_err(), "{bad_id}");
        }
        let text = format!(r#"{{"id":"x","name":1,"platform":"p","nonce":"{nonce}"}}"#);
        assert!(parse_identity(text.as_bytes()).is_err(), "non-string name");
    }

    #[test]
    fn commitment_is_checked() {
        let (a, b) = (DeviceKey::generate(), DeviceKey::generate());
        let me = DeviceInfo {
            id: "x".into(),
            name: "X".into(),
            platform: "linux".into(),
        };
        let mut i = Initiator::new(&a).unwrap();
        let mut r = Responder::new(&b).unwrap();
        r.read_msg1(&i.write_msg1().unwrap()).unwrap();
        i.read_msg2(&r.write_msg2(&me).unwrap()).unwrap();
        i.nonce[0] ^= 1; // reveal a different nonce than committed to
        let m3 = i.write_msg3(&me).unwrap();
        assert!(matches!(r.read_msg3(&m3), Err(Error::Crypto(_))));
    }

    #[test]
    fn stream_roundtrip_spans_messages() {
        let (a, b) = (DeviceKey::generate(), DeviceKey::generate());
        let me = DeviceInfo {
            id: "x".into(),
            name: "X".into(),
            platform: "linux".into(),
        };
        let mut i = Initiator::new(&a).unwrap();
        let mut r = Responder::new(&b).unwrap();
        r.read_msg1(&i.write_msg1().unwrap()).unwrap();
        i.read_msg2(&r.write_msg2(&me).unwrap()).unwrap();
        r.read_msg3(&i.write_msg3(&me).unwrap()).unwrap();
        let (ci, cr) = (i.finish().unwrap(), r.finish().unwrap());
        let data: Vec<u8> = (0..200_000u32).map(|x| (x % 251) as u8).collect();
        let mut w = NoiseWriter::new(Vec::new(), ci);
        w.write_all(&data).unwrap();
        w.flush().unwrap();
        w.flush().unwrap(); // nothing buffered: no empty message
        let wire = w.inner;
        assert_eq!(wire.len(), data.len() + 4 * (2 + TAG_LEN), "4 messages");
        let mut back = Vec::new();
        NoiseReader::new(wire.as_slice(), cr.clone())
            .read_to_end(&mut back)
            .unwrap();
        assert_eq!(back, data);
        let mut cut = wire.clone();
        cut.truncate(wire.len() - 1);
        let mut sink = Vec::new();
        assert!(
            NoiseReader::new(cut.as_slice(), cr)
                .read_to_end(&mut sink)
                .is_err(),
            "truncated message"
        );
    }
}
