//! TCP file transfer (spec §3), encrypted with protocol v1 (spec §7) when both sides can,
//! plus the callback ("reverse connect") fallback of spec §6. Blocking I/O, one thread per
//! connection.
//!
//! Sender: connect (direct, else callback) → choose v1 or v0 (spec §7.1) → handshake →
//! `Progress::Session` (SAS + trust state for the UI) → Offer → Decision → files → Result.
//! Receiver: hello byte picks v0 or v1 → handshake → the UI's `decide` sees the offer with
//! the session's trust state and SAS → files → Result. The v1 stream inside Noise carries
//! exactly the v0 bytes after the hello.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Error;
use crate::crypto::{DeviceKey, QrPayload};
use crate::discovery::{
    CALLBACK_CONNECT_TIMEOUT, CALLBACK_RESEND, CALLBACK_WINDOW, CallbackRequest, Discovery, Peer,
    same_ip,
};
use crate::protocol::{
    self, CRYPTO_VERSION, CallbackHello, Decision, DeviceInfo, FileEntry, HASH_LEN, MAX_FILES,
    Offer, TransferResult, VERSION_V1,
};
use crate::sanitize::{sanitize_file_name, unique_path};
use crate::session::{
    Cipher, Initiator, MAX_HANDSHAKE_MESSAGE, NoiseReader, NoiseWriter, Responder, read_frame,
    write_frame,
};
use crate::trust::{PeerKey, SessionInfo, Trust, TrustState, TrustStore};

const CHUNK: usize = 256 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Direct connect timeout before `send_to_peer` falls back to a callback (spec §6).
pub const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Deadline for the whole callback hello on an accepted connection (not per read).
const CALLBACK_HELLO_TIMEOUT: Duration = Duration::from_secs(3);
/// The receiver may be waiting for a human to press "Accept".
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Transition period (ADR 0010): v1 nodes still talk to v0 peers (old apps) without
/// encryption, flagged in the UI. A later release sets this to `false` (then v0 peers are
/// refused on both sides) — see `docs/protocol.md` §7.1.
pub const ALLOW_PLAINTEXT_DEFAULT: bool = true;

/// What this node accepts besides encrypted sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CryptoPolicy {
    /// Talk to v0 peers in plaintext (never to a device whose key is already known).
    pub allow_plaintext: bool,
}

impl Default for CryptoPolicy {
    fn default() -> Self {
        Self {
            allow_plaintext: ALLOW_PLAINTEXT_DEFAULT,
        }
    }
}

/// This device as a party to sessions: who it says it is, its static key, the trust store
/// and the plaintext policy. Cheap to clone (the key and store are shared).
#[derive(Debug, Clone)]
pub struct Local {
    pub device: DeviceInfo,
    pub key: Arc<DeviceKey>,
    pub trust: Trust,
    pub policy: CryptoPolicy,
}

impl Local {
    pub fn new(device: DeviceInfo, key: DeviceKey, trust: Trust) -> Self {
        Self {
            device,
            key: Arc::new(key),
            trust,
            policy: CryptoPolicy::default(),
        }
    }

    /// A throw-away identity: fresh key, empty in-memory trust store (tests, tools).
    pub fn ephemeral(device: DeviceInfo) -> Self {
        Self::new(
            device,
            DeviceKey::generate(),
            Trust::in_memory(TrustStore::new()),
        )
    }

    /// This device's key fingerprint (spec §7.5).
    pub fn fingerprint(&self) -> String {
        self.key.fingerprint()
    }

    /// Starts showing a QR code for this device; returns the payload (spec §7.7).
    pub fn begin_qr(&self) -> String {
        self.trust.begin_qr(&self.device, &self.key)
    }
}

/// Progress callbacks, shared by sender and receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    FileStarted {
        index: usize,
        name: String,
        size: u64,
    },
    Bytes {
        done: u64,
        total: u64,
    },
    FileDone {
        index: usize,
        path: Option<PathBuf>,
    },
    /// Sender only: the direct connection failed; asking the peer to connect back
    /// (spec §6), which can take up to `CALLBACK_WINDOW`.
    Fallback,
    /// Sender only, before the offer goes out: how the session is protected (encrypted or
    /// not, trust state, SAS to show while waiting for the receiver).
    Session(SessionInfo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Delivered,
    Declined(Option<String>),
}

