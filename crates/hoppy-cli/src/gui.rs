//! `hoppyw`: the Windows tray receiver and the "Send with Hoppy" / "Send to → Hoppy" GUI.
//!
//! - `hoppyw` (no args): single instance, notification-area icon, receives into
//!   Downloads\Hoppy asking Yes/No for each offer, balloon when done.
//! - `hoppyw send FILE…`: searches nearby devices, shows a popup menu at the cursor, sends
//!   with progress in the tray tooltip, final balloon, then exits. Plays music unless muted
//!   (`music` in device.json): idle loop → transfer loop once accepted → done jingle.
//!   The receiver never plays music. Explorer's "Send with Hoppy" verb starts one instance
//!   per selected file: the first collects the others' files (`platform::gather_send_files`).
//!   Without files (desktop shortcut double-clicked) it opens the file picker first.
//! - Updates: the tray checks GitHub Releases 30 s after start and then daily (`update.rs`);
//!   "Update to X…" downloads the installer, verifies SHA256SUMS.txt, runs it silently and
//!   exits (the installer restarts the tray). Portable copies only link to the portal.
//! - Firewall (`firewall.rs`): the tray checks at start whether other devices can reach it;
//!   if not, a balloon (once per version) and a menu item offer to add the rules (UAC →
//!   `hoppyw firewall --apply`) or to set a Public network to Private.
//! - `hoppyw firewall --apply|--remove`: no UI, exit code only (started elevated by the tray
//!   and the installer).
//!
//! Safe Rust only: every Win32 call goes through `platform::win` (a stub off Windows, so
//! this module is compiled and linted everywhere). Worker threads never touch the UI; they
//! update shared state and call `Waker::wake`, which the UI thread sees as `TrayEvent::Wake`.
//!
//! The UI thread never does file, registry or network I/O and never waits on a lock that
//! is held across I/O: device.json is cached in `Prefs` (re-read by a worker on each menu
//! open), and opening folders, starting processes, UAC prompts and config writes all run on
//! workers. Handlers are timed (`diag::UiWatch`); see `diag.rs` for the log.

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
    Discovery, Error, Offer, Peer, Progress, ReceiveHandler, SendOutcome, accept_callbacks,
    send_to_peer,
};

