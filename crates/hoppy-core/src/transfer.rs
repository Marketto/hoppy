//! TCP file transfer (spec §3). Blocking I/O with one thread per connection.
//! Also the callback ("reverse connect") fallback of spec §6.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::Error;
use crate::discovery::{
    CALLBACK_CONNECT_TIMEOUT, CALLBACK_RESEND, CALLBACK_WINDOW, CallbackRequest, Discovery, Peer,
};
use crate::protocol::{
    self, CallbackHello, Decision, DeviceInfo, FileEntry, HASH_LEN, MAX_FILES, Offer,
    TransferResult,
};
use crate::sanitize::{sanitize_file_name, unique_path};

const CHUNK: usize = 256 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Direct connect timeout before `send_to_peer` falls back to a callback (spec §6).
pub const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the sender waits for the callback hello on an accepted connection.
const CALLBACK_HELLO_TIMEOUT: Duration = Duration::from_secs(3);
/// The receiver may be waiting for a human to press "Accept".
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
const IO_TIMEOUT: Duration = Duration::from_secs(30);

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Delivered,
    Declined(Option<String>),
}

/// Sends regular files to a receiver. Directories are rejected (v0 has no folders).
/// Direct connection only; see [`send_to_peer`] for the callback fallback.
pub fn send_files(
    to: SocketAddr,
    me: &DeviceInfo,
    paths: &[PathBuf],
    progress: &mut dyn FnMut(Progress),
) -> Result<SendOutcome, Error> {
    let offer = build_offer(me, paths)?;
    let stream = TcpStream::connect_timeout(&to, CONNECT_TIMEOUT)?;
    send_over(stream, &offer, paths, progress)
}

/// Sends to a discovered peer: direct TCP first (3 s timeout); if that cannot connect and
/// `discovery` is given, asks the peer to connect back (spec §6). Fails with
/// [`Error::Unreachable`] if neither works. `me` should equal `discovery.me()`.
pub fn send_to_peer(
    peer: &Peer,
    me: &DeviceInfo,
    discovery: Option<&Discovery>,
    paths: &[PathBuf],
    progress: &mut dyn FnMut(Progress),
) -> Result<SendOutcome, Error> {
    let offer = build_offer(me, paths)?;
    let stream = match TcpStream::connect_timeout(&peer.addr, DIRECT_CONNECT_TIMEOUT) {
        Ok(stream) => stream,
        Err(e) => match discovery {
            Some(d) => {
                progress(Progress::Fallback);
                connect_via_callback(peer, d)?
            }
            None => return Err(Error::Unreachable(format!("{}: {e}", peer.addr))),
        },
    };
    send_over(stream, &offer, paths, progress)
}

/// Like [`send_to_peer`] but skips the direct attempt (tests, `--no-direct` debugging).
pub fn send_via_callback(
    peer: &Peer,
    me: &DeviceInfo,
    discovery: &Discovery,
    paths: &[PathBuf],
    progress: &mut dyn FnMut(Progress),
) -> Result<SendOutcome, Error> {
    let offer = build_offer(me, paths)?;
    progress(Progress::Fallback);
    let stream = connect_via_callback(peer, discovery)?;
    send_over(stream, &offer, paths, progress)
}

/// Sender side of spec §6: listen on an ephemeral port, send `callback` datagrams for up to
/// [`CALLBACK_WINDOW`], and return the first connection that proves the token.
fn connect_via_callback(peer: &Peer, discovery: &Discovery) -> Result<TcpStream, Error> {
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let token = protocol::new_token();
    let ticket = discovery.request_callback(peer, port, &token);
    let start = Instant::now();
    let mut next_resend = start + CALLBACK_RESEND;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // A wrong or silent client is dropped; keep waiting for the real one.
                if let Ok(stream) = check_callback_hello(stream, &token, &peer.device.id) {
                    return Ok(stream);
                }
            }
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