/// Sends regular files to an address (no discovery: no id, no callback). Directories are
/// rejected (v0 has no folders). Encrypted unless the receiver is a v0 node and the policy
/// allows plaintext.
pub fn send_files(
    to: SocketAddr,
    local: &Local,
    paths: &[PathBuf],
    progress: &mut dyn FnMut(Progress),
) -> Result<SendOutcome, Error> {
    let offer = build_offer(&local.device, paths)?;
    let peer = Peer::at(to);
    let stream = TcpStream::connect_timeout(&to, CONNECT_TIMEOUT)?;
    send_session(stream, Route::Direct, &peer, local, &offer, paths, progress)
}

/// Sends to a discovered peer: direct TCP first (3 s timeout); if that cannot connect and
/// `discovery` is given, asks the peer to connect back (spec §6). Fails with
/// [`Error::Unreachable`] if neither works, [`Error::KeyChanged`] if the peer's key is not
/// the remembered one. `local.device` should equal `discovery.me()`.
pub fn send_to_peer(
    peer: &Peer,
    local: &Local,
    discovery: Option<&Discovery>,
    paths: &[PathBuf],
    progress: &mut dyn FnMut(Progress),
) -> Result<SendOutcome, Error> {
    let offer = build_offer(&local.device, paths)?;
    let (stream, route) = connect(peer, discovery, true, progress)?;
    send_session(stream, route, peer, local, &offer, paths, progress)
}

/// Like [`send_to_peer`] but skips the direct attempt (tests, `--no-direct` debugging).
pub fn send_via_callback(
    peer: &Peer,
    local: &Local,
    discovery: &Discovery,
    paths: &[PathBuf],
    progress: &mut dyn FnMut(Progress),
) -> Result<SendOutcome, Error> {
    let offer = build_offer(&local.device, paths)?;
    let (stream, route) = connect(peer, Some(discovery), false, progress)?;
    send_session(stream, route, peer, local, &offer, paths, progress)
}

/// Scanner side of the QR verification (spec §7.7), after `Trust::check_qr` matched
/// locally: opens an encrypted session to `peer`, checks that it presents exactly the
/// scanned key, and sends the one-time code so the other device marks this one verified
/// too. The code is never sent over a session with another key.
pub fn confirm_qr(
    peer: &Peer,
    local: &Local,
    discovery: Option<&Discovery>,
    qr: &QrPayload,
) -> Result<(), Error> {
    if peer.device.id != qr.id {
        return Err(Error::Input("the code belongs to another device".into()));
    }
    let (stream, _) = connect(peer, discovery, true, &mut |_| {})?;
    let (cipher, _) = match open_initiator(&stream, local, Some(&qr.id), Some(&qr.fingerprint)) {
        Ok(v) => v,
        Err(Handshake::PeerClosed) => {
            return Err(Error::Unencrypted(format!(
                "{} cannot verify (old Hoppy version)",
                peer.device.name
            )));
        }
        Err(Handshake::Failed(e)) => return Err(e),
    };
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut writer = NoiseWriter::new(stream.try_clone()?, cipher.clone());
    let mut reader = NoiseReader::new(BufReader::new(stream.try_clone()?), cipher);
    protocol::write_json(
        &mut writer,
        &VerifyRequest {
            verify: qr.code.clone(),
        },
    )?;
    writer.flush()?;
    let result: TransferResult = protocol::read_json(&mut reader)?;
    if result.ok {
        Ok(())
    } else {
        Err(Error::Remote(
            result
                .error
                .unwrap_or_else(|| "verification refused".into()),
        ))
    }
}

/// First frame of a verification session instead of an Offer (spec §7.7).
#[derive(Serialize, Deserialize)]
struct VerifyRequest {
    verify: String,
}

/// How the connection came about; a callback hello may say whether the peer encrypts.
#[derive(Debug, Clone, Copy)]
enum Route {
    Direct,
    Callback { crypto: Option<u8> },
}