use crate::firewall::{self, Verdict};
use crate::platform::win::{self, Tray, Waker};
use crate::platform::{self, ElevationDeclined, HttpSink, MenuItem, TooManyFiles, TrayEvent, Tune};
use crate::{
    DialogGuard, SHOW_TEXT_MAX, bind_receiver, config, default_receive_dir, diag,
    display_file_name, display_name, display_platform, display_text, human, start_discovery,
    update,
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
    pub fallback: &'static str,
    pub unreachable_title: &'static str,
    pub unreachable_text: &'static str,
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
    pub fw_title: &'static str,
    pub fw_text: &'static str,
    pub public_title: &'static str,
    pub public_text: &'static str,
    pub menu_firewall: &'static str,
    pub menu_network: &'static str,
    pub fw_done: &'static str,
    pub fw_failed: &'static str,
    pub fw_policy: &'static str,
    pub menu_classic: &'static str,
    pub menu_about: &'static str,
    pub restart_explorer: &'static str,
    pub classic_failed: &'static str,
    pub about_failed: &'static str,
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
    fallback: "{name} non risponde, provo un'altra strada…",
    unreachable_title: "Non riesco a raggiungere «{name}»",
    unreachable_text: "Controlla che Hoppy sia aperto sull'altro dispositivo e che siate sulla stessa rete. Se è un PC Windows: lì clic sull'icona di Hoppy → «Consenti nel firewall…» (su un PC aziendale chiedi all'IT).",
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
    fw_title: "Gli altri dispositivi potrebbero non riuscire a inviarti file",
    fw_text: "Clic per consentire Hoppy nel firewall di Windows (serve la conferma di un amministratore).",
    public_title: "La rete è impostata come Pubblica",
    public_text: "Gli altri dispositivi non possono inviarti file: clic per impostarla come Privata.",
    menu_firewall: "Consenti nel firewall…",
    menu_network: "Imposta la rete come Privata…",
    fw_done: "Hoppy è consentito nel firewall ✓",
    fw_failed: "Impossibile consentire Hoppy nel firewall",
    fw_policy: "Le regole sono state aggiunte ma non hanno effetto: probabilmente un criterio aziendale le ignora. Chiedi all'IT di consentire Hoppy (TCP 47471, UDP 47470).",
    menu_classic: "«Invia con Hoppy» direttamente nel tasto destro (menu classico)",
    menu_about: "Informazioni e diagnostica…",
    restart_explorer: "Il nuovo menu del tasto destro compare dopo il riavvio di Esplora risorse (o al prossimo accesso).\n\nRiavviare Esplora risorse ora? Le finestre di Esplora file aperte verranno chiuse.",
    classic_failed: "Impossibile cambiare il menu del tasto destro",
    about_failed: "Impossibile aprire il registro di diagnostica",
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
    fallback: "{name} is not answering, trying another way…",
    unreachable_title: "Can't reach \"{name}\"",
    unreachable_text: "Check that Hoppy is open on the other device and that you are on the same network. If it is a Windows PC: click the Hoppy icon there → \"Allow through firewall…\" (on a company PC, ask IT).",
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
    fw_title: "Other devices may not be able to send you files",
    fw_text: "Click to allow Hoppy through Windows Firewall (needs administrator approval).",
    public_title: "This network is set to Public",
    public_text: "Other devices cannot send you files: click to set it to Private.",
    menu_firewall: "Allow through firewall…",
    menu_network: "Set the network to Private…",
    fw_done: "Hoppy is allowed through the firewall ✓",
    fw_failed: "Could not allow Hoppy through the firewall",
    fw_policy: "The rules were added but have no effect: a company policy probably ignores them. Ask IT to allow Hoppy (TCP 47471, UDP 47470).",
    menu_classic: "\"Send with Hoppy\" directly on right-click (classic menu)",
    menu_about: "About & diagnostics…",
    restart_explorer: "The new right-click menu appears after File Explorer restarts (or at the next sign-in).\n\nRestart File Explorer now? Open File Explorer windows will close.",
    classic_failed: "Could not change the right-click menu",
    about_failed: "Could not open the diagnostic log",
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
    let mode = args.first().map(|a| a.to_str());
    // Not for `firewall`: the elevated copy may run as another (admin) user.
    match mode {
        None => diag::init("tray"),
        Some(Some("send")) => diag::init("send"),
        _ => {}
    }
    diag::log(format!(
        "hoppyw {} started, {} argument(s)",
        update::CURRENT_VERSION,
        args.len()
    ));
    let result = match mode {
        None => tray_mode(s),
        Some(Some("send")) => send_mode(s, args[1..].iter().map(PathBuf::from).collect()),
        Some(Some("firewall")) => return firewall_mode(&args[1..]),
        Some(_) => Err(anyhow::anyhow!("{}", s.usage)),
    };
    let code = match result {
        Ok(()) => 0,
        Err(e) => {
            diag::log(format!("exit with error: {e:#}"));
            win::message_box_error("Hoppy", &format!("{e:#}"));
            1
        }
    };
    diag::log("exit");
    diag::flush(Duration::from_millis(500));
    code
}

/// `hoppyw firewall --apply|--remove` (run elevated by the tray and the installer) and
/// `--check` (installer, see `firewall::check_code`). No UI, since the installer may be
/// silent: the exit code is the result.
fn firewall_mode(args: &[OsString]) -> i32 {
    let exe = std::env::current_exe().ok();
    let Some(dir) = exe.as_deref().and_then(|e| e.parent()) else {
        return 3;
    };
    let result = match args.first().and_then(|a| a.to_str()) {
        Some("--apply") => firewall::apply(dir),
        Some("--remove") => firewall::remove(dir),
        Some("--check") => {
            let status = firewall::check(&firewall::programs_in(dir));
            return firewall::check_code(status.as_ref());
        }
        _ => return 3,
    };
    i32::from(result.is_err())
}

// ---------------------------------------------------------------- tray receiver

