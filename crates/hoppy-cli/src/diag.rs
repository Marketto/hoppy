//! Lightweight diagnostic log for `hoppyw` (Windows: `%LOCALAPPDATA%\Hoppy\hoppy.log`,
//! macOS: `~/Library/Logs/Hoppy/hoppy.log`, elsewhere `<data dir>/hoppy/hoppy.log`). Tray events, menus, slow UI handlers and worker
//! errors go here, so "nothing happens when I click" can be diagnosed on a user's PC
//! (`hoppy log`, tray menu "About & diagnostics…").
//!
//! - `log` never blocks: lines go through a channel to one writer thread (the UI thread must
//!   not do file I/O). Before `init`, and in the `hoppy` CLI, it does nothing.
//! - One line per entry: UTC timestamp, process id, role, message (newlines flattened).
//! - Privacy: no file contents, file names or paths. Logged: counts, sizes, ids, versions
//!   and errors: hoppy-core errors only as their category (`error_kind`; their texts can
//!   carry received names), other errors as text with anything path-like replaced by
//!   `<path>` (`redact_paths`, applied to every line by `format_line`). Local and peer IP
//!   addresses and device names may appear (network diagnosis).
//! - Rotation: when the file would exceed `MAX_LOG` it becomes `hoppy.log.1` (one backup).
//! - `UiWatch`: times UI-thread handlers (modal menus/dialogs excluded), logs those over
//!   `SLOW_HANDLER`, and a watchdog thread logs a handler still running after `STUCK`.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Rotate before the file grows past this.
pub const MAX_LOG: u64 = 256 * 1024;
/// UI handlers slower than this are logged.
pub const SLOW_HANDLER: Duration = Duration::from_millis(200);
/// The watchdog reports a UI handler still running after this.
pub const STUCK: Duration = Duration::from_secs(2);

enum Msg {
    Line(String),
    Flush(Sender<()>),
}

struct Sink {
    tx: Mutex<Sender<Msg>>,
    role: &'static str,
}

static SINK: OnceLock<Sink> = OnceLock::new();

/// `%LOCALAPPDATA%\Hoppy\hoppy.log` (local, never a roaming/redirected folder); macOS:
/// `~/Library/Logs/Hoppy/hoppy.log` (shown by Console.app).
pub fn log_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return Some(dirs::home_dir()?.join("Library/Logs/Hoppy/hoppy.log"));
    }
    let base = dirs::data_local_dir()?;
    let dir = if cfg!(windows) { "Hoppy" } else { "hoppy" };
    Some(base.join(dir).join("hoppy.log"))
}

/// Starts the writer thread; later calls are ignored. `role`: "tray", "send", …
pub fn init(role: &'static str) {
    let Some(path) = log_path() else { return };
    let (tx, rx) = mpsc::channel::<Msg>();
    let sink = Sink {
        tx: Mutex::new(tx),
        role,
    };
    if SINK.set(sink).is_err() {
        return;
    }
    thread::spawn(move || {
        let _ = path.parent().map(fs::create_dir_all);
        for msg in rx {
            match msg {
                // Logging must never take the app down: write errors are dropped.
                Msg::Line(line) => {
                    let _ = append(&path, &line, MAX_LOG);
                }
                Msg::Flush(done) => {
                    let _ = done.send(());
                }
            }
        }
    });
}

/// Queues one line (no-op before `init`). Never blocks on I/O.
pub fn log(msg: impl AsRef<str>) {
    let Some(sink) = SINK.get() else { return };
    let line = format_line(
        SystemTime::now(),
        std::process::id(),
        sink.role,
        msg.as_ref(),
    );
    if let Ok(tx) = sink.tx.lock() {
        let _ = tx.send(Msg::Line(line));
    }
}

/// Waits (up to `timeout`) until every queued line is written. Workers only.
pub fn flush(timeout: Duration) {
    let Some(sink) = SINK.get() else { return };
    let (done_tx, done_rx) = mpsc::channel();
    let sent = sink
        .tx
        .lock()
        .is_ok_and(|tx| tx.send(Msg::Flush(done_tx)).is_ok());
    if sent {
        let _ = done_rx.recv_timeout(timeout);
    }
}

/// `2026-10-15 08:30:01.250Z [1234 tray] message`, control characters flattened, paths
/// redacted (`redact_paths`).
pub fn format_line(now: SystemTime, pid: u32, role: &str, msg: &str) -> String {
    let clean: String = msg
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!(
        "{} [{pid} {role}] {}\n",
        utc_stamp(now),
        redact_paths(&clean)
    )
}

