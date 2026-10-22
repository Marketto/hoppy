//! `platform::ui` on Linux: the API of `platform::win`, on freedesktop pieces.
//!
//! - `Tray` for the receiver (`gui::TRAY_WINDOW`): a panel icon (`sni.rs`) whose menu
//!   `gui.rs` keeps current with `set_menu` (picks arrive as `TrayEvent::Menu`), plus
//!   notifications as balloons (body click = `TrayEvent::BalloonClick`).
//! - `Tray` for `hoppyw send`: no icon; the tooltip becomes a "status" notification updated
//!   in place (at most every `STATUS_EVERY`) whose buttons are the `set_menu` actions
//!   ("Cancel sending"), and `popup_menu` is a zenity/kdialog list.
//! - Events from every thread (wakers, the panel icon, notification signals, timers, a
//!   second launch over D-Bus) go through one process-wide channel to `next_event`.
//! - Single instance: the session-bus name `app.hoppy.Tray`; a second `hoppyw` calls its
//!   `Reopen` method instead of starting.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use super::notify::{self, DEFAULT_ACTION, Note, Signal, Urgency};
use super::sni::{Item, Sni};
use super::{dialog, http, sound};
use crate::platform::{COLLECT_MAX_FILES, HttpSink, MenuItem, TooManyFiles, TrayEvent};
use crate::verify::{DialogLive, DialogView, LIVE_POLL};
use crate::{diag, gui};

/// Linux has no popup menus at the cursor: the tray menu is persistent (`set_menu`) and
/// `popup_menu` is a dialog with its own Cancel button.
pub const NATIVE_POPUPS: bool = false;

/// How long an incoming offer waits for an answer (the sender gives up after 120 s).
pub const ASK_TIMEOUT: Duration = Duration::from_secs(110);
/// Send progress notification: at most one update this often.
pub const STATUS_EVERY: Duration = Duration::from_secs(2);
const BUS_NAME: &str = "app.hoppy.Tray";
const OBJECT_PATH: &str = "/app/hoppy/Tray";
const MENU_ACTION: &str = "menu:";

struct Events {
    tx: Sender<TrayEvent>,
    rx: Mutex<Receiver<TrayEvent>>,
}

fn events() -> &'static Events {
    static EVENTS: OnceLock<Events> = OnceLock::new();
    EVENTS.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        Events {
            tx,
            rx: Mutex::new(rx),
        }
    })
}