struct Notice {
    title: String,
    text: String,
    error: bool,
    /// What clicking this balloon does.
    action: BalloonAction,
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
            diag::log(format!("receive: offer from {} declined (busy)", from.ip()));
            return false; // another offer is waiting for an answer
        };
        let body = offer_body(self.s, offer, from);
        let accepted = platform::ask_accept(self.s.incoming_title, &body);
        diag::log(format!(
            "receive: offer of {} file(s) from {}: accepted={accepted}",
            offer.files.len(),
            from.ip()
        ));
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
                action: BalloonAction::OpenDir,
            },
            Err(e) => {
                diag::log(format!("receive: from {} failed: {e}", from.ip()));
                Notice {
                    title: fill(self.s.receive_failed, &[("name", &name)]),
                    text: display_text(&e.to_string(), SHOW_TEXT_MAX),
                    error: true,
                    action: BalloonAction::Nothing,
                }
            }
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
const ID_FIREWALL: u32 = 6;
const ID_CLASSIC: u32 = 7;
const ID_ABOUT: u32 = 8;

/// What clicking the last balloon does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BalloonAction {
    Nothing,
    OpenDir,
    Update,
    Portal,
    /// Add the firewall rules (UAC).
    Firewall,
    /// Open Windows network settings (Public → Private).
    Network,
}

/// Settings as last read by a worker. The UI thread only ever reads this copy: device.json
/// lives in the roaming AppData folder, which company PCs often redirect to a network
/// share where a read can take seconds (or minutes when the server is unreachable).
struct Prefs {
    identity: config::Identity,
    /// Windows 11 classic context menu; `None` = not applicable (Windows 10) or unknown.
    classic: Option<bool>,
    refreshing: bool,
    /// A classic-menu change (with its "restart Explorer?" question) is in progress.
    classic_busy: bool,
}

/// Serialises the workers' device.json reads/writes (and their cache updates). Never taken
/// by the UI thread.
static CONFIG_IO: Mutex<()> = Mutex::new(());

/// What tray workers need to report back to the UI thread.
#[derive(Clone)]
struct Ctx {
    s: &'static Strings,
    waker: Waker,
    prefs: Arc<Mutex<Prefs>>,
    notices: Arc<Mutex<VecDeque<Notice>>>,
}

impl Ctx {
    /// Error balloon (shown by the UI thread on the next `Wake`), also logged.
    fn notify_error(&self, title: &str, text: &str) {
        diag::log(format!("error: {title}: {text}"));
        lock(&self.notices).push_back(Notice {
            title: title.to_owned(),
            text: display_text(text, SHOW_TEXT_MAX),
            error: true,
            action: BalloonAction::Nothing,
        });
        self.waker.wake();
    }

    /// Re-reads device.json and the classic-menu switch in the background (each menu open,
    /// so `hoppy music on|off` is picked up). At most one refresh at a time.
    fn refresh_prefs(&self) {
        {
            let mut p = lock(&self.prefs);
            if p.refreshing {
                return;
            }
            p.refreshing = true;
        }
        let ctx = self.clone();
        thread::spawn(move || {
            let started = Instant::now();
            let classic = platform::classic_menu();
            let io = lock(&CONFIG_IO);
            let identity = config::load_or_create();
            let mut p = lock(&ctx.prefs);
            match identity {
                Ok(identity) => p.identity = identity,
                Err(e) => diag::log(format!("settings: reading device.json failed: {e:#}")),
            }
            if !p.classic_busy {
                p.classic = classic; // else `set_classic` reports the new state itself
            }
            p.refreshing = false;
            drop((p, io));
            let took = started.elapsed();
            if took >= diag::SLOW_HANDLER {
                diag::log(format!("settings: refresh took {} ms", took.as_millis()));
            }
        });
    }

    /// Runs a device.json change on a worker; its result becomes the cached identity.
    fn config_job(
        &self,
        what: &'static str,
        job: impl FnOnce() -> Result<config::Identity> + Send + 'static,
    ) {
        let ctx = self.clone();
        thread::spawn(move || {
            let io = lock(&CONFIG_IO);
            match job() {
                Ok(identity) => lock(&ctx.prefs).identity = identity,
                Err(e) => {
                    drop(io);
                    ctx.notify_error("Hoppy", &format!("{what}: {e:#}"));
                }
            }
        });
    }

