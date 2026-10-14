//! `hoppyw`: the Windows tray receiver and the "Send to → Hoppy" GUI.
//!
//! - `hoppyw` (no args): single instance, notification-area icon, receives into
//!   Downloads\Hoppy asking Yes/No for each offer, balloon when done.
//! - `hoppyw send FILE…`: searches nearby devices, shows a popup menu at the cursor, sends
//!   with progress in the tray tooltip, final balloon, then exits. Plays music unless muted
//!   (`music` in device.json): idle loop → transfer loop once accepted → done jingle.
//!   The receiver never plays music.
//! - Updates: the tray checks GitHub Releases 30 s after start and then daily (`update.rs`);
//!   "Update to X…" downloads the installer, verifies SHA256SUMS.txt, runs it silently and
//!   exits (the installer restarts the tray). Portable copies only link to the portal.
//!
//! Safe Rust only: every Win32 call goes through `platform::win` (a stub off Windows, so
//! this module is compiled and linted everywhere). Worker threads never touch the UI; they
//! update shared state and call `Waker::wake`, which the UI thread sees as `TrayEvent::Wake`.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};
use hoppy_core::protocol::TRANSFER_PORT;
use hoppy_core::transfer::serve;
use hoppy_core::{
    DeviceInfo, Discovery, Error, Offer, Peer, Progress, ReceiveHandler, SendOutcome, send_files,
};

use crate::platform::win::{self, Tray, Waker};
use crate::platform::{self, HttpSink, MenuItem, TooManyFiles, TrayEvent, Tune};
use crate::{
    DialogGuard, SHOW_TEXT_MAX, bind_receiver, config, default_receive_dir, display_file_name,
    display_name, display_platform, display_text, human, start_discovery, update,
};

/// Window titles double as identities: `signal_existing` looks for the tray one.
const TRAY_WINDOW: &str = "Hoppy tray";
const SEND_WINDOW: &str = "Hoppy send";
const INSTANCE_MUTEX: &str = r"Local\HoppyTray";

/// Peer search: give up after `SEARCH_MAX`, or `SEARCH_SETTLE` after the first answer.
pub const SEARCH_MAX: Duration = Duration::from_millis(2500);
pub const SEARCH_SETTLE: Duration = Duration::from_millis(800);
/// Keep the icon (and its balloon) alive this long after a send finishes.
const EXIT_AFTER_MS: u32 = 7000;
const EXIT_TIMER: usize = 1;

/// Every user-visible `hoppyw` string, Italian + English. Placeholders: `{name}` etc.,
/// filled by `fill` in a single pass.
pub struct Strings {
    pub tip_ready: &'static str,
    pub menu_header: &'static str,
    pub menu_send: &'static str,
    pub menu_open: &'static str,
    pub menu_music: &'static str,
    pub menu_quit: &'static str,
    pub pick_title: &'static str,
    pub already_running: &'static str,
    pub received_title: &'static str,
    pub received_text: &'static str,
    pub receive_failed: &'static str,
    pub incoming_title: &'static str,
    pub incoming_body: &'static str,
    pub incoming_more: &'static str,
    pub searching: &'static str,
    pub send_header: &'static str,
    pub no_devices: &'static str,
    pub retry: &'static str,
    pub cancel: &'static str,
    pub waiting: &'static str,
    pub sending: &'static str,
    pub delivered: &'static str,
    pub declined: &'static str,
    pub send_failed: &'static str,
    pub cancel_send: &'static str,
    pub close: &'static str,
    pub not_a_file: &'static str,
    pub no_files: &'static str,
    pub too_many_files: &'static str,
    pub usage: &'static str,
    pub update_ready: &'static str,
    pub update_portal: &'static str,
    pub menu_update: &'static str,
    pub menu_download: &'static str,
    pub update_progress: &'static str,
    pub update_failed: &'static str,
    pub update_mismatch: &'static str,
}