/// Direct connect (when `direct`), else/then the callback fallback when `discovery` is set.
fn connect(
    peer: &Peer,
    discovery: Option<&Discovery>,
    direct: bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<(TcpStream, Route), Error> {
    if direct {
        match TcpStream::connect_timeout(&peer.addr, DIRECT_CONNECT_TIMEOUT) {
            Ok(stream) => return Ok((stream, Route::Direct)),
            Err(e) if discovery.is_none() || peer.device.id.is_empty() => {
                return Err(Error::Unreachable(format!("{}: {e}", peer.addr)));
            }
            Err(_) => {}
        }
    }
    let discovery = discovery.ok_or_else(|| Error::Input("a callback needs discovery".into()))?;
    progress(Progress::Fallback);
    let (stream, hello) = connect_via_callback(peer, discovery)?;
    Ok((
        stream,
        Route::Callback {
            crypto: hello.crypto,
        },
    ))
}

/// Sender side of spec §6: listen on an ephemeral port, send `callback` datagrams for up to
/// [`CALLBACK_WINDOW`], and return the first connection that proves the token.
fn connect_via_callback(
    peer: &Peer,
    discovery: &Discovery,
) -> Result<(TcpStream, CallbackHello), Error> {
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let token = protocol::new_token();
    let ticket = discovery.request_callback(peer, port, &token);
    let start = Instant::now();
    let mut next_resend = start + CALLBACK_RESEND;
    loop {
        match listener.accept() {
            // Only the peer's known IP may connect back (spec §6 step 3); anything else is
            // closed before we read a byte. A wrong or silent client is dropped too.
            Ok((stream, from)) if same_ip(from.ip(), peer.addr.ip()) => {
                if let Ok(found) = check_callback_hello(stream, &token, &peer.device.id) {
                    return Ok(found);
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(_) => {} // e.g. ECONNABORTED: transient, retry until the window closes
        }
        let now = Instant::now();
        if now.duration_since(start) >= CALLBACK_WINDOW {
            return Err(Error::Unreachable(format!(
                "{} did not connect directly or back within {} s",
                peer.device.name,
                CALLBACK_WINDOW.as_secs()
            )));
        }
        if now >= next_resend {
            ticket.resend();
            next_resend = now + CALLBACK_RESEND;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn check_callback_hello(
    stream: TcpStream,
    token: &str,
    peer_id: &str,
) -> Result<(TcpStream, CallbackHello), Error> {
    stream.set_nonblocking(false)?; // accepted sockets may inherit non-blocking mode
    let mut reader = DeadlineReader {
        stream: &stream,
        deadline: Instant::now() + CALLBACK_HELLO_TIMEOUT,
    };
    let hello = protocol::read_callback_hello(&mut reader)?;
    if protocol::tokens_equal(&hello.token, token) && hello.id == peer_id {
        Ok((stream, hello))
    } else {
        Err(Error::Protocol("callback token mismatch".into()))
    }
}

/// Reads from a socket until one overall deadline, so a peer trickling one byte at a time
/// cannot hold the reader longer than that (slow-loris).
struct DeadlineReader<'a> {
    stream: &'a TcpStream,
    deadline: Instant,
}

impl Read for DeadlineReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        (&mut &*self.stream).read(buf)
    }
}

fn build_offer(me: &DeviceInfo, paths: &[PathBuf]) -> Result<Offer, Error> {
    if paths.is_empty() {
        return Err(Error::Input("no files to send".into()));
    }
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let meta = fs::metadata(path)?;
        if !meta.is_file() {
            return Err(Error::Input(format!(
                "{} is not a regular file",
                path.display()
            )));
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| Error::Input(format!("{} has no file name", path.display())))?;
        files.push(FileEntry {
            name,
            size: meta.len(),
        });
    }
    Ok(Offer {
        sender: me.clone(),
        files,
    })
}

/// Which session version the sender uses (spec §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    Plaintext,
    /// `fallback`: if the peer closes right after our v1 hello (a v0 node), reconnect and
    /// speak v0.
    Encrypted {
        fallback: bool,
    },
}

/// `crypto`: what the peer advertised (`None` = unknown); `pinned`: we hold a key for its
/// id, so it is a v1 device and plaintext would be a downgrade.
fn plan(crypto: Option<u8>, pinned: bool, policy: CryptoPolicy) -> Plan {
    match crypto {
        Some(c) if c >= CRYPTO_VERSION => Plan::Encrypted { fallback: false },
        _ if pinned || !policy.allow_plaintext => Plan::Encrypted { fallback: false },
        Some(_) => Plan::Plaintext,
        None => Plan::Encrypted { fallback: true },
    }
}

/// Why the initiator's handshake did not complete.
enum Handshake {
    /// The peer closed (or reset) the connection without answering our v1 hello: a v0 node.
    PeerClosed,
    Failed(Error),
}

impl From<Error> for Handshake {
    fn from(e: Error) -> Self {
        Handshake::Failed(e)
    }
}

fn closed_by_peer(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::UnexpectedEof
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe
    )
}