    fn set_music(&self, on: bool) {
        lock(&self.prefs).identity.music = on;
        self.config_job("music", move || config::set_music(on));
    }

    /// Turns the classic context menu on/off, then offers to restart Explorer (worker:
    /// registry writes, a Yes/No dialog and taskkill must not block the tray).
    fn set_classic(&self, on: bool) {
        {
            let mut p = lock(&self.prefs);
            if p.classic_busy {
                return;
            }
            p.classic_busy = true;
            p.classic = Some(on);
        }
        let ctx = self.clone();
        thread::spawn(move || {
            let result = platform::set_classic_menu(on);
            lock(&ctx.prefs).classic = platform::classic_menu();
            match result {
                Ok(true) => {
                    if platform::ask_accept("Hoppy", ctx.s.restart_explorer)
                        && let Err(e) = platform::restart_explorer()
                    {
                        ctx.notify_error(ctx.s.classic_failed, &format!("{e:#}"));
                    }
                }
                Ok(false) => {}
                Err(e) => ctx.notify_error(ctx.s.classic_failed, &format!("{e:#}")),
            }
            lock(&ctx.prefs).classic_busy = false;
        });
    }

    /// Writes a summary line, then opens the log in Notepad.
    fn open_about(&self, mode: UpdateMode) {
        let (music, classic) = {
            let p = lock(&self.prefs);
            (p.identity.music, p.classic)
        };
        diag::log(format!(
            "about: Hoppy {}, updates={}, music={music}, classic_menu={classic:?}",
            update::CURRENT_VERSION,
            match mode {
                UpdateMode::Off => "off",
                UpdateMode::Setup => "setup",
                UpdateMode::Portal => "portable",
            }
        ));
        let ctx = self.clone();
        thread::spawn(move || {
            diag::flush(Duration::from_secs(2));
            let opened = match diag::log_path() {
                Some(path) => Command::new("notepad.exe")
                    .arg(path)
                    .spawn()
                    .map(|_| ())
                    .map_err(anyhow::Error::from),
                None => Err(anyhow::anyhow!("LOCALAPPDATA is not set")),
            };
            if let Err(e) = opened {
                ctx.notify_error(ctx.s.about_failed, &format!("{e:#}"));
            }
        });
    }
}

/// Opens a folder/URL on a worker (ShellExecute can block on a slow network folder).
fn open_dir(dir: &std::path::Path) {
    let dir = dir.to_path_buf();
    thread::spawn(move || {
        let _ = std::fs::create_dir_all(&dir);
        if !win::open_folder(&dir) {
            diag::log("opening the received folder failed");
        }
    });
}

fn open_url(url: &'static str) {
    thread::spawn(move || {
        if !win::open_url(url) {
            diag::log(format!("opening {url} failed"));
        }
    });
}

/// Short name of an event for the UI timing log.
fn event_name(event: &TrayEvent) -> &'static str {
    match event {
        TrayEvent::Click(_) => "click",
        TrayEvent::BalloonClick => "balloon click",
        TrayEvent::Wake => "wake",
        TrayEvent::Reopen => "reopen",
        TrayEvent::Timer(_) => "timer",
    }
}