pub const IT: Strings = Strings {
    tip_ready: "Hoppy · {name}",
    menu_header: "Hoppy · ricevo come {name}",
    menu_send: "Invia file…",
    menu_open: "Apri cartella ricevuti",
    menu_music: "♪ Musica durante l'invio",
    menu_quit: "Esci",
    pick_title: "Hoppy — scegli i file da inviare",
    already_running: "Hoppy è già attivo: trovi l'icona vicino all'orologio.",
    received_title: "Ricevuti {n} file da {name}",
    received_text: "Clicca per aprire la cartella.",
    receive_failed: "Ricezione da {name} non riuscita",
    incoming_title: "Hoppy — file in arrivo",
    incoming_body: "{name} ({platform}, {ip}) vuole inviarti {n} file, {size}:",
    incoming_more: "… e altri {n}",
    searching: "Cerco dispositivi…",
    send_header: "Invia {n} file ({size}) a:",
    no_devices: "Nessun dispositivo trovato — apri Hoppy sull'altro dispositivo (stessa Wi-Fi)",
    retry: "Cerca di nuovo",
    cancel: "Annulla",
    waiting: "In attesa che {name} accetti…",
    sending: "Invio a {name}… {pct}%",
    delivered: "Consegnato a {name} ✓",
    declined: "{name} ha rifiutato",
    send_failed: "Invio a {name} non riuscito",
    cancel_send: "Annulla invio",
    close: "Chiudi",
    not_a_file: "«{path}» non è un file: le cartelle arriveranno in una versione futura.",
    no_files: "Nessun file da inviare.",
    too_many_files: "Troppi file selezionati: inviali in più volte.",
    usage: "Uso: hoppyw (icona vicino all'orologio) oppure hoppyw send FILE…",
    update_ready: "Hoppy {version} è disponibile — clic per aggiornare",
    update_portal: "Hoppy {version} è disponibile — clic per scaricarlo",
    menu_update: "Aggiorna a {version}…",
    menu_download: "Scarica Hoppy {version}…",
    update_progress: "Scarico aggiornamento… {pct}%",
    update_failed: "Aggiornamento non riuscito",
    update_mismatch: "Il file scaricato non corrisponde al checksum ufficiale: non è stato installato.",
};

pub const EN: Strings = Strings {
    tip_ready: "Hoppy · {name}",
    menu_header: "Hoppy · receiving as {name}",
    menu_send: "Send files…",
    menu_open: "Open received folder",
    menu_music: "♪ Music while sending",
    menu_quit: "Quit",
    pick_title: "Hoppy — choose files to send",
    already_running: "Hoppy is already running: look for its icon near the clock.",
    received_title: "Received {n} file(s) from {name}",
    received_text: "Click to open the folder.",
    receive_failed: "Receiving from {name} failed",
    incoming_title: "Hoppy — incoming files",
    incoming_body: "{name} ({platform}, {ip}) wants to send you {n} file(s), {size}:",
    incoming_more: "… and {n} more",
    searching: "Looking for devices…",
    send_header: "Send {n} file(s) ({size}) to:",
    no_devices: "No devices found — open Hoppy on the other device (same Wi-Fi)",
    retry: "Search again",
    cancel: "Cancel",
    waiting: "Waiting for {name} to accept…",
    sending: "Sending to {name}… {pct}%",
    delivered: "Delivered to {name} ✓",
    declined: "{name} declined",
    send_failed: "Sending to {name} failed",
    cancel_send: "Cancel sending",
    close: "Close",
    not_a_file: "\"{path}\" is not a file: folders arrive in a later version.",
    no_files: "Nothing to send.",
    too_many_files: "Too many files selected: send them in several batches.",
    usage: "Usage: hoppyw (icon near the clock) or hoppyw send FILE…",
    update_ready: "Hoppy {version} is available — click to update",
    update_portal: "Hoppy {version} is available — click to download it",
    menu_update: "Update to {version}…",
    menu_download: "Download Hoppy {version}…",
    update_progress: "Downloading update… {pct}%",
    update_failed: "Update failed",
    update_mismatch: "The downloaded file does not match the official checksum: it was not installed.",
};

pub fn strings() -> &'static Strings {
    if platform::ui_is_italian() { &IT } else { &EN }
}