/// Initiator side of spec §7.3 on a fresh connection: `HOPY 0x01` + message 1, read
/// message 2, check the responder (expected id; `expect_fp` for QR verification, else the
/// trust store: a changed key aborts before we reveal anything), write message 3.
fn open_initiator(
    stream: &TcpStream,
    local: &Local,
    expected_id: Option<&str>,
    expect_fp: Option<&str>,
) -> Result<(Cipher, SessionInfo), Handshake> {
    let io = |e: std::io::Error| Handshake::Failed(e.into());
    stream.set_nodelay(true).map_err(io)?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(io)?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(io)?;
    let mut init = Initiator::new(&local.key)?;
    let mut first = Vec::new();
    protocol::write_hello_v1(&mut first)?;
    write_frame(&mut first, &init.write_msg1()?).map_err(io)?;
    match (&mut &*stream).write_all(&first) {
        Err(e) if closed_by_peer(&e) => return Err(Handshake::PeerClosed),
        other => other.map_err(io)?,
    }
    let msg2 = match read_frame(&mut &*stream, MAX_HANDSHAKE_MESSAGE) {
        Ok(Some(m)) => m,
        Ok(None) => return Err(Handshake::PeerClosed),
        Err(e) if closed_by_peer(&e) => return Err(Handshake::PeerClosed),
        Err(e) => return Err(io(e)),
    };
    let remote = init.read_msg2(&msg2)?;
    let peer = PeerKey::new(&remote.device, remote.fingerprint());
    if let Some(id) = expected_id.filter(|id| !id.is_empty())
        && peer.id != id
    {
        return Err(Handshake::Failed(Error::Crypto(format!(
            "the device at this address is not the one chosen ({})",
            remote.device.name
        ))));
    }
    let state = local.trust.state(&peer.id, &peer.fingerprint);
    match expect_fp {
        Some(fp) if fp != peer.fingerprint => {
            return Err(Handshake::Failed(Error::VerificationFailed(format!(
                "{} presented another key than the scanned code (possible interception)",
                remote.device.name
            ))));
        }
        Some(_) => {}
        None if state == TrustState::KeyChanged => {
            return Err(Handshake::Failed(Error::KeyChanged(remote.device.name)));
        }
        None => {}
    }
    let msg3 = init.write_msg3(&local.device)?;
    write_frame(&mut &*stream, &msg3).map_err(io)?;
    let cipher = init.finish()?;
    Ok((
        cipher,
        SessionInfo {
            encrypted: true,
            peer: Some(peer),
            trust: Some(state),
            sas: Some(remote.sas),
        },
    ))
}

/// Picks v1 or v0 for this connection (spec §7.1) and runs the sender side.
fn send_session(
    stream: TcpStream,
    route: Route,
    peer: &Peer,
    local: &Local,
    offer: &Offer,
    paths: &[PathBuf],
    progress: &mut dyn FnMut(Progress),
) -> Result<SendOutcome, Error> {
    let pinned = !peer.device.id.is_empty() && local.trust.get(&peer.device.id).is_some();
    let advertised = match route {
        Route::Direct => peer.crypto,
        Route::Callback { crypto } => crypto.max(peer.crypto),
    };
    let expected = Some(peer.device.id.as_str());
    match plan(advertised, pinned, local.policy) {
        Plan::Plaintext => {
            progress(Progress::Session(SessionInfo::plaintext()));
            send_over(stream, None, local, offer, paths, progress)
        }
        Plan::Encrypted { fallback } => match open_initiator(&stream, local, expected, None) {
            Ok((cipher, info)) => {
                let peer_key = info.peer.clone();
                progress(Progress::Session(info));
                send_over(
                    stream,
                    Some((cipher, peer_key)),
                    local,
                    offer,
                    paths,
                    progress,
                )
            }
            Err(Handshake::PeerClosed) if fallback && matches!(route, Route::Direct) => {
                drop(stream);
                let stream = TcpStream::connect_timeout(&peer.addr, CONNECT_TIMEOUT)?;
                progress(Progress::Session(SessionInfo::plaintext()));
                send_over(stream, None, local, offer, paths, progress)
            }
            Err(Handshake::PeerClosed) if advertised.unwrap_or(0) >= CRYPTO_VERSION => Err(
                Error::Protocol("connection closed during the encrypted handshake".into()),
            ),
            Err(Handshake::PeerClosed) => Err(Error::Unencrypted(format!(
                "{} cannot encrypt (old Hoppy version?){}",
                peer.device.name,
                if pinned {
                    " although it did before: possible downgrade attack"
                } else {
                    ""
                }
            ))),
            Err(Handshake::Failed(e)) => Err(e),
        },
    }
}