/// Replaces anything path-like with `<path>`: `X:\…`/`X:/…` (also `X:\\…` from `{:?}`),
/// `\\…` (UNC, `\\?\`) and `/…` at a word start. Windows names may contain spaces, so a
/// path runs to a character Windows forbids in names (`"`, `<`, `>`, `|`), to `": "` / `"; "`
/// (as in `context: cause`) or to the end of the line. Over-redacting is fine.
pub fn redact_paths(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let boundary = |i: usize| i == 0 || matches!(c[i - 1], ' ' | '(' | '[' | '"' | '\'' | '=');
    let starts_path = |i: usize| {
        let at = |k: usize| c.get(i + k).copied();
        let drive =
            c[i].is_ascii_alphabetic() && at(1) == Some(':') && matches!(at(2), Some('\\' | '/'));
        let unc = c[i] == '\\' && at(1) == Some('\\');
        let unix = c[i] == '/' && at(1).is_some_and(|n| !n.is_whitespace() && n != '/');
        boundary(i) && (drive || unc || unix)
    };
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < c.len() {
        if !starts_path(i) {
            out.push(c[i]);
            i += 1;
            continue;
        }
        out.push_str("<path>");
        while i < c.len()
            && !matches!(c[i], '"' | '<' | '>' | '|')
            && !(matches!(c[i], ':' | ';') && c.get(i + 1) == Some(&' '))
        {
            i += 1;
        }
    }
    out
}

/// Log text for a hoppy-core error: its category only (I/O errors with their kind). The
/// message may hold peer-chosen text (offer JSON, file names, remote errors) or paths.
pub fn error_kind(e: &hoppy_core::Error) -> String {
    use hoppy_core::Error;
    match e {
        Error::Io(io) => format!("I/O error ({:?})", io.kind()),
        Error::Protocol(_) => "protocol error".into(),
        Error::Input(_) => "input error".into(),
        Error::Remote(_) => "remote error".into(),
        Error::Unreachable(_) => "peer unreachable".into(),
        Error::Crypto(_) => "encryption error".into(),
        Error::KeyChanged(_) => "peer key changed".into(),
        Error::VerificationFailed(_) => "verification failed".into(),
        Error::Unencrypted(_) => "not encrypted".into(),
    }
}

/// Log text for an `anyhow` chain: hoppy-core errors as `error_kind` (and nothing below
/// them), I/O errors as their kind, other messages as text (paths redacted on output).
pub fn error_chain(e: &anyhow::Error) -> String {
    let mut parts = Vec::new();
    for cause in e.chain() {
        if let Some(core) = cause.downcast_ref::<hoppy_core::Error>() {
            parts.push(error_kind(core));
            break;
        }
        match cause.downcast_ref::<io::Error>() {
            Some(io) => parts.push(format!("I/O error ({:?})", io.kind())),
            None => parts.push(cause.to_string()),
        }
    }
    parts.join(": ")
}

/// UTC `YYYY-MM-DD hh:mm:ss.mmmZ` without a date/time dependency.
pub fn utc_stamp(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil-from-days (H. Hinnant), valid for every date after 1970.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
    )
}

/// Appends `line`, first moving a file that would grow past `max` to `<name>.1`.
pub fn append(path: &Path, line: &str, max: u64) -> io::Result<()> {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > 0 && size + line.len() as u64 > max {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        let _ = fs::remove_file(&old);
        fs::rename(path, &old)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(line.as_bytes())
}

/// The last `lines` lines of the log (reads at most the last 64 KiB).
pub fn tail(path: &Path, lines: usize) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(64 * 1024);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let all: Vec<&str> = text.lines().collect();
    // A partial first line when we did not start at the beginning.
    let skip = usize::from(start > 0 && all.len() > 1);
    let body = &all[skip..];
    let from = body.len().saturating_sub(lines);
    Ok(body[from..].join("\n"))
}

/// What the UI thread is doing right now (`None` = waiting for messages or in a menu).
#[derive(Default)]
struct Busy {
    current: Option<(&'static str, Instant)>,
    /// The watchdog already reported `current`.
    reported: bool,
}

/// Times UI-thread handlers; see the module docs.
pub struct UiWatch {
    busy: Arc<Mutex<Busy>>,
}

impl UiWatch {
    /// Starts the watchdog thread; it ends when the `UiWatch` is dropped.
    pub fn start() -> UiWatch {
        let busy = Arc::new(Mutex::new(Busy::default()));
        let weak: Weak<Mutex<Busy>> = Arc::downgrade(&busy);
        thread::spawn(move || {
            while let Some(busy) = weak.upgrade() {
                let stuck = {
                    let mut b = busy.lock().unwrap_or_else(|e| e.into_inner());
                    match b.current {
                        Some((name, since)) if !b.reported && since.elapsed() >= STUCK => {
                            b.reported = true;
                            Some(name)
                        }
                        _ => None,
                    }
                };
                drop(busy);
                if let Some(name) = stuck {
                    log(format!(
                        "watchdog: UI thread busy in '{name}' for more than {} s",
                        STUCK.as_secs()
                    ));
                }
                thread::sleep(Duration::from_millis(500));
            }
        });
        UiWatch { busy }
    }