/// Replaces `{key}` placeholders in one pass (values are never re-scanned, so a device
/// called "{n}" stays literal). Unknown placeholders are kept as-is.
pub fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let value = tail.find('}').and_then(|end| {
            let key = &tail[1..end];
            args.iter().find(|(k, _)| *k == key).map(|(_, v)| (*v, end))
        });
        match value {
            Some((v, end)) => {
                out.push_str(v);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// "Pixel 8 · Android" (network text cleaned by `display_name`/`display_platform`).
pub fn peer_label(peer: &Peer) -> String {
    let platform = match peer.device.platform.as_str() {
        "android" => "Android".to_owned(),
        "windows" => "Windows".to_owned(),
        "linux" => "Linux".to_owned(),
        "macos" => "macOS".to_owned(),
        "ios" => "iOS".to_owned(),
        other => display_platform(other),
    };
    format!("{} · {platform}", display_name(&peer.device.name))
}

/// Waits up to `max` for peers, returning early `settle` after the first one appears.
pub fn collect_peers(discovery: &Discovery, max: Duration, settle: Duration) -> Vec<Peer> {
    let start = Instant::now();
    let mut first_seen: Option<Instant> = None;
    loop {
        let peers = discovery.peers();
        let now = Instant::now();
        if !peers.is_empty() && first_seen.is_none() {
            first_seen = Some(now);
        }
        let settled = first_seen.is_some_and(|t| now.duration_since(t) >= settle);
        if settled || now.duration_since(start) >= max {
            return peers;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Body of the Yes/No dialog for an incoming offer. Everything from the network is
/// cleaned first: names via `display_*`, file names as the §4 sanitizer will save them.
pub fn offer_body(s: &Strings, offer: &Offer, from: SocketAddr) -> String {
    let n = offer.files.len().to_string();
    let mut body = fill(
        s.incoming_body,
        &[
            ("name", &display_name(&offer.sender.name)),
            ("platform", &display_platform(&offer.sender.platform)),
            ("ip", &from.ip().to_string()),
            ("n", &n),
            ("size", &human(offer.total_size())),
        ],
    );
    for f in offer.files.iter().take(5) {
        body.push_str("\n• ");
        body.push_str(&display_file_name(&f.name));
    }
    let more = offer.files.len().saturating_sub(5);
    if more > 0 {
        body.push('\n');
        body.push_str(&fill(s.incoming_more, &[("n", &more.to_string())]));
    }
    body
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Entry point of `hoppyw`; returns the process exit code.
pub fn run(args: Vec<OsString>) -> i32 {
    let s = strings();
    let result = match args.first().map(|a| a.to_str()) {
        None => tray_mode(s),
        Some(Some("send")) => send_mode(s, args[1..].iter().map(PathBuf::from).collect()),
        Some(_) => Err(anyhow::anyhow!("{}", s.usage)),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            win::message_box_error("Hoppy", &format!("{e:#}"));
            1
        }
    }
}

// ---------------------------------------------------------------- tray receiver

struct Notice {
    title: String,
    text: String,
    error: bool,
}

struct GuiReceiver {
    s: &'static Strings,
    waker: Waker,
    notices: Arc<Mutex<VecDeque<Notice>>>,
    /// Sender names by connection, remembered at accept time for the final balloon.
    names: Mutex<HashMap<SocketAddr, String>>,
    /// One accept dialog at a time; offers arriving meanwhile are declined.
    asking: AtomicBool,
}

impl ReceiveHandler for GuiReceiver {
    fn decide(&self, offer: &Offer, from: SocketAddr) -> bool {
        let Some(_asking) = DialogGuard::try_take(&self.asking) else {
            return false; // another offer is waiting for an answer
        };
        let body = offer_body(self.s, offer, from);
        let accepted = platform::ask_accept(self.s.incoming_title, &body);
        if accepted {
            lock(&self.names).insert(from, display_name(&offer.sender.name));
        }
        accepted
    }

    fn finished(&self, from: SocketAddr, result: &Result<Vec<PathBuf>, Error>) {
        let name = lock(&self.names)
            .remove(&from)
            .unwrap_or_else(|| from.ip().to_string());
        let notice = match result {
            Ok(paths) => Notice {
                title: fill(
                    self.s.received_title,
                    &[("n", &paths.len().to_string()), ("name", &name)],
                ),
                text: self.s.received_text.to_owned(),
                error: false,
            },
            Err(e) => Notice {
                title: fill(self.s.receive_failed, &[("name", &name)]),
                text: display_text(&e.to_string(), SHOW_TEXT_MAX),
                error: true,
            },
        };
        lock(&self.notices).push_back(notice);
        self.waker.wake();
    }
}

const ID_SEND: u32 = 1;
const ID_OPEN: u32 = 2;
const ID_QUIT: u32 = 3;
const ID_MUSIC: u32 = 4;
const ID_UPDATE: u32 = 5;

/// What clicking the last balloon does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BalloonAction {
    Nothing,
    OpenDir,
    Update,
    Portal,
}

fn tray_mode(s: &'static Strings) -> Result<()> {
    let Some(_instance) = win::single_instance(INSTANCE_MUTEX) else {
        win::signal_existing(TRAY_WINDOW);
        return Ok(());
    };
    let identity = config::load_or_create()?;
    let me = config::device_info(&identity);
    let dir = default_receive_dir()?;
    let listener = bind_receiver(TRANSFER_PORT)?;
    let _discovery = start_discovery(&me, listener.local_addr()?.port(), &[])?;

    let tray = Tray::new(TRAY_WINDOW, &fill(s.tip_ready, &[("name", &me.name)]))?;
    let notices = Arc::new(Mutex::new(VecDeque::new()));
    let handler = Arc::new(GuiReceiver {
        s,
        waker: tray.waker(),
        notices: Arc::clone(&notices),
        names: Mutex::new(HashMap::new()),
        asking: AtomicBool::new(false),
    });
    let serve_dir = dir.clone();
    thread::spawn(move || serve(listener, serve_dir, handler));

    let updates = Arc::new(Mutex::new(Updates::default()));
    let mode = update_mode();
    if mode != UpdateMode::Off {
        spawn_update_checker(&updates, tray.waker());
    }

    let ready_tip = fill(s.tip_ready, &[("name", &me.name)]);
    let header = fill(s.menu_header, &[("name", &me.name)]);
    let mut balloon_action = BalloonAction::Nothing;
    while let Some(event) = tray.next_event() {
        match event {
            TrayEvent::Click => {
                // Re-read: `hoppy music on|off` may have changed it meanwhile.
                let music = config::load_or_create().map_or(true, |i| i.music);
                let update_item = update_menu_text(s, &lock(&updates), mode);
                let mut items = Vec::new();
                if let Some((clickable, text)) = &update_item {
                    items.push(if *clickable {
                        MenuItem::Bold(ID_UPDATE, text)
                    } else {
                        MenuItem::Label(text)
                    });
                    items.push(MenuItem::Separator);
                }
                items.extend([
                    MenuItem::Label(&header),
                    MenuItem::Action(ID_SEND, s.menu_send),
                    MenuItem::Action(ID_OPEN, s.menu_open),
                    MenuItem::Check(ID_MUSIC, s.menu_music, music),
                    MenuItem::Separator,
                    MenuItem::Action(ID_QUIT, s.menu_quit),
                ]);
                match tray.popup_menu(&items, true) {
                    Some(ID_UPDATE) => start_update(s, &updates, mode, tray.waker()),
                    Some(ID_SEND) => match tray.pick_files(s.pick_title) {
                        Ok(files) if files.is_empty() => {}
                        Ok(files) => {
                            if let Err(e) = spawn_sender(&files) {
                                tray.balloon("Hoppy", &format!("{e:#}"), true, true);
                            }
                        }
                        Err(TooManyFiles) => win::message_box_error("Hoppy", s.too_many_files),
                    },
                    Some(ID_OPEN) => open_dir(&dir),
                    Some(ID_MUSIC) => {
                        if let Err(e) = config::set_music(!music) {
                            tray.balloon("Hoppy", &format!("{e:#}"), true, true);
                        }
                    }
                    Some(ID_QUIT) => break,
                    _ => {}
                }
            }
            TrayEvent::Wake => {
                loop {
                    // Pop first so the lock is not held during the UI call.
                    let next = lock(&notices).pop_front();
                    let Some(n) = next else { break };
                    // System sound on: this is the "files arrived" notification.
                    tray.balloon(&n.title, &n.text, n.error, true);
                    balloon_action = if n.error {
                        BalloonAction::Nothing
                    } else {
                        BalloonAction::OpenDir
                    };
                }
                let upd = update_wake(&tray, s, &updates, mode, &ready_tip);
                if let Some(action) = upd.balloon {
                    balloon_action = action;
                }
                if let Some(path) = upd.launch {
                    match Command::new(&path).args(update::INSTALLER_ARGS).spawn() {
                        // The installer stops running copies anyway; leave cleanly first
                        // (dropping `tray` removes the icon) and let it restart us.
                        Ok(_) => break,
                        Err(e) => {
                            let text = display_text(&e.to_string(), SHOW_TEXT_MAX);
                            tray.set_tip(&ready_tip);
                            tray.balloon(s.update_failed, &text, true, true);
                            balloon_action = BalloonAction::Nothing;
                        }
                    }
                }
            }
            TrayEvent::BalloonClick => match balloon_action {
                BalloonAction::OpenDir => open_dir(&dir),
                BalloonAction::Update => start_update(s, &updates, mode, tray.waker()),
                BalloonAction::Portal => {
                    win::open_url(update::PORTAL_URL);
                }
                BalloonAction::Nothing => {}
            },
            TrayEvent::Reopen => tray.balloon("Hoppy", s.already_running, false, true),
            _ => {}
        }
    }
    // Locals drop in reverse order: the tray icon goes first, then discovery says "bye".
    Ok(())
}

// ---------------------------------------------------------------- updates

#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdateMode {
    /// Dev build: never check.
    Off,
    /// Installed by Hoppy-Setup: download, verify and run the installer.
    Setup,
    /// Portable copy: only point to the download portal.
    Portal,
}

fn update_mode() -> UpdateMode {
    if cfg!(debug_assertions) || update::is_dev_version(update::CURRENT_VERSION) {
        return UpdateMode::Off;
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.to_string_lossy().into_owned()));
    let local = std::env::var_os("LOCALAPPDATA").map(|l| l.to_string_lossy().into_owned());
    match (exe_dir, local) {
        (Some(dir), Some(local)) if update::is_setup_install(&dir, &local) => UpdateMode::Setup,
        _ => UpdateMode::Portal,
    }
}

/// Shared between the update threads and the UI thread.
#[derive(Default)]
struct Updates {
    available: Option<update::Release>,
    /// `available` changed and has not been announced with a balloon yet.
    fresh: bool,
    /// Some(percent) while the installer is downloading.
    downloading: Option<u64>,
    /// The download thread's result: the verified installer, or a message to show.
    finished: Option<Result<PathBuf, String>>,
}

struct WinHttp;

impl update::Http for WinHttp {
    fn get(&self, url: &str, sink: &mut HttpSink<'_>) -> Result<()> {
        win::http_get(url, update::USER_AGENT, sink)
    }
}

/// Checks after `FIRST_CHECK`, then once per `CHECK_EVERY` of wall-clock time (hourly
/// ticks, so a PC that sleeps a lot still checks daily). Network errors are ignored.
fn spawn_update_checker(updates: &Arc<Mutex<Updates>>, waker: Waker) {
    let updates = Arc::clone(updates);
    thread::spawn(move || {
        thread::sleep(update::FIRST_CHECK);
        loop {
            if let Ok(Some(release)) = update::check(&WinHttp, update::CURRENT_VERSION) {
                let mut u = lock(&updates);
                if u.available.as_ref() != Some(&release) && u.downloading.is_none() {
                    u.available = Some(release);
                    u.fresh = true;
                    drop(u);
                    waker.wake();
                }
            }
            let checked = SystemTime::now();
            loop {
                thread::sleep(Duration::from_secs(3600));
                let elapsed = checked.elapsed().unwrap_or(update::CHECK_EVERY);
                if elapsed >= update::CHECK_EVERY {
                    break;
                }
            }
        }
    });
}

/// Top menu entry: (clickable, text). Bold "Update to X…", or the progress while
/// downloading.
fn update_menu_text(s: &Strings, u: &Updates, mode: UpdateMode) -> Option<(bool, String)> {
    if let Some(pct) = u.downloading {
        return Some((false, fill(s.update_progress, &[("pct", &pct.to_string())])));
    }
    let version = u.available.as_ref()?.version.to_string();
    let template = match mode {
        UpdateMode::Setup => s.menu_update,
        UpdateMode::Portal => s.menu_download,
        UpdateMode::Off => return None,
    };
    Some((true, fill(template, &[("version", &version)])))
}

/// Starts the download (Setup) or opens the portal (portable). One download at a time.
fn start_update(
    s: &'static Strings,
    updates: &Arc<Mutex<Updates>>,
    mode: UpdateMode,
    waker: Waker,
) {
    if mode == UpdateMode::Portal {
        win::open_url(update::PORTAL_URL);
        return;
    }
    let release = {
        let mut u = lock(updates);
        if mode != UpdateMode::Setup || u.downloading.is_some() {
            return;
        }
        let Some(release) = u.available.clone() else {
            return;
        };
        u.downloading = Some(0);
        release
    };
    waker.wake(); // show 0% now
    let updates = Arc::clone(updates);
    thread::spawn(move || {
        let dir = std::env::temp_dir();
        let result = update::download(&WinHttp, &release, &dir, &mut |pct| {
            let mut u = lock(&updates);
            if u.downloading != Some(pct) {
                u.downloading = Some(pct);
                drop(u);
                waker.wake(); // at most ~100 wake-ups
            }
        });
        let result = result.map_err(|e| {
            if e.is::<update::HashMismatch>() {
                s.update_mismatch.to_owned()
            } else {
                display_text(&format!("{e:#}"), SHOW_TEXT_MAX)
            }
        });
        lock(&updates).finished = Some(result);
        waker.wake();
    });
}

#[derive(Default)]
struct UpdateWake {
    /// New action for clicks on the balloon just shown.
    balloon: Option<BalloonAction>,
    /// Verified installer to run now.
    launch: Option<PathBuf>,
}

/// UI-thread side of `Wake` for updates: announce, show progress, report the result.
fn update_wake(
    tray: &Tray,
    s: &Strings,
    updates: &Mutex<Updates>,
    mode: UpdateMode,
    ready_tip: &str,
) -> UpdateWake {
    let (announce, pct, finished) = {
        let mut u = lock(updates);
        let announce = std::mem::take(&mut u.fresh)
            .then(|| u.available.as_ref().map(|r| r.version.to_string()))
            .flatten();
        let finished = u.finished.take();
        if finished.is_some() {
            u.downloading = None;
        }
        (announce, u.downloading, finished)
    };
    let mut out = UpdateWake::default();
    if let Some(version) = announce {
        let (template, action) = match mode {
            UpdateMode::Setup => (s.update_ready, BalloonAction::Update),
            _ => (s.update_portal, BalloonAction::Portal),
        };
        tray.balloon(&fill(template, &[("version", &version)]), "", false, true);
        out.balloon = Some(action);
    }
    match finished {
        Some(Ok(path)) => out.launch = Some(path),
        Some(Err(text)) => {
            tray.set_tip(ready_tip);
            tray.balloon(s.update_failed, &text, true, true);
            out.balloon = Some(BalloonAction::Nothing);
        }
        None => {
            if let Some(pct) = pct {
                tray.set_tip(&fill(s.update_progress, &[("pct", &pct.to_string())]));
            }
        }
    }
    out
}

fn open_dir(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    win::open_folder(dir);
}

/// Runs `hoppyw send FILE…` as a separate process (one tray icon per transfer).
fn spawn_sender(files: &[PathBuf]) -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut child = Command::new(exe).arg("send").args(files).spawn()?;
    thread::spawn(move || child.wait());
    Ok(())
}

// ---------------------------------------------------------------- "Send to → Hoppy"

/// Sender music; stops when dropped (before the process exits).
struct Music {
    enabled: bool,
    playing: Option<Tune>,
}

impl Music {
    fn new(enabled: bool) -> Music {
        Music {
            enabled,
            playing: None,
        }
    }

    /// Starts `tune` unless muted or already playing (restarting would skip the loop).
    fn play(&mut self, tune: Tune) {
        if self.enabled && self.playing != Some(tune) {
            win::play_tune(tune);
            self.playing = Some(tune);
        }
    }

    fn stop(&mut self) {
        if self.playing.take().is_some() {
            win::stop_tune();
        }
    }
}

impl Drop for Music {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Default)]
struct SendState {
    peers: Option<Vec<Peer>>,
    /// The receiver said yes (first `Progress::FileStarted`).
    accepted: bool,
    pct: Option<u64>,
    outcome: Option<Result<SendOutcome, String>>,
}