fn tray_mode(s: &'static Strings) -> Result<()> {
    let Some(_instance) = win::single_instance(INSTANCE_MUTEX) else {
        diag::log("tray: already running, asking it to show itself");
        win::signal_existing(TRAY_WINDOW);
        return Ok(());
    };
    // Before the icon exists, so nothing is waiting on the UI thread yet.
    let identity = config::load_or_create()?;
    let me = config::device_info(&identity);
    let dir = default_receive_dir()?;
    let listener = bind_receiver(TRANSFER_PORT)?;
    let discovery = start_discovery(&me, listener.local_addr()?.port(), &[])?;

    let tray = Tray::new(TRAY_WINDOW, &fill(s.tip_ready, &[("name", &me.name)]))?;
    let notices = Arc::new(Mutex::new(VecDeque::new()));
    let ctx = Ctx {
        s,
        waker: tray.waker(),
        prefs: Arc::new(Mutex::new(Prefs {
            identity,
            classic: None,
            refreshing: false,
            classic_busy: false,
        })),
        notices: Arc::clone(&notices),
    };
    ctx.refresh_prefs(); // classic-menu state
    let handler = Arc::new(GuiReceiver {
        s,
        waker: tray.waker(),
        notices: Arc::clone(&notices),
        names: Mutex::new(HashMap::new()),
        asking: AtomicBool::new(false),
    });
    // Same folder and handler for both paths, so `GuiReceiver::asking` covers callbacks too.
    accept_callbacks(&discovery, dir.clone(), handler.clone());
    let serve_dir = dir.clone();
    thread::spawn(move || serve(listener, serve_dir, handler));

    let updates = Arc::new(Mutex::new(Updates::default()));
    let mode = update_mode();
    // Installers of earlier updates (their folder could not be removed while they ran).
    thread::spawn(|| {
        update::remove_stale_update_dirs(&std::env::temp_dir(), update::STALE_UPDATE_DIR)
    });
    if mode != UpdateMode::Off {
        spawn_update_checker(&updates, tray.waker());
    }
    let fw = Arc::new(Mutex::new(FirewallState::default()));
    spawn_firewall_check(&fw, tray.waker());

    let ready_tip = fill(s.tip_ready, &[("name", &me.name)]);
    let header = fill(s.menu_header, &[("name", &me.name)]);
    let mut balloon_action = BalloonAction::Nothing;
    let watch = diag::UiWatch::start();
    while let Some(event) = tray.next_event() {
        let mut h = watch.handler(event_name(&event));
        match event {
            TrayEvent::Click(anchor) => {
                let (music, classic) = {
                    let p = lock(&ctx.prefs);
                    (p.identity.music, p.classic)
                };
                // For the next menu: `hoppy music on|off` may have changed it meanwhile.
                ctx.refresh_prefs();
                let update_item = update_menu_text(s, &lock(&updates), mode);
                let fw_item = firewall_menu_text(s, &fw, &tray);
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
                ]);
                if let Some(on) = classic {
                    items.push(MenuItem::Check(ID_CLASSIC, s.menu_classic, on));
                }
                if let Some(text) = fw_item {
                    items.push(MenuItem::Action(ID_FIREWALL, text));
                }
                items.extend([
                    MenuItem::Separator,
                    MenuItem::Action(ID_ABOUT, s.menu_about),
                    MenuItem::Action(ID_QUIT, s.menu_quit),
                ]);
                match h.modal(|| tray.popup_menu(&items, true, anchor)) {
                    Some(ID_UPDATE) => start_update(s, &updates, mode, tray.waker()),
                    Some(ID_SEND) => match h.modal(|| tray.pick_files(s.pick_title)) {
                        Ok(files) if files.is_empty() => {}
                        Ok(files) => spawn_sender(&ctx, files),
                        Err(TooManyFiles) => {
                            h.modal(|| win::message_box_error("Hoppy", s.too_many_files));
                        }
                    },
                    Some(ID_OPEN) => open_dir(&dir),
                    Some(ID_FIREWALL) => match lock(&fw).verdict {
                        Some(Verdict::PublicNetwork) => open_url(NETWORK_SETTINGS),
                        _ => start_firewall_fix(&ctx, &fw),
                    },
                    Some(ID_MUSIC) => ctx.set_music(!music),
                    Some(ID_CLASSIC) => ctx.set_classic(!classic.unwrap_or(false)),
                    Some(ID_ABOUT) => ctx.open_about(mode),
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
                    balloon_action = n.action;
                }
                if let Some(action) = firewall_wake(&tray, &ctx, &fw) {
                    balloon_action = action;
                }
                let upd = update_wake(&tray, s, &updates, mode, &ready_tip);
                if let Some(action) = upd.balloon {
                    balloon_action = action;
                }
                if let Some(path) = upd.launch {
                    diag::log("update: starting the verified installer and exiting");
                    match Command::new(&path).args(update::INSTALLER_ARGS).spawn() {
                        // The installer stops running copies anyway; leave cleanly first
                        // (dropping `tray` removes the icon) and let it restart us.
                        Ok(_) => break,
                        Err(e) => {
                            diag::log(format!("update: starting the installer failed: {e}"));
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
                BalloonAction::Portal => open_url(update::PORTAL_URL),
                BalloonAction::Firewall => start_firewall_fix(&ctx, &fw),
                BalloonAction::Network => open_url(NETWORK_SETTINGS),
                BalloonAction::Nothing => {}
            },
            TrayEvent::Reopen => tray.balloon("Hoppy", s.already_running, false, true),
            _ => {}
        }
    }
    // Locals drop in reverse order: the tray icon goes first, then discovery says "bye".
    Ok(())
}

// ---------------------------------------------------------------- firewall

/// Windows Settings page with the Public/Private switch of the current network.
const NETWORK_SETTINGS: &str = "ms-settings:network-status";
/// Re-check (in the background) when the menu opens at most this often, so the menu item
/// goes away after the user fixed things elsewhere.
const FIREWALL_RECHECK: Duration = Duration::from_secs(60);

#[derive(Default)]
struct FirewallState {
    /// Latest result; `None` until the first check ends, or when it failed (no hint then).
    verdict: Option<Verdict>,
    /// A check ended and the UI thread has not looked at it yet.
    fresh: bool,
    checking: bool,
    checked_at: Option<Instant>,
    /// The UAC prompt / elevated `hoppyw firewall --apply` is running.
    fixing: bool,
    /// Its result (exit code 0), for the UI thread to report.
    fixed: Option<bool>,
}

/// Blocking (PowerShell, seconds): workers only. The lock is never held around it.
fn check_now() -> Option<Verdict> {
    let exe = std::env::current_exe().ok()?;
    let started = Instant::now();
    let verdict = firewall::check(&[exe]).map(|s| s.verdict(0));
    diag::log(format!(
        "firewall: check {verdict:?} in {} ms",
        started.elapsed().as_millis()
    ));
    verdict
}

fn spawn_firewall_check(fw: &Arc<Mutex<FirewallState>>, waker: Waker) {
    {
        let mut f = lock(fw);
        if f.checking || f.fixing {
            return;
        }
        f.checking = true;
    }
    let fw = Arc::clone(fw);
    thread::spawn(move || {
        let verdict = check_now();
        let mut f = lock(&fw);
        f.verdict = verdict;
        f.fresh = true;
        f.checking = false;
        f.checked_at = Some(Instant::now());
        drop(f);
        waker.wake();
    });
}

/// Menu item text while there is something to fix; starts a background re-check when the
/// last one is old.
fn firewall_menu_text(
    s: &'static Strings,
    fw: &Arc<Mutex<FirewallState>>,
    tray: &Tray,
) -> Option<&'static str> {
    let (verdict, stale) = {
        let f = lock(fw);
        let stale = f
            .checked_at
            .is_some_and(|t| t.elapsed() >= FIREWALL_RECHECK);
        (if f.fixing { None } else { f.verdict }, stale)
    };
    if stale {
        spawn_firewall_check(fw, tray.waker());
    }
    match verdict? {
        Verdict::NeedsRules => Some(s.menu_firewall),
        Verdict::PublicNetwork => Some(s.menu_network),
        Verdict::Ok => None,
    }
}