    fn set(&self, current: Option<(&'static str, Instant)>) {
        let mut b = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        b.current = current;
        b.reported = false;
    }

    /// Marks the UI thread busy with `name` until the returned guard is dropped.
    pub fn handler(&self, name: &'static str) -> Handler<'_> {
        let now = Instant::now();
        self.set(Some((name, now)));
        Handler {
            watch: self,
            name,
            started: now,
            modal: Duration::ZERO,
        }
    }
}

/// See `UiWatch::handler`.
pub struct Handler<'a> {
    watch: &'a UiWatch,
    name: &'static str,
    started: Instant,
    modal: Duration,
}

impl Handler<'_> {
    /// Runs a modal UI call (menu, dialog): the time the user spends in it is not "busy".
    pub fn modal<T>(&mut self, f: impl FnOnce() -> T) -> T {
        self.watch.set(None);
        let t = Instant::now();
        let out = f();
        self.modal += t.elapsed();
        self.watch.set(Some((self.name, Instant::now())));
        out
    }

    /// Time spent so far outside modal calls.
    pub fn busy(&self) -> Duration {
        self.started.elapsed().saturating_sub(self.modal)
    }
}

impl Drop for Handler<'_> {
    fn drop(&mut self) {
        self.watch.set(None);
        let busy = self.busy();
        if busy >= STUCK {
            log(format!(
                "watchdog: '{}' blocked the UI thread for {} ms",
                self.name,
                busy.as_millis()
            ));
        } else if busy >= SLOW_HANDLER {
            log(format!("ui: '{}' took {} ms", self.name, busy.as_millis()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_redacted() {
        let cases = [
            (
                r"Not a file: C:\Users\Mario Rossi\Downloads\foto 1.jpg",
                "Not a file: <path>",
            ),
            (
                r"reading C:\Users\M\AppData\device.json: Access is denied. (os error 5)",
                "reading <path>: Access is denied. (os error 5)",
            ),
            (r#"open "C:\\x\\a b.txt" failed"#, r#"open "<path>" failed"#),
            (r"\\?\D:\x and \\nas\share\y", "<path>"),
            (
                "cannot open /home/u/a b.txt: denied",
                "cannot open <path>: denied",
            ),
            (
                "opening https://hoppy.example/download failed, 1/2, TCP/IP",
                "opening https://hoppy.example/download failed, 1/2, TCP/IP",
            ),
            (
                "offer from 192.168.1.20 accepted=true",
                "offer from 192.168.1.20 accepted=true",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(redact_paths(input), want, "{input}");
        }
        let line = format_line(UNIX_EPOCH, 1, "send", r"failed: E:\secret.txt");
        assert!(line.ends_with("failed: <path>\n"), "{line}");
    }

    #[test]
    fn core_errors_are_logged_by_kind_only() {
        use hoppy_core::Error;
        let e = Error::Remote("hash mismatch: diary.txt".into());
        assert_eq!(error_kind(&e), "remote error");
        let io = io::Error::new(io::ErrorKind::NotFound, "secret.txt");
        assert_eq!(error_kind(&Error::Io(io)), "I/O error (NotFound)");
        let chain = anyhow::Error::new(Error::Input("x.txt is not a regular file".into()))
            .context("sending");
        assert_eq!(error_chain(&chain), "sending: input error");
        let chain = anyhow::Error::new(io::Error::other("x")).context(r"reading C:\a\b.json");
        assert_eq!(
            redact_paths(&error_chain(&chain)),
            "reading <path>: I/O error (Other)"
        );
    }

    #[test]
    fn stamps_are_utc_civil_dates() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "1970-01-01 00:00:00.000Z");
        let t = UNIX_EPOCH + Duration::from_millis(1_709_210_096_789); // leap day 2024
        assert_eq!(utc_stamp(t), "2024-02-29 12:34:56.789Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_798_761_599); // last second of 2026
        assert_eq!(utc_stamp(t), "2026-12-31 23:59:59.000Z");
    }

    #[test]
    fn lines_are_single_line() {
        let line = format_line(UNIX_EPOCH, 7, "tray", "a\nb\r\u{1b}c");
        assert_eq!(line, "1970-01-01 00:00:00.000Z [7 tray] a b  c\n");
    }

    #[test]
    fn rotates_and_tails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hoppy.log");
        for i in 0..10 {
            append(&path, &format!("line {i}\n"), 40).unwrap();
        }
        let current = fs::read_to_string(&path).unwrap();
        assert!(current.len() <= 40, "{current:?}");
        assert!(current.ends_with("line 9\n"));
        let backup = fs::read_to_string(dir.path().join("hoppy.log.1")).unwrap();
        assert!(backup.contains("line"));
        assert_eq!(tail(&path, 2).unwrap(), {
            let l: Vec<&str> = current.lines().collect();
            l[l.len() - 2..].join("\n")
        });
        assert_eq!(tail(&path, 1).unwrap(), "line 9");
    }

    #[test]
    fn modal_time_is_not_busy_time() {
        let watch = UiWatch::start();
        let mut h = watch.handler("test");
        h.modal(|| thread::sleep(Duration::from_millis(120)));
        assert!(h.busy() < Duration::from_millis(100), "{:?}", h.busy());
    }
}