/// Spec §3 from the sender's side, over an already-open connection (direct or callback),
/// in plaintext (`secure` = None, writes the v0 hello first) or inside the v1 session.
fn send_over(
    stream: TcpStream,
    secure: Option<(Cipher, Option<PeerKey>)>,
    local: &Local,
    offer: &Offer,
    paths: &[PathBuf],
    progress: &mut dyn FnMut(Progress),
) -> Result<SendOutcome, Error> {
    let total = offer.total_size();
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let (mut reader, mut writer, peer): (Box<dyn Read>, Box<dyn Write>, Option<PeerKey>) =
        match secure {
            None => {
                let mut w = BufWriter::with_capacity(CHUNK, stream.try_clone()?);
                protocol::write_hello(&mut w)?;
                (
                    Box::new(BufReader::new(stream.try_clone()?)),
                    Box::new(w),
                    None,
                )
            }
            Some((cipher, peer)) => (
                Box::new(NoiseReader::new(
                    BufReader::new(stream.try_clone()?),
                    cipher.clone(),
                )),
                Box::new(NoiseWriter::new(stream.try_clone()?, cipher)),
                peer,
            ),
        };

    protocol::write_json(&mut writer, offer)?;
    writer.flush()?;

    stream.set_read_timeout(Some(DECISION_TIMEOUT))?;
    let decision: Decision = protocol::read_json(&mut reader)?;
    if !decision.accept {
        return Ok(SendOutcome::Declined(decision.reason));
    }
    if let Some(peer) = &peer {
        local.trust.pin(peer); // TOFU: remembered once a transfer was accepted
    }

    let mut done = 0u64;
    let mut buf = vec![0u8; CHUNK];
    for (index, (path, entry)) in paths.iter().zip(&offer.files).enumerate() {
        progress(Progress::FileStarted {
            index,
            name: entry.name.clone(),
            size: entry.size,
        });
        let mut file = File::open(path)?;
        let mut hasher = Sha256::new();
        let mut remaining = entry.size;
        while remaining > 0 {
            let want = remaining.min(CHUNK as u64) as usize;
            let n = file.read(&mut buf[..want])?;
            if n == 0 {
                return Err(Error::Input(format!(
                    "{} shrank while sending",
                    path.display()
                )));
            }
            hasher.update(&buf[..n]);
            writer.write_all(&buf[..n])?;
            remaining -= n as u64;
            done += n as u64;
            progress(Progress::Bytes { done, total });
        }
        let hash: [u8; HASH_LEN] = hasher.finalize().into();
        writer.write_all(&hash)?;
        progress(Progress::FileDone { index, path: None });
    }
    writer.flush()?;

    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    let result: TransferResult = protocol::read_json(&mut reader)?;
    if result.ok {
        Ok(SendOutcome::Delivered)
    } else {
        Err(Error::Remote(
            result.error.unwrap_or_else(|| "unknown error".into()),
        ))
    }
}