fn post(event: TrayEvent) {
    let _ = events().tx.send(event);
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------- Windows-only pieces

pub struct Elevated(Infallible);

impl Elevated {
    pub fn wait(self, _timeout_ms: u32) -> Option<u32> {
        match self.0 {}
    }
}

pub fn run_elevated(_owner: Option<&Waker>, _exe: &Path, _params: &str) -> Result<Elevated> {
    bail!("elevation is Windows-only")
}

// ---------------------------------------------------------------- sound, network, shell

/// The send window was shown (`sendui::window`): GTK raises it itself; nothing to do.
pub fn activate_app() {}

pub fn play_done_jingle() {
    sound::play_done_jingle();
}

pub fn stop_sound() {
    sound::stop_sound();
}

pub fn http_get(url: &str, user_agent: &str, sink: &mut HttpSink<'_>) -> Result<()> {
    http::http_get(url, user_agent, sink)
}

/// `xdg-open`: the file manager for folders, the browser for URLs. Blocking: workers only.
fn xdg_open(target: &std::ffi::OsStr) -> bool {
    Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn open_folder(dir: &Path) -> bool {
    xdg_open(dir.as_os_str())
}

pub fn open_url(url: &str) -> bool {
    xdg_open(url.as_ref())
}

// ---------------------------------------------------------------- single instance

/// Owns the bus name while the tray runs.
pub struct InstanceGuard {
    _conn: Option<zbus::blocking::Connection>,
}

struct Remote;

#[zbus::interface(name = "app.hoppy.Tray")]
impl Remote {
    /// A second `hoppyw` started: show "already running".
    fn reopen(&self) {
        post(TrayEvent::Reopen);
    }
}

pub fn single_instance(_name: &str) -> Option<InstanceGuard> {
    let conn = match zbus::blocking::Connection::session() {
        Ok(conn) => conn,
        Err(e) => {
            // `Tray::new` reports the missing session bus.
            diag::log(format!("instance: no session bus: {e}"));
            return Some(InstanceGuard { _conn: None });
        }
    };
    if let Err(e) = conn.object_server().at(OBJECT_PATH, Remote) {
        diag::log(format!("instance: cannot serve {OBJECT_PATH}: {e}"));
    }
    let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
    match conn.request_name_with_flags(BUS_NAME, flags) {
        Ok(
            zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner,
        ) => Some(InstanceGuard { _conn: Some(conn) }),
        Ok(_) | Err(zbus::Error::NameTaken) => None,
        Err(e) => {
            diag::log(format!("instance: cannot own {BUS_NAME}: {e}"));
            Some(InstanceGuard { _conn: Some(conn) })
        }
    }
}

pub fn signal_existing(_window_title: &str) -> bool {
    let Ok(conn) = zbus::blocking::Connection::session() else {
        return false;
    };
    conn.call_method(Some(BUS_NAME), OBJECT_PATH, Some(BUS_NAME), "Reopen", &())
        .is_ok()
}

// ---------------------------------------------------------------- dialogs

/// Error box (dialog, else notification, else stderr). Blocks until closed.
pub fn message_box_error(title: &str, body: &str) {
    if dialog::error(title, body) {
        return;
    }
    let shown = notify::get().and_then(|n| {
        n.show(&Note {
            summary: title,
            body,
            ..Default::default()
        })
    });
    if shown.is_none() {
        eprintln!("{title}: {body}");
    }
}

/// The accept question for an incoming offer: a notification with Accept / Decline (and
/// "Verify…" when there is a code to compare, which opens [`show_dialog`]) where the server
/// supports actions (GNOME, KDE…), else the dialog straight away. Clicking the notification
/// body opens the dialog too. `view.buttons` = [Decline, Accept, …]. No answer within the
/// timeout, a dismissed notification, or no way to ask at all: the default (Decline).
/// Blocking: called on transfer threads.
pub fn ask_offer(view: &DialogView, live: &DialogLive) -> Option<u32> {
    let deadline = Instant::now() + view.timeout.unwrap_or(ASK_TIMEOUT);
    if let Some(n) = notify::get().filter(|n| n.has("actions")) {
        match ask_by_notification(n, view, deadline, live) {
            Some(Asked::Answer(id)) => return id,
            Some(Asked::Details) => {}
            None => diag::log("receive: the accept notification failed, using a dialog"),
        }
    }
    if dialog::question_tool().is_none() {
        diag::log("receive: no way to ask (no notification buttons, no zenity/kdialog)");
        return None;
    }
    let mut view = view.clone();
    view.timeout = Some(deadline.saturating_duration_since(Instant::now()));
    show_dialog(&view, live)
}

enum Asked {
    Answer(Option<u32>),
    /// Body or "Verify…" clicked: ask again with the dialog (whole text, large emoji).
    Details,
}

fn ask_by_notification(
    n: &notify::Notifier,
    view: &DialogView,
    deadline: Instant,
    live: &DialogLive,
) -> Option<Asked> {
    let (Some(decline), Some(accept)) = (view.buttons.first(), view.buttons.get(1)) else {
        return None;
    };
    let can_detail = dialog::tool().is_some();
    // Without a dialog program a body click could only decline: no "default" action then.
    let mut actions: Vec<(&str, &str)> = Vec::new();
    if can_detail {
        actions.push((DEFAULT_ACTION, ""));
    }
    actions.extend([
        ("accept", accept.1.as_str()),
        ("decline", decline.1.as_str()),
    ]);
    if let (true, Some(details)) = (can_detail, &view.details) {
        actions.push(("details", details.as_str()));
    }
    let body = view.plain_text();
    let id = n.show(&Note {
        summary: &view.title,
        body: &body,
        actions: &actions,
        urgency: Urgency::Critical,
        timeout_ms: 0,
        ..Default::default()
    })?;
    let rx = n.listen(id);
    let asked = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || live.closed() {
            break Asked::Answer(None);
        }
        match rx.recv_timeout(left.min(LIVE_POLL)) {
            Ok(Signal::Action(key)) if key == "accept" => break Asked::Answer(Some(accept.0)),
            Ok(Signal::Action(key)) if key == "decline" => break Asked::Answer(Some(decline.0)),
            Ok(Signal::Action(key)) if key == DEFAULT_ACTION || key == "details" => {
                break Asked::Details;
            }
            Ok(Signal::Action(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(Signal::Closed(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Asked::Answer(None);
            }
        }
    };
    n.unlisten(id);
    n.close(id);
    Some(asked)
}

/// The verification dialog (zenity, else kdialog): see `dialog::dialog_args`. Closed when
/// `live` says so or after `view.timeout`. `None` = closed without a choice (the caller uses
/// the default). Blocking: workers only.
pub fn show_dialog(view: &DialogView, live: &DialogLive) -> Option<u32> {
    let Some(tool) = dialog::question_tool() else {
        diag::log("dialog: neither zenity nor kdialog is installed");
        return None;
    };
    if live.closed() {
        return None;
    }
    let args = dialog::dialog_args(tool, view, view.timeout);
    // zenity gets a little longer than its own --timeout; kdialog has none.
    let limit = view.timeout.map(|l| l + Duration::from_secs(2));
    match dialog::run_watch(tool, &args, limit, &|| live.closed()) {
        dialog::Ran::Exited(code, out) => match dialog::parse_answer(tool, view, code, &out) {
            dialog::Answer::Button(id) => Some(id),
            dialog::Answer::Closed => None,
        },
        dialog::Ran::Stopped | dialog::Ran::Failed => None,
    }
}

/// Tells the user a dialog program is needed (device list, file picker).
fn no_dialog_tool() {
    let text = gui::strings().no_dialog_tool;
    diag::log("dialog: neither zenity nor kdialog is installed");
    let shown = notify::get().and_then(|n| {
        n.show(&Note {
            summary: "Hoppy",
            body: text,
            ..Default::default()
        })
    });
    if shown.is_none() {
        eprintln!("Hoppy: {text}");
    }
}

pub fn pick_files_unowned(title: &str) -> Result<Vec<PathBuf>, TooManyFiles> {
    let Some(files) = dialog::pick_files(title) else {
        no_dialog_tool();
        return Ok(Vec::new());
    };
    if files.len() > COLLECT_MAX_FILES {
        return Err(TooManyFiles);
    }
    Ok(files)
}

// ---------------------------------------------------------------- tray

/// Lets worker threads nudge the UI thread (`TrayEvent::Wake`).
#[derive(Clone)]
pub struct Waker(());

impl Waker {
    pub fn wake(&self) {
        post(TrayEvent::Wake);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The receiver: panel icon + persistent menu.
    Receiver,
    /// `hoppyw send`: status notification + dialogs.
    Send,
}

/// Notification ids the signal dispatcher compares against.
#[derive(Default)]
struct Shown {
    balloon: Option<u32>,
    status: Option<u32>,
}

/// Maps a notification signal to a tray event (runs on the dispatcher thread).
fn on_signal(shown: &Mutex<Shown>, id: u32, signal: &Signal) {
    let mut s = lock(shown);
    match signal {
        Signal::Action(key) if s.balloon == Some(id) && key == DEFAULT_ACTION => {
            post(TrayEvent::BalloonClick);
        }
        Signal::Action(key) if s.status == Some(id) => {
            if let Some(n) = key.strip_prefix(MENU_ACTION).and_then(|n| n.parse().ok()) {
                post(TrayEvent::Menu(n));
            }
        }
        Signal::Closed(_) if s.status == Some(id) => s.status = None,
        _ => {}
    }
}

/// Notification "tray" (see the module docs). UI-thread only.
pub struct Tray {
    kind: Kind,
    sni: Option<Sni>,
    shown: Arc<Mutex<Shown>>,
    tip: RefCell<String>,
    /// Send: buttons of the status notification, from `set_menu`.
    actions: RefCell<Vec<(String, String)>>,
    status_at: Cell<Option<Instant>>,
    timers: Arc<Mutex<HashMap<usize, u64>>>,
}

impl Tray {
    /// `window_title` = `gui::TRAY_WINDOW` for the receiver; anything else is a sender.
    pub fn new(window_title: &str, tip: &str) -> Result<Tray> {
        let kind = if window_title == gui::TRAY_WINDOW {
            Kind::Receiver
        } else {
            Kind::Send
        };
        let shown = Arc::new(Mutex::new(Shown::default()));
        if let Some(n) = notify::get() {
            let shown = Arc::clone(&shown);
            n.set_fallback(move |id, signal| on_signal(&shown, id, signal));
        }
        let sni = match kind {
            Kind::Receiver => Some(Sni::spawn(tip, events().tx.clone())?),
            Kind::Send => None,
        };
        let tray = Tray {
            kind,
            sni,
            shown,
            tip: RefCell::new(tip.to_owned()),
            actions: RefCell::new(Vec::new()),
            status_at: Cell::new(None),
            timers: Arc::new(Mutex::new(HashMap::new())),
        };
        if kind == Kind::Send {
            tray.show_status(true);
        }
        Ok(tray)
    }

    /// Shows the tip as the status notification (send only); throttled unless `now`.
    fn show_status(&self, now: bool) {
        let Some(n) = notify::get() else { return };
        if !now
            && self
                .status_at
                .get()
                .is_some_and(|t| t.elapsed() < STATUS_EVERY)
        {
            return;
        }
        let actions = self.actions.borrow();
        let pairs: Vec<(&str, &str)> = actions
            .iter()
            .map(|(k, l)| (k.as_str(), l.as_str()))
            .collect();
        let replaces = lock(&self.shown).status.unwrap_or(0);
        let tip = self.tip.borrow();
        let id = n.show(&Note {
            summary: &tip,
            actions: &pairs,
            transient: true,
            sound: Some(false),
            replaces,
            timeout_ms: 0,
            ..Default::default()
        });
        lock(&self.shown).status = id;
        self.status_at.set(Some(Instant::now()));
    }

    fn close_status(&self) {
        let id = lock(&self.shown).status.take();
        if let (Some(id), Some(n)) = (id, notify::get()) {
            n.close(id);
        }
        self.status_at.set(None);
    }

    pub fn set_tip(&self, tip: &str) {
        // Same text: nothing to do, unless a sender's status was closed (a dialog was shown)
        // and must come back ("Search again").
        let hidden = self.kind == Kind::Send && lock(&self.shown).status.is_none();
        if *self.tip.borrow() == tip && !hidden {
            return;
        }
        *self.tip.borrow_mut() = tip.to_owned();
        match (&self.sni, self.kind) {
            (Some(sni), _) => sni.set_tip(tip),
            (None, Kind::Send) => self.show_status(false),
            (None, Kind::Receiver) => {}
        }
    }

    /// The tray menu (receiver), or the status notification's buttons (send).
    pub fn set_menu(&self, items: &[MenuItem<'_>]) {
        if let Some(sni) = &self.sni {
            sni.set_menu(items.iter().map(Item::from_menu).collect());
            return;
        }
        let actions: Vec<(String, String)> = items
            .iter()
            .filter_map(|i| match *i {
                MenuItem::Action(id, t) | MenuItem::Bold(id, t) => {
                    Some((format!("{MENU_ACTION}{id}"), t.to_owned()))
                }
                _ => None,
            })
            .collect();
        if *self.actions.borrow() != actions {
            *self.actions.borrow_mut() = actions;
            if lock(&self.shown).status.is_some() {
                self.show_status(true);
            }
        }
    }

    /// A notification; clicking it yields `TrayEvent::BalloonClick`. `sound`: the server's
    /// sound (off when our jingle plays).
    pub fn balloon(&self, title: &str, text: &str, _error: bool, sound: bool) {
        if self.kind == Kind::Send {
            self.close_status();
        }
        let Some(n) = notify::get() else {
            eprintln!("{title} {text}");
            return;
        };
        let id = n.show(&Note {
            summary: title,
            body: text,
            actions: &[(DEFAULT_ACTION, "")],
            sound: (!sound).then_some(false),
            ..Default::default()
        });
        lock(&self.shown).balloon = id;
    }

    pub fn waker(&self) -> Waker {
        Waker(())
    }

    /// Repeats every `ms` until `kill_timer` (like WM_TIMER).
    pub fn set_timer(&self, id: usize, ms: u32) {
        let generation = {
            let mut t = lock(&self.timers);
            let g = t.entry(id).or_insert(0);
            *g += 1;
            *g
        };
        let timers = Arc::clone(&self.timers);
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(u64::from(ms)));
                if lock(&timers).get(&id) != Some(&generation) {
                    return;
                }
                post(TrayEvent::Timer(id));
            }
        });
    }

    pub fn kill_timer(&self, id: usize) {
        if let Some(g) = lock(&self.timers).get_mut(&id) {
            *g += 1;
        }
    }

    /// A list dialog: labels become its text, actions its rows (the dialog has its own
    /// Cancel). `None` = cancelled.
    pub fn popup_menu(
        &self,
        items: &[MenuItem<'_>],
        _above: bool,
        _anchor: Option<(i32, i32)>,
    ) -> Option<u32> {
        if self.kind == Kind::Send {
            self.close_status();
        }
        let text: Vec<&str> = items
            .iter()
            .filter_map(|i| match *i {
                MenuItem::Label(t) => Some(t),
                _ => None,
            })
            .collect();
        let rows: Vec<(u32, &str)> = items
            .iter()
            .filter_map(|i| match *i {
                MenuItem::Action(id, t) | MenuItem::Bold(id, t) | MenuItem::Check(id, t, _) => {
                    Some((id, t))
                }
                _ => None,
            })
            .collect();
        diag::log(format!("menu: dialog with {} entries", rows.len()));
        match dialog::choose_from("Hoppy", &text.join("\n"), &rows) {
            Some(choice) => choice,
            None => {
                no_dialog_tool();
                None
            }
        }
    }

    pub fn pick_files(&self, title: &str) -> Result<Vec<PathBuf>, TooManyFiles> {
        pick_files_unowned(title)
    }

    pub fn next_event(&self) -> Option<TrayEvent> {
        lock(&events().rx).recv().ok()
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.close_status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain() -> Vec<TrayEvent> {
        let rx = lock(&events().rx);
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[test]
    fn notification_signals_become_tray_events() {
        let shown = Mutex::new(Shown {
            balloon: Some(5),
            status: Some(9),
        });
        drain();
        on_signal(&shown, 5, &Signal::Action(DEFAULT_ACTION.into()));
        on_signal(&shown, 4, &Signal::Action(DEFAULT_ACTION.into())); // an older balloon
        on_signal(&shown, 9, &Signal::Action("menu:2".into()));
        on_signal(&shown, 9, &Signal::Action("menu:x".into()));
        assert_eq!(drain(), [TrayEvent::BalloonClick, TrayEvent::Menu(2)]);
        on_signal(&shown, 9, &Signal::Closed(2));
        assert_eq!(lock(&shown).status, None);
    }
}