/// UAC prompt for `hoppyw firewall --apply` on a worker (the prompt blocks until answered);
/// the result is reported by `firewall_wake`. Declining the prompt is silent.
fn start_firewall_fix(ctx: &Ctx, fw: &Arc<Mutex<FirewallState>>) {
    {
        let mut f = lock(fw);
        if f.fixing {
            return;
        }
        f.fixing = true;
    }
    let (ctx, fw) = (ctx.clone(), Arc::clone(fw));
    thread::spawn(move || {
        let child = std::env::current_exe()
            .map_err(anyhow::Error::from)
            .and_then(|exe| win::run_elevated(Some(&ctx.waker), &exe, "firewall --apply"));
        let child = match child {
            Ok(child) => child,
            Err(e) => {
                lock(&fw).fixing = false;
                if e.is::<ElevationDeclined>() {
                    diag::log("firewall: UAC declined");
                } else {
                    ctx.notify_error(ctx.s.fw_failed, &format!("{e:#}"));
                }
                return;
            }
        };
        let ok = child.wait(120_000) == Some(0);
        diag::log(format!("firewall: apply ok={ok}"));
        let verdict = check_now();
        let mut f = lock(&fw);
        f.fixing = false;
        f.fixed = Some(ok);
        f.verdict = verdict;
        f.checked_at = Some(Instant::now());
        drop(f);
        ctx.waker.wake();
    });
}