fn check_callback_hello(stream: TcpStream, token: &str, peer_id: &str) -> Result<TcpStream, Error> {
    stream.set_nonblocking(false)?; // accepted sockets may inherit non-blocking mode
    stream.set_read_timeout(Some(CALLBACK_HELLO_TIMEOUT))?;
    let hello = protocol::read_callback_hello(&mut &stream)?;
    if protocol::tokens_equal(&hello.token, token) && hello.id == peer_id {
        Ok(stream)
    } else {
        Err(Error::Protocol("callback token mismatch".into()))
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

/// Spec §3 from the sender's side, over an already-open connection (direct or callback).
fn send_over(
    stream: TcpStream,
    offer: &Offer,
    paths: &[PathBuf],
    progress: &mut dyn FnMut(Progress),
) -> Result<SendOutcome, Error> {
    let total = offer.total_size();
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::with_capacity(CHUNK, stream.try_clone()?);

    protocol::write_hello(&mut writer)?;
    protocol::write_json(&mut writer, offer)?;
    writer.flush()?;

    stream.set_read_timeout(Some(DECISION_TIMEOUT))?;
    let decision: Decision = protocol::read_json(&mut reader)?;
    if !decision.accept {
        return Ok(SendOutcome::Declined(decision.reason));
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
    /// Blocking; may ask the user. Return `false` to decline.
    fn decide(&self, offer: &Offer, from: SocketAddr) -> bool;
    fn progress(&self, _from: SocketAddr, _event: Progress) {}
    /// Called once per connection that got past the offer stage.
    fn finished(&self, _from: SocketAddr, _result: &Result<Vec<PathBuf>, Error>) {}
}

/// Accepts connections forever, one thread each.
pub fn serve(listener: TcpListener, dir: PathBuf, handler: Arc<dyn ReceiveHandler>) -> ! {
    loop {
        match listener.accept() {
            Ok((stream, from)) => {
                let dir = dir.clone();
                let handler = Arc::clone(&handler);
                thread::spawn(move || {
                    let result = receive_one(stream, from, &dir, handler.as_ref());
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
pub fn accept_callbacks(discovery: &Discovery, dir: PathBuf, handler: Arc<dyn ReceiveHandler>) {
    let me = discovery.me().clone();
    discovery.on_callback(Arc::new(move |request: CallbackRequest| {
        if let Some(result) = answer_callback(&request, &me, &dir, handler.as_ref()) {
            handler.finished(request.addr, &result);
        }
    }));
}

/// Connects to `request.addr`, sends the callback hello, then behaves like [`receive_one`].
pub fn answer_callback(
    request: &CallbackRequest,
    me: &DeviceInfo,
    dir: &Path,
    handler: &dyn ReceiveHandler,
) -> Option<Result<Vec<PathBuf>, Error>> {
    let stream = TcpStream::connect_timeout(&request.addr, CALLBACK_CONNECT_TIMEOUT).ok()?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;
    let hello = CallbackHello {
        id: me.id.clone(),
        token: request.token.clone(),
    };
    protocol::write_callback_hello(&mut &stream, &hello).ok()?;
    receive_one(stream, request.addr, dir, handler)
}

/// Handles a single connection. Returns `None` if the offer was declined or invalid.
pub fn receive_one(
    stream: TcpStream,
    from: SocketAddr,
    dir: &Path,
    handler: &dyn ReceiveHandler,
) -> Option<Result<Vec<PathBuf>, Error>> {
    let mut reader = match stream.try_clone() {
        Ok(s) => BufReader::with_capacity(CHUNK, s),
        Err(e) => return Some(Err(e.into())),
    };
    let mut writer = stream;
    let _ = writer.set_read_timeout(Some(IO_TIMEOUT));
    let _ = reader.get_ref().set_read_timeout(Some(IO_TIMEOUT));

    if protocol::read_hello(&mut reader).is_err() {
        return None;
    }
    let offer: Offer = protocol::read_json(&mut reader).ok()?;

    let names = match validate_offer(&offer) {
        Ok(names) => names,
        Err(reason) => {
            let _ = protocol::write_json(&mut writer, &Decision::reject(reason));
            return None;
        }
    };
    if !handler.decide(&offer, from) {
        let _ = protocol::write_json(&mut writer, &Decision::reject("declined"));
        return None;
    }
    if let Err(e) = protocol::write_json(&mut writer, &Decision::accept()) {
        return Some(Err(e));
    }

    let result = receive_files(&mut reader, &offer, &names, dir, from, handler);
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
    let _ = protocol::write_json(&mut writer, &reply);
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