enum Phase {
    Searching,
    Sending(String),
    Done,
}

const ID_RETRY: u32 = 1;
const ID_CANCEL: u32 = 2;
const ID_PEER: u32 = 100;

fn send_mode(s: &'static Strings, files: Vec<PathBuf>) -> Result<()> {
    if files.is_empty() {
        bail!("{}", s.no_files);
    }
    if let Some(f) = files.iter().find(|f| !f.is_file()) {
        bail!(
            "{}",
            fill(s.not_a_file, &[("path", &f.display().to_string())])
        );
    }
    let total: u64 = files
        .iter()
        .filter_map(|f| f.metadata().ok())
        .map(|m| m.len())
        .sum();
    let identity = config::load_or_create()?;
    let me = config::device_info(&identity);
    let discovery = Arc::new(start_discovery(&me, 0, &[])?);

    let tray = Tray::new(SEND_WINDOW, s.searching)?;
    // Declared after `tray`, so it is dropped (music stopped) before the icon goes.
    let mut music = Music::new(identity.music);
    music.play(Tune::Idle);
    let state = Arc::new(Mutex::new(SendState::default()));
    spawn_search(&discovery, &state, tray.waker());

    let header = fill(
        s.send_header,
        &[("n", &files.len().to_string()), ("size", &human(total))],
    );
    let mut phase = Phase::Searching;
    let mut files = Some(files);
    while let Some(event) = tray.next_event() {
        match (event, &phase) {
            (TrayEvent::Wake, Phase::Searching) => {
                let Some(peers) = lock(&state).peers.take() else {
                    continue;
                };
                let labels: Vec<String> = peers.iter().map(peer_label).collect();
                let mut items = vec![MenuItem::Label(&header), MenuItem::Separator];
                if peers.is_empty() {
                    items.push(MenuItem::Label(s.no_devices));
                }
                for (i, label) in labels.iter().enumerate() {
                    items.push(MenuItem::Action(ID_PEER + i as u32, label));
                }
                items.extend([
                    MenuItem::Separator,
                    MenuItem::Action(ID_RETRY, s.retry),
                    MenuItem::Action(ID_CANCEL, s.cancel),
                ]);
                match tray.popup_menu(&items, false) {
                    Some(ID_RETRY) => {
                        tray.set_tip(s.searching);
                        discovery.refresh();
                        spawn_search(&discovery, &state, tray.waker());
                    }
                    Some(id) if id >= ID_PEER => {
                        let Some(peer) = peers.get((id - ID_PEER) as usize).cloned() else {
                            break;
                        };
                        let name = display_name(&peer.device.name);
                        tray.set_tip(&fill(s.waiting, &[("name", &name)]));
                        let files = files.take().unwrap_or_default();
                        spawn_send(me.clone(), peer, files, &state, tray.waker());
                        phase = Phase::Sending(name);
                    }
                    _ => break, // "Cancel" or menu dismissed
                }
            }
            (TrayEvent::Wake, Phase::Sending(name)) => {
                let (accepted, pct, outcome) = {
                    let mut st = lock(&state);
                    (st.accepted, st.pct, st.outcome.take())
                };
                if let Some(outcome) = outcome {
                    let delivered = matches!(outcome, Ok(SendOutcome::Delivered));
                    if delivered {
                        music.play(Tune::Done); // replaces the loop
                    } else {
                        music.stop();
                    }
                    // Our jingle is the sound of success; skip the system one on top of it.
                    let sound = !(delivered && music.enabled);
                    let (title, text, error) = match outcome {
                        Ok(SendOutcome::Delivered) => (s.delivered, String::new(), false),
                        Ok(SendOutcome::Declined(reason)) => {
                            // "declined" = the user said No; other reasons are protocol limits.
                            let reason = reason.filter(|r| r != "declined").unwrap_or_default();
                            (s.declined, display_text(&reason, SHOW_TEXT_MAX), true)
                        }
                        Err(e) => (s.send_failed, display_text(&e, SHOW_TEXT_MAX), true),
                    };
                    let title = fill(title, &[("name", name)]);
                    tray.set_tip(&title);
                    tray.balloon(&title, &text, error, sound);
                    tray.set_timer(EXIT_TIMER, EXIT_AFTER_MS);
                    phase = Phase::Done;
                } else {
                    if accepted {
                        music.play(Tune::Transfer);
                    }
                    if let Some(pct) = pct {
                        let pct = pct.to_string();
                        tray.set_tip(&fill(s.sending, &[("name", name), ("pct", &pct)]));
                    }
                }
            }
            (TrayEvent::Click, Phase::Sending(_)) => {
                let items = [MenuItem::Action(ID_CANCEL, s.cancel_send)];
                if tray.popup_menu(&items, true) == Some(ID_CANCEL) {
                    break; // closing the process aborts the transfer
                }
            }
            (TrayEvent::Click, Phase::Done) => {
                let items = [MenuItem::Action(ID_CANCEL, s.close)];
                if tray.popup_menu(&items, true) == Some(ID_CANCEL) {
                    break;
                }
            }
            (TrayEvent::Timer(EXIT_TIMER), _) => {
                tray.kill_timer(EXIT_TIMER);
                break;
            }
            _ => {}
        }
    }
    Ok(()) // dropping `tray` removes the icon
}

