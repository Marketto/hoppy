//! TCP file transfer (spec §3). Blocking I/O with one thread per connection.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::Error;
use crate::protocol::{
    self, Decision, DeviceInfo, FileEntry, HASH_LEN, MAX_FILES, Offer, TransferResult,
};
use crate::sanitize::{sanitize_file_name, unique_path};

const CHUNK: usize = 256 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Delivered,
    Declined(Option<String>),
}

/// Sends regular files to a receiver. Directories are rejected (v0 has no folders).
pub fn send_files(
    to: SocketAddr,
    me: &DeviceInfo,
    paths: &[PathBuf],
    progress: &mut dyn FnMut(Progress),
) -> Result<SendOutcome, Error> {
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
    let offer = Offer {
        sender: me.clone(),
        files,
    };
    let total = offer.total_size();

    let stream = TcpStream::connect_timeout(&to, CONNECT_TIMEOUT)?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = BufWriter::with_capacity(CHUNK, stream.try_clone()?);

    protocol::write_hello(&mut writer)?;
    protocol::write_json(&mut writer, &offer)?;
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