/// UI-thread side of `Wake` for the firewall: the hint (once per version) or the result of
/// a fix. Returns the new balloon action when a balloon was shown.
fn firewall_wake(tray: &Tray, ctx: &Ctx, fw: &Mutex<FirewallState>) -> Option<BalloonAction> {
    let s = ctx.s;
    let (fresh, fixed, verdict) = {
        let mut f = lock(fw);
        (std::mem::take(&mut f.fresh), f.fixed.take(), f.verdict)
    };
    if let Some(ok) = fixed {
        let action = match (ok, verdict) {
            (false, _) => {
                tray.balloon(s.fw_failed, "", true, true);
                BalloonAction::Nothing
            }
            (true, Some(Verdict::NeedsRules)) => {
                tray.balloon(s.fw_failed, s.fw_policy, true, true);
                BalloonAction::Nothing
            }
            (true, Some(Verdict::PublicNetwork)) => {
                tray.balloon(s.public_title, s.public_text, false, true);
                BalloonAction::Network
            }
            (true, _) => {
                tray.balloon(s.fw_done, "", false, true);
                BalloonAction::Nothing
            }
        };
        return Some(action);
    }
    if !fresh {
        return None;
    }
    let (title, text, action) = match verdict? {
        Verdict::NeedsRules => (s.fw_title, s.fw_text, BalloonAction::Firewall),
        Verdict::PublicNetwork => (s.public_title, s.public_text, BalloonAction::Network),
        Verdict::Ok => return None,
    };
    {
        let mut p = lock(&ctx.prefs);
        if p.identity.firewall_hint == update::CURRENT_VERSION {
            return None;
        }
        p.identity.firewall_hint = update::CURRENT_VERSION.to_owned();
    }
    ctx.config_job("firewall hint", || {
        config::set_firewall_hint(update::CURRENT_VERSION)
    });
    tray.balloon(title, text, false, true);
    Some(action)
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
            match update::check(&WinHttp, update::CURRENT_VERSION) {
                Ok(Some(release)) => {
                    diag::log(format!("update: {} is available", release.version));
                    let mut u = lock(&updates);
                    if u.available.as_ref() != Some(&release) && u.downloading.is_none() {
                        u.available = Some(release);
                        u.fresh = true;
                        drop(u);
                        waker.wake();
                    }
                }
                Ok(None) => diag::log("update: up to date"),
                // Silent for the user (offline, proxy…), but kept for diagnosis.
                Err(e) => diag::log(format!("update: check failed: {e:#}")),
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
        open_url(update::PORTAL_URL);
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
        let temp = std::env::temp_dir();
        let result = update::download_fresh(&WinHttp, &release, &temp, &mut |pct| {
            let mut u = lock(&updates);
            if u.downloading != Some(pct) {
                u.downloading = Some(pct);
                drop(u);
                waker.wake(); // at most ~100 wake-ups
            }
        });
        let result = result.map_err(|e| {
            diag::log(format!("update: download failed: {e:#}"));
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

/// Runs `hoppyw send FILE…` as a separate process (one tray icon per transfer), from a
/// worker: process creation can stall while antivirus software scans the exe.
fn spawn_sender(ctx: &Ctx, files: Vec<PathBuf>) {
    let ctx = ctx.clone();
    thread::spawn(move || {
        let child = std::env::current_exe()
            .and_then(|exe| Command::new(exe).arg("send").args(&files).spawn());
        match child {
            Ok(mut child) => {
                let _ = child.wait();
            }
            Err(e) => ctx.notify_error("Hoppy", &e.to_string()),
        }
    });
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
    /// The direct connection failed; waiting for a callback (spec §6).
    fallback: bool,
    outcome: Option<Result<SendOutcome, SendError>>,
}

enum SendError {
    /// Neither direct nor callback reached the peer: show the firewall hint.
    Unreachable(String),
    Other(String),
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
    let files = if files.is_empty() {
        // Desktop shortcut double-clicked (dropping files on it passes them as arguments).
        match win::pick_files_unowned(s.pick_title) {
            Ok(files) if files.is_empty() => return Ok(()),
            Ok(files) => files,
            Err(TooManyFiles) => bail!("{}", s.too_many_files),
        }
    } else {
        // "Send with Hoppy" on several files: one process per file, merged here.
        match platform::gather_send_files(files) {
            Some(files) => files,
            None => return Ok(()), // handed to the first instance
        }
    };
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
    diag::log(format!("send: {} file(s), {}", files.len(), human(total)));
    let mut phase = Phase::Searching;
    let mut files = Some(files);
    let watch = diag::UiWatch::start();
    while let Some(event) = tray.next_event() {
        let mut h = watch.handler(event_name(&event));
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
                diag::log(format!("send: {} device(s) found", peers.len()));
                match h.modal(|| tray.popup_menu(&items, false, None)) {
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
                        spawn_send(&discovery, peer, files, &state, tray.waker());
                        phase = Phase::Sending(name);
                    }
                    _ => break, // "Cancel" or menu dismissed
                }
            }
            (TrayEvent::Wake, Phase::Sending(name)) => {
                let (accepted, pct, fallback, outcome) = {
                    let mut st = lock(&state);
                    (st.accepted, st.pct, st.fallback, st.outcome.take())
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
                    match &outcome {
                        Err(SendError::Unreachable(e)) => {
                            diag::log(format!("send: unreachable: {e}"))
                        }
                        Err(SendError::Other(e)) => diag::log(format!("send: failed: {e}")),
                        Ok(_) => diag::log(format!("send: done (callback: {fallback})")),
                    }
                    let (title, text, error) = match outcome {
                        Ok(SendOutcome::Delivered) => (s.delivered, String::new(), false),
                        Ok(SendOutcome::Declined(reason)) => {
                            // "declined" = the user said No; other reasons are protocol limits.
                            let reason = reason.filter(|r| r != "declined").unwrap_or_default();
                            (s.declined, display_text(&reason, SHOW_TEXT_MAX), true)
                        }
                        Err(SendError::Unreachable(_)) => {
                            (s.unreachable_title, s.unreachable_text.to_owned(), true)
                        }
                        Err(SendError::Other(e)) => {
                            (s.send_failed, display_text(&e, SHOW_TEXT_MAX), true)
                        }
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
                    } else if fallback && !accepted {
                        tray.set_tip(&fill(s.fallback, &[("name", name)]));
                    }
                }
            }
            (TrayEvent::Click(anchor), Phase::Sending(_)) => {
                let items = [MenuItem::Action(ID_CANCEL, s.cancel_send)];
                if h.modal(|| tray.popup_menu(&items, true, anchor)) == Some(ID_CANCEL) {
                    break; // closing the process aborts the transfer
                }
            }
            (TrayEvent::Click(anchor), Phase::Done) => {
                let items = [MenuItem::Action(ID_CANCEL, s.close)];
                if h.modal(|| tray.popup_menu(&items, true, anchor)) == Some(ID_CANCEL) {
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
    discovery: &Arc<Discovery>,
    peer: Peer,
    files: Vec<PathBuf>,
    state: &Arc<Mutex<SendState>>,
    waker: Waker,
) {
    let (discovery, state) = (Arc::clone(discovery), Arc::clone(state));
    thread::spawn(move || {
        let mut last = None;
        let me = discovery.me().clone();
        let result = send_to_peer(&peer, &me, Some(&discovery), &files, &mut |p| match p {
            // Direct connect failed (firewalled receiver?): asking it to connect back.
            Progress::Fallback => {
                diag::log("send: direct connection failed, trying a callback");
                lock(&state).fallback = true;
                waker.wake();
            }
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
        lock(&state).outcome = Some(result.map_err(|e| match e {
            Error::Unreachable(detail) => SendError::Unreachable(detail),
            e => SendError::Other(e.to_string()),
        }));
        waker.wake();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoppy_core::{DeviceInfo, DiscoveryConfig};

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
                s.fallback,
                s.unreachable_title,
                s.unreachable_text,
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