fn spawn_search(discovery: &Arc<Discovery>, state: &Arc<Mutex<SendState>>, waker: Waker) {
    let (discovery, state) = (Arc::clone(discovery), Arc::clone(state));
    thread::spawn(move || {
        let peers = collect_peers(&discovery, SEARCH_MAX, SEARCH_SETTLE);
        lock(&state).peers = Some(peers);
        waker.wake();
    });
}

fn spawn_send(
    me: DeviceInfo,
    peer: Peer,
    files: Vec<PathBuf>,
    state: &Arc<Mutex<SendState>>,
    waker: Waker,
) {
    let state = Arc::clone(state);
    thread::spawn(move || {
        let mut last = None;
        let result = send_files(peer.addr, &me, &files, &mut |p| match p {
            // Only sent after the receiver accepted: switch to the transfer music.
            Progress::FileStarted { index: 0, .. } => {
                lock(&state).accepted = true;
                waker.wake();
            }
            Progress::Bytes { done, total } => {
                let pct = done.saturating_mul(100).checked_div(total).unwrap_or(100);
                if last != Some(pct) {
                    last = Some(pct);
                    lock(&state).pct = Some(pct);
                    waker.wake(); // at most ~100 wake-ups per transfer
                }
            }
            _ => {}
        });
        lock(&state).outcome = Some(result.map_err(|e| e.to_string()));
        waker.wake();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoppy_core::DiscoveryConfig;

    fn placeholders(t: &str) -> Vec<&str> {
        let mut keys: Vec<&str> = t
            .split('{')
            .skip(1)
            .filter_map(|p| p.split_once('}').map(|(k, _)| k))
            .collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn italian_and_english_use_the_same_placeholders() {
        let pairs = |s: &Strings| {
            [
                s.tip_ready,
                s.menu_header,
                s.received_title,
                s.receive_failed,
                s.incoming_body,
                s.incoming_more,
                s.send_header,
                s.waiting,
                s.sending,
                s.delivered,
                s.declined,
                s.send_failed,
                s.not_a_file,
                s.update_ready,
                s.update_portal,
                s.menu_update,
                s.menu_download,
                s.update_progress,
            ]
        };
        for (it, en) in pairs(&IT).iter().zip(pairs(&EN)) {
            assert_eq!(placeholders(it), placeholders(en), "{it} / {en}");
        }
    }

    #[test]
    fn fill_is_single_pass() {
        assert_eq!(
            fill(
                "Invio a {name}… {pct}%",
                &[("name", "{pct}"), ("pct", "42")]
            ),
            "Invio a {pct}… 42%"
        );
        assert_eq!(fill("{a} {missing} {", &[("a", "x")]), "x {missing} {");
    }

    #[test]
    fn peer_labels() {
        let peer = Peer {
            device: DeviceInfo {
                id: "x".into(),
                name: "Pixel & co".into(),
                platform: "android".into(),
            },
            addr: "10.0.0.2:47471".parse().unwrap(),
        };
        assert_eq!(peer_label(&peer), "Pixel & co · Android");
        let spoof = Peer {
            device: DeviceInfo {
                id: "y".into(),
                name: "Mario\u{202E}\u{0}".into(),
                platform: "\u{2066}toaster".into(),
            },
            ..peer
        };
        assert_eq!(peer_label(&spoof), "Mario · toaster");
    }

    #[test]
    fn offer_body_shows_cleaned_text() {
        let file = |name: &str| hoppy_core::protocol::FileEntry {
            name: name.into(),
            size: 1,
        };
        let offer = Offer {
            sender: DeviceInfo {
                id: "x".into(),
                name: "Pixel\u{202E}\n".into(),
                platform: "android\u{200F}".into(),
            },
            files: vec![file("../../a\u{202E}gpj.exe"), file("..")],
        };
        let body = offer_body(&EN, &offer, "10.0.0.9:1".parse().unwrap());
        assert_eq!(
            body,
            "Pixel (android, 10.0.0.9) wants to send you 2 file(s), 2 B:\n• agpj.exe\n• (?)"
        );
    }

    fn quiet_discovery(name: &str, transfer_port: u16, seed: Option<u16>) -> Discovery {
        let me = DeviceInfo {
            id: format!("id-{name}"),
            name: name.into(),
            platform: "linux".into(),
        };
        let mut cfg = DiscoveryConfig::new(me, transfer_port);
        cfg.bind_port = 0;
        cfg.use_multicast = false;
        cfg.seeds = seed
            .map(|p| vec![SocketAddr::from(([127, 0, 0, 1], p))])
            .unwrap_or_default();
        Discovery::start(cfg).unwrap()
    }

    #[test]
    fn collect_peers_times_out_when_alone() {
        let alone = quiet_discovery("alone", 0, None);
        let t = Instant::now();
        let peers = collect_peers(&alone, Duration::from_millis(300), SEARCH_SETTLE);
        assert!(peers.is_empty());
        assert!(t.elapsed() >= Duration::from_millis(300));
    }

    #[test]
    fn collect_peers_returns_soon_after_the_first_peer() {
        let receiver = quiet_discovery("receiver", 47999, None);
        let sender = quiet_discovery("sender", 0, Some(receiver.local_port()));
        let t = Instant::now();
        let peers = collect_peers(&sender, Duration::from_secs(10), Duration::from_millis(200));
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device.name, "receiver");
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}