/// Decides whether to accept an incoming offer and observes progress.
pub trait ReceiveHandler: Send + Sync {
    /// Blocking; may ask the user. Return `false` to decline. `session` says whether the
    /// connection is encrypted, the sender's trust state (New/Known/Verified; KeyChanged
    /// offers never get here) and the SAS to show next to the question.
    fn decide(&self, offer: &Offer, from: SocketAddr, session: &SessionInfo) -> bool;
    fn progress(&self, _from: SocketAddr, _event: Progress) {}
    /// Called once per connection that got past the offer stage.
    fn finished(&self, _from: SocketAddr, _result: &Result<Vec<PathBuf>, Error>) {}
    /// An offer was declined without asking (spec §7.5): `reason` is `key_changed` (the
    /// sender's key is not the remembered one: warn, possible impersonation) or
    /// `encryption_required` (plaintext from a device known to encrypt, or plaintext not
    /// allowed).
    fn blocked(&self, _from: SocketAddr, _offer: &Offer, _session: &SessionInfo, _reason: &str) {}
    /// A device that scanned this device's QR code proved it (spec §7.7): it is now
    /// verified here too (show ✓, close the QR code).
    fn verified(&self, _from: SocketAddr, _peer: &PeerKey) {}
}

/// Accepts connections forever, one thread each.
pub fn serve(
    listener: TcpListener,
    dir: PathBuf,
    local: Local,
    handler: Arc<dyn ReceiveHandler>,
) -> ! {
    loop {
        match listener.accept() {
            Ok((stream, from)) => {
                let dir = dir.clone();
                let handler = Arc::clone(&handler);
                let local = local.clone();
                thread::spawn(move || {
                    let result = receive_one(stream, from, &dir, &local, handler.as_ref());
                    if let Some(result) = result {
                        handler.finished(from, &result);
                    }
                });
            }
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// Receiver side of spec §6: makes `discovery` answer callback requests by connecting
/// back and receiving into `dir` with `handler` (which still decides: invariant 3).
pub fn accept_callbacks(
    discovery: &Discovery,
    dir: PathBuf,
    local: Local,
    handler: Arc<dyn ReceiveHandler>,
) {
    discovery.on_callback(Arc::new(move |request: CallbackRequest| {
        if let Some(result) = answer_callback(&request, &local, &dir, handler.as_ref()) {
            handler.finished(request.addr, &result);
        }
    }));
}

/// Connects to `request.addr`, sends the callback hello, then behaves like [`receive_one`].
pub fn answer_callback(
    request: &CallbackRequest,
    local: &Local,
    dir: &Path,
    handler: &dyn ReceiveHandler,
) -> Option<Result<Vec<PathBuf>, Error>> {
    let stream = TcpStream::connect_timeout(&request.addr, CALLBACK_CONNECT_TIMEOUT).ok()?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
    let hello = CallbackHello {
        id: local.device.id.clone(),
        token: request.token.clone(),
        crypto: Some(CRYPTO_VERSION),
    };
    protocol::write_callback_hello(&mut &stream, &hello).ok()?;
    request.mark_connected();
    receive_one(stream, request.addr, dir, local, handler)
}

/// Handles a single connection (v0 or v1, chosen by the hello byte). Returns `None` if the
/// offer was declined or invalid, or the connection was a verification.
pub fn receive_one(
    stream: TcpStream,
    from: SocketAddr,
    dir: &Path,
    local: &Local,
    handler: &dyn ReceiveHandler,
) -> Option<Result<Vec<PathBuf>, Error>> {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    // Unbuffered: nothing past the hello may be consumed before we know the version.
    let version = protocol::read_hello(&mut &stream).ok()?;
    let reader = stream.try_clone().ok()?;
    if version == VERSION_V1 {
        return receive_v1(stream, reader, from, dir, local, handler);
    }
    let mut reader: Box<dyn Read> = Box::new(BufReader::with_capacity(CHUNK, reader));
    let mut writer: Box<dyn Write> = Box::new(stream);
    let offer: Offer = protocol::read_json(&mut reader).ok()?;
    let session = SessionInfo::plaintext();
    // A device whose key we hold speaks v1: plaintext from its id is a downgrade (or an
    // impersonation). Without the transition allowance, every v0 sender is refused.
    if !local.policy.allow_plaintext || local.trust.get(&offer.sender.id).is_some() {
        reject(&mut writer, "encryption_required");
        handler.blocked(from, &offer, &session, "encryption_required");
        return None;
    }
    receive_offer(
        &mut reader,
        &mut writer,
        offer,
        session,
        from,
        dir,
        local,
        handler,
    )
}

fn reject(writer: &mut dyn Write, reason: &str) {
    let _ = protocol::write_json(writer, &Decision::reject(reason));
    let _ = writer.flush();
}

/// Responder side of spec §7.3, then the offer (or a verification, §7.7).
fn receive_v1(
    stream: TcpStream,
    mut raw: TcpStream,
    from: SocketAddr,
    dir: &Path,
    local: &Local,
    handler: &dyn ReceiveHandler,
) -> Option<Result<Vec<PathBuf>, Error>> {
    let mut resp = Responder::new(&local.key).ok()?;
    let msg1 = read_frame(&mut raw, MAX_HANDSHAKE_MESSAGE).ok()??;
    resp.read_msg1(&msg1).ok()?;
    let msg2 = resp.write_msg2(&local.device).ok()?;
    write_frame(&mut &stream, &msg2).ok()?;
    let msg3 = read_frame(&mut raw, MAX_HANDSHAKE_MESSAGE).ok()??;
    let remote = resp.read_msg3(&msg3).ok()?;
    let cipher = resp.finish().ok()?;
    let peer = PeerKey::new(&remote.device, remote.fingerprint());
    let state = local.trust.state(&peer.id, &peer.fingerprint);
    let session = SessionInfo {
        encrypted: true,
        peer: Some(peer.clone()),
        trust: Some(state),
        sas: Some(remote.sas),
    };
    let mut reader: Box<dyn Read> = Box::new(NoiseReader::new(
        BufReader::with_capacity(CHUNK, raw),
        cipher.clone(),
    ));
    let mut writer: Box<dyn Write> = Box::new(NoiseWriter::new(stream, cipher));

    let first: serde_json::Value = protocol::read_json(&mut reader).ok()?;
    if let Some(code) = first.get("verify") {
        let code = code.as_str().unwrap_or_default();
        let reply = answer_verify(code, &peer, state, local);
        if reply.ok {
            handler.verified(from, &peer);
        }
        let _ = protocol::write_json(&mut writer, &reply);
        let _ = writer.flush();
        return None;
    }
    let offer: Offer = match serde_json::from_value(first) {
        Ok(o) => o,
        Err(_) => {
            reject(&mut writer, "invalid_offer");
            return None;
        }
    };
    if offer.sender.id != peer.id {
        reject(&mut writer, "invalid_offer"); // the offer must come from the handshake's id
        return None;
    }
    if state == TrustState::KeyChanged {
        reject(&mut writer, "key_changed");
        handler.blocked(from, &offer, &session, "key_changed");
        return None;
    }
    receive_offer(
        &mut reader,
        &mut writer,
        offer,
        session,
        from,
        dir,
        local,
        handler,
    )
}

/// The receiver's answer to a verification request (spec §7.7).
fn answer_verify(code: &str, peer: &PeerKey, state: TrustState, local: &Local) -> TransferResult {
    let fail = |e: &str| TransferResult {
        ok: false,
        error: Some(e.into()),
    };
    if state == TrustState::KeyChanged {
        return fail("key_changed");
    }
    if !local.trust.take_qr_code(code) {
        return fail("invalid_code");
    }
    match local.trust.mark_verified(peer) {
        Ok(()) => TransferResult {
            ok: true,
            error: None,
        },
        Err(_) => fail("key_changed"),
    }
}

/// Common to v0 and v1 once the offer is read: validate, ask, receive, answer.
#[allow(clippy::too_many_arguments)]
fn receive_offer(
    reader: &mut Box<dyn Read>,
    writer: &mut Box<dyn Write>,
    offer: Offer,
    session: SessionInfo,
    from: SocketAddr,
    dir: &Path,
    local: &Local,
    handler: &dyn ReceiveHandler,
) -> Option<Result<Vec<PathBuf>, Error>> {
    let names = match validate_offer(&offer) {
        Ok(names) => names,
        Err(reason) => {
            reject(writer, reason);
            return None;
        }
    };
    if !handler.decide(&offer, from, &session) {
        reject(writer, "declined");
        return None;
    }
    if let Some(peer) = &session.peer {
        local.trust.pin(peer); // TOFU: remembered once the user accepted
    }
    if let Err(e) = protocol::write_json(writer, &Decision::accept()).and_then(|()| {
        writer.flush()?;
        Ok(())
    }) {
        return Some(Err(e));
    }

    let result = receive_files(reader, &offer, &names, dir, from, handler);
    let reply = match &result {
        Ok(_) => TransferResult {
            ok: true,
            error: None,
        },
        Err(e) => TransferResult {
            ok: false,
            error: Some(e.to_string()),
        },
    };
    let _ = protocol::write_json(writer, &reply);
    let _ = writer.flush();
    Some(result)
}

fn validate_offer(offer: &Offer) -> Result<Vec<String>, &'static str> {
    if offer.files.is_empty() {
        return Err("invalid_offer");
    }
    if offer.files.len() > MAX_FILES {
        return Err("too_many_files");
    }
    offer
        .files
        .iter()
        .map(|f| sanitize_file_name(&f.name).ok_or("invalid_offer"))
        .collect()
}

fn receive_files(
    reader: &mut impl Read,
    offer: &Offer,
    names: &[String],
    dir: &Path,
    from: SocketAddr,
    handler: &dyn ReceiveHandler,
) -> Result<Vec<PathBuf>, Error> {
    fs::create_dir_all(dir)?;
    let total = offer.total_size();
    let mut done = 0u64;
    let mut saved = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    let mut buf = vec![0u8; CHUNK];

    for (index, (entry, name)) in offer.files.iter().zip(names).enumerate() {
        handler.progress(
            from,
            Progress::FileStarted {
                index,
                name: name.clone(),
                size: entry.size,
            },
        );
        let part = unique_path(dir, &format!("{name}.hoppy-part"));
        let mut out = PartFile::create(&part)?;
        let mut hasher = Sha256::new();
        let mut remaining = entry.size;
        while remaining > 0 {
            let want = remaining.min(CHUNK as u64) as usize;
            let n = reader.read(&mut buf[..want])?;
            if n == 0 {
                return Err(Error::Protocol("connection closed mid-file".into()));
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            remaining -= n as u64;
            done += n as u64;
            handler.progress(from, Progress::Bytes { done, total });
        }
        let mut expected = [0u8; HASH_LEN];
        reader.read_exact(&mut expected)?;
        let actual: [u8; HASH_LEN] = hasher.finalize().into();
        if actual != expected {
            failed.push(name.clone());
            handler.progress(from, Progress::FileDone { index, path: None });
            continue; // PartFile drop deletes the partial file; stream is still in sync.
        }
        let final_path = unique_path(dir, name);
        out.commit(&final_path)?;
        handler.progress(
            from,
            Progress::FileDone {
                index,
                path: Some(final_path.clone()),
            },
        );
        saved.push(final_path);
    }

    if failed.is_empty() {
        Ok(saved)
    } else {
        Err(Error::Remote(format!(
            "hash mismatch: {}",
            failed.join(", ")
        )))
    }
}

/// A temporary file deleted on drop unless committed.
/// The handle is closed before rename/delete because Windows forbids both on open files.
struct PartFile {
    path: PathBuf,
    file: Option<BufWriter<File>>,
    committed: bool,
}

impl PartFile {
    fn create(path: &Path) -> Result<Self, Error> {
        let file = File::create_new(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Some(BufWriter::with_capacity(CHUNK, file)),
            committed: false,
        })
    }

    fn write_all(&mut self, data: &[u8]) -> Result<(), Error> {
        self.file
            .as_mut()
            .expect("open until commit")
            .write_all(data)?;
        Ok(())
    }

    fn commit(mut self, to: &Path) -> Result<(), Error> {
        let mut writer = self.file.take().expect("open until commit");
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&self.path, to)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PartFile {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_plan() {
        let allow = CryptoPolicy {
            allow_plaintext: true,
        };
        let strict = CryptoPolicy {
            allow_plaintext: false,
        };
        let enc = |fallback| Plan::Encrypted { fallback };
        assert_eq!(plan(Some(1), false, allow), enc(false));
        assert_eq!(plan(Some(2), false, allow), enc(false), "future versions");
        assert_eq!(plan(Some(0), false, allow), Plan::Plaintext, "old app");
        assert_eq!(
            plan(Some(0), true, allow),
            enc(false),
            "known key: no downgrade"
        );
        assert_eq!(
            plan(None, false, allow),
            enc(true),
            "typed address: try v1 first"
        );
        assert_eq!(plan(None, true, allow), enc(false));
        assert_eq!(plan(Some(0), false, strict), enc(false));
        assert_eq!(plan(None, false, strict), enc(false));
    }
}
