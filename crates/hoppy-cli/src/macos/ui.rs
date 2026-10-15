//! `platform::ui` on macOS: the API of `platform::win`, on AppKit (objc2).
//!
//! Threading: AppKit lives on the main thread, where `gui.rs` runs its event loop.
//! `Tray::next_event` pumps AppKit events (`nextEventMatchingMask…` + `sendEvent`) until
//! our own channel has a `TrayEvent`; events from other threads (wakers, timers,
//! notification clicks) are sent to the channel, then the main thread is woken by an
//! application-defined `NSEvent` posted from the main dispatch queue. Worker threads that
//! need a dialog (the accept question) run it on the main thread with
//! `dispatch2::run_on_main`; the main queue is served by the run loop in every mode, also
//! while a menu or another dialog is open.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::convert::Infallible;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use block2::RcBlock;
use dispatch2::{DispatchQueue, run_on_main};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSAlertSecondButtonReturn, NSAlertStyle, NSApplication, NSApplicationActivationPolicy,
    NSApplicationDelegate, NSControlStateValueOn, NSEvent, NSEventMask, NSEventModifierFlags,
    NSEventType, NSFont, NSFontAttributeName, NSImage, NSMenu, NSMenuDelegate, NSMenuItem,
    NSModalResponseOK, NSOpenPanel, NSPasteboard, NSPasteboardTypeFileURL, NSSound, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength, NSWindowCollectionBehavior,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSData, NSDate, NSDefaultRunLoopMode, NSDictionary, NSError,
    NSLocale, NSPoint, NSSize, NSString, NSURL,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

use crate::platform::{COLLECT_MAX_FILES, HttpSink, MenuItem, TooManyFiles, TrayEvent};
use crate::{diag, gui};

/// Clicking the menu-bar icon pops a menu built at that moment, like on Windows.
pub const NATIVE_POPUPS: bool = true;
/// How long an incoming offer waits for an answer (the sender gives up after 120 s).
pub const ASK_TIMEOUT: Duration = Duration::from_secs(110);
/// The event loop also wakes up this often, in case a wake-up event was swallowed by a
/// menu or dialog that ran its own loop.
const PUMP_TIMEOUT: f64 = 5.0;
/// `data1` of our application-defined wake-up event.
const WAKE_MAGIC: isize = 0x486f_7070; // "Hopp"

static TEMPLATE_ICON: &[u8] = include_bytes!("../../../../assets/brand/macos/menubar-template.png");
static DONE_JINGLE: &[u8] = include_bytes!("../../../../assets/music/desktop_done.wav");

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------- events

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

/// A wake-up event is on its way to the main thread.
static WAKE_PENDING: AtomicBool = AtomicBool::new(false);

/// Queues an event for `Tray::next_event` and wakes the main thread. Any thread.
fn post(event: TrayEvent) {
    let _ = events().tx.send(event);
    wake_main();
}

fn wake_main() {
    if WAKE_PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    DispatchQueue::main().exec_async(|| {
        // SAFETY: blocks on the main queue run on the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        post_wake_event(mtm);
    });
}

fn post_wake_event(mtm: MainThreadMarker) {
    let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        0,
        WAKE_MAGIC,
        0,
    );
    match event {
        Some(event) => NSApplication::sharedApplication(mtm).postEvent_atStart(&event, false),
        None => WAKE_PENDING.store(false, Ordering::Release),
    }
}

/// After a menu or dialog ran its own event loop: our wake-up event may have been taken
/// by it, so allow a new one and send it.
fn after_modal() {
    WAKE_PENDING.store(false, Ordering::Release);
    wake_main();
}

/// Waits for one AppKit event (or `PUMP_TIMEOUT`) and dispatches it.
fn pump_once(app: &NSApplication) {
    autoreleasepool(|_| {
        let until = NSDate::dateWithTimeIntervalSinceNow(PUMP_TIMEOUT);
        // SAFETY: a valid run loop mode constant.
        let mode = unsafe { NSDefaultRunLoopMode };
        let event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&until),
            mode,
            true,
        );
        match event {
            Some(e) if e.r#type() == NSEventType::ApplicationDefined && e.data1() == WAKE_MAGIC => {
                WAKE_PENDING.store(false, Ordering::Release);
            }
            Some(e) => app.sendEvent(&e),
            None => WAKE_PENDING.store(false, Ordering::Release),
        }
    });
}

// ---------------------------------------------------------------- the app object

/// Main-thread state, created on first use.
struct App {
    app: Retained<NSApplication>,
    delegate: Retained<Delegate>,
}

thread_local! {
    static APP: OnceCell<App> = const { OnceCell::new() };
    /// The tag of the menu item picked in the menu that is open now.
    static PICKED: Cell<Option<isize>> = const { Cell::new(None) };
    /// A status-item menu is open (a click on the icon meanwhile is not a new click).
    static MENU_OPEN: Cell<bool> = const { Cell::new(false) };
    /// `menuWillOpen:` was called for the menu being shown.
    static MENU_SHOWN: Cell<bool> = const { Cell::new(false) };
    /// `hoppyw --smoke-test`: no permission prompts.
    static SMOKE: Cell<bool> = const { Cell::new(false) };
    static SOUND: RefCell<Option<Retained<NSSound>>> = const { RefCell::new(None) };
}

/// The shared `NSApplication`: accessory (menu-bar only, no Dock icon even outside the
/// bundle), with our delegate, launched. Main thread only.
fn app(mtm: MainThreadMarker) -> Retained<NSApplication> {
    APP.with(|cell| {
        cell.get_or_init(|| {
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            let delegate = Delegate::new(mtm);
            app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            app.finishLaunching();
            App { app, delegate }
        })
        .app
        .clone()
    })
}

fn delegate(mtm: MainThreadMarker) -> Retained<Delegate> {
    app(mtm);
    APP.with(|cell| cell.get().map(|a| a.delegate.clone()))
        .expect("app() initialises APP")
}

/// Brings Hoppy's menus and dialogs in front of other apps.
#[allow(deprecated)] // `activate` is macOS 14+; Hoppy supports 12
fn activate(app: &NSApplication) {
    app.activateIgnoringOtherApps(true);
}

define_class!(
    /// Target of the status-item button and of menu items; app delegate (files dropped on
    /// the icon, "Open With", reopen) and Finder services provider.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "HoppyAppDelegate"]
    struct Delegate;

    impl Delegate {
        #[unsafe(method(statusClicked:))]
        fn status_clicked(&self, _sender: Option<&AnyObject>) {
            if !MENU_OPEN.with(Cell::get) {
                post(TrayEvent::Click(None));
            }
        }

        #[unsafe(method(menuPicked:))]
        fn menu_picked(&self, sender: Option<&NSMenuItem>) {
            if let Some(item) = sender {
                PICKED.with(|p| p.set(Some(item.tag())));
            }
        }

        /// Finder → Services → "Send with Hoppy".
        #[unsafe(method(sendFiles:userData:error:))]
        fn send_files(
            &self,
            pboard: &NSPasteboard,
            _user_data: Option<&NSString>,
            _error: *mut *mut NSString,
        ) {
            let files = pasteboard_files(pboard);
            diag::log(format!("service: {} file(s)", files.len()));
            spawn_sender(files);
        }
    }

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSMenuDelegate for Delegate {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            MENU_SHOWN.with(|m| m.set(true));
        }
    }

    unsafe impl NSApplicationDelegate for Delegate {
        /// Files dropped on the app icon, "Open With → Hoppy", `open -a Hoppy FILE`.
        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            let files: Vec<PathBuf> = urls.iter().filter_map(|u| url_path(&u)).collect();
            diag::log(format!("open: {} file(s)", files.len()));
            spawn_sender(files);
        }

        /// Hoppy opened again while running (Finder, Launchpad, `open -a`).
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            post(TrayEvent::Reopen);
            false
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// Path of a `file:` URL (file reference URLs resolved).
fn url_path(url: &NSURL) -> Option<PathBuf> {
    let url = url.filePathURL()?;
    Some(PathBuf::from(url.path()?.to_string()))
}

/// The files on a services/drag pasteboard: `public.file-url` items, else the legacy
/// `NSFilenamesPboardType` list of paths.
pub fn pasteboard_files(pboard: &NSPasteboard) -> Vec<PathBuf> {
    // SAFETY: a valid pasteboard type constant.
    let file_url = unsafe { NSPasteboardTypeFileURL };
    let files: Vec<PathBuf> = pboard
        .pasteboardItems()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.stringForType(file_url))
                .filter_map(|s| NSURL::URLWithString(&s))
                .filter_map(|u| url_path(&u))
                .collect()
        })
        .unwrap_or_default();
    if !files.is_empty() {
        return files;
    }
    let legacy = NSString::from_str("NSFilenamesPboardType");
    let Some(list) = pboard.propertyListForType(&legacy) else {
        return files;
    };
    let Some(list) = list.downcast_ref::<NSArray>() else {
        return files;
    };
    list.iter()
        .filter_map(|p| {
            p.downcast_ref::<NSString>()
                .map(|s| PathBuf::from(s.to_string()))
        })
        .collect()
}

/// `hoppyw send FILE…` as a separate process (its own status item, like on Windows).
fn spawn_sender(files: Vec<PathBuf>) {
    if files.is_empty() {
        return;
    }
    thread::spawn(move || {
        let child = std::env::current_exe()
            .and_then(|exe| Command::new(exe).arg("send").args(&files).spawn());
        match child {
            Ok(mut child) => {
                let _ = child.wait();
            }
            Err(e) => diag::log(format!("send: cannot start hoppyw send: {e}")),
        }
    });
}

// ---------------------------------------------------------------- language

/// Italian UI when the first preferred language is Italian (System Settings → General →
/// Language & Region; GUI apps get no `LANG`).
pub fn ui_is_italian() -> bool {
    let languages = NSLocale::preferredLanguages();
    let first = languages.firstObject().map(|l| l.to_string());
    super::language_is_italian(first.as_deref())
}

// ---------------------------------------------------------------- notifications

const NOTES_PENDING: u8 = 0;
const NOTES_READY: u8 = 1;
const NOTES_DENIED: u8 = 2;
/// Not in an app bundle, or the system refused (e.g. the signature): use osascript.
const NOTES_UNAVAILABLE: u8 = 3;

static NOTES: AtomicU8 = AtomicU8::new(NOTES_PENDING);
static NOTE_SEQ: AtomicU64 = AtomicU64::new(0);
/// Identifier of this process's latest balloon (only a click on it counts).
static LAST_NOTE: Mutex<String> = Mutex::new(String::new());

/// Running from Hoppy.app (UNUserNotificationCenter throws outside a bundle).
fn in_bundle() -> bool {
    let bundle = objc2_foundation::NSBundle::mainBundle();
    let id = bundle.bundleIdentifier().map(|s| s.to_string());
    id.as_deref() == Some(super::BUNDLE_ID) && bundle.bundlePath().to_string().ends_with(".app")
}

define_class!(
    /// `UNUserNotificationCenter` delegate: shows banners while Hoppy is active and turns a
    /// click on the latest balloon into `TrayEvent::BalloonClick`. Any thread.
    #[unsafe(super(NSObject))]
    #[name = "HoppyNotificationDelegate"]
    struct NoteDelegate;

    unsafe impl NSObjectProtocol for NoteDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NoteDelegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            // SAFETY: a valid string constant.
            let default = unsafe { UNNotificationDefaultActionIdentifier };
            let id = response.notification().request().identifier().to_string();
            let action = response.actionIdentifier().to_string();
            if action == default.to_string() && *lock(&LAST_NOTE) == id {
                post(TrayEvent::BalloonClick);
            }
            handler.call(());
        }
    }
);

impl NoteDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// Asks for permission once per process (the system asks the user only the first time).
fn start_notifications() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if SMOKE.with(Cell::get) {
        return;
    }
    STARTED.get_or_init(|| {
        if !in_bundle() {
            diag::log("notifications: not running from Hoppy.app, using osascript");
            NOTES.store(NOTES_UNAVAILABLE, Ordering::Release);
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = NoteDelegate::new();
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        std::mem::forget(delegate); // the center keeps a weak reference: live forever
        let done = RcBlock::new(|granted: Bool, error: *mut NSError| {
            let state = if !error.is_null() {
                // SAFETY: a non-null NSError from the system.
                let code = unsafe { (*error).code() };
                diag::log(format!(
                    "notifications: unavailable (error {code}), using osascript"
                ));
                NOTES_UNAVAILABLE
            } else if granted.as_bool() {
                NOTES_READY
            } else {
                diag::log("notifications: turned off by the user");
                NOTES_DENIED
            };
            NOTES.store(state, Ordering::Release);
        });
        let options = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound;
        center.requestAuthorizationWithOptions_completionHandler(options, &done);
    });
}

fn notify(title: &str, text: &str, sound: bool) {
    match NOTES.load(Ordering::Acquire) {
        NOTES_DENIED => diag::log("notifications: off, not shown"),
        NOTES_UNAVAILABLE => notify_with_osascript(title, text),
        _ => {
            let id = format!(
                "hoppy-{}-{}",
                std::process::id(),
                NOTE_SEQ.fetch_add(1, Ordering::Relaxed)
            );
            let content = UNMutableNotificationContent::new();
            content.setTitle(&NSString::from_str(title));
            content.setBody(&NSString::from_str(text));
            if sound {
                content.setSound(Some(&UNNotificationSound::defaultSound()));
            }
            let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
                &NSString::from_str(&id),
                &content,
                None,
            );
            *lock(&LAST_NOTE) = id;
            let done = RcBlock::new(|error: *mut NSError| {
                if !error.is_null() {
                    // SAFETY: a non-null NSError from the system.
                    let code = unsafe { (*error).code() };
                    diag::log(format!("notifications: not shown (error {code})"));
                }
            });
            UNUserNotificationCenter::currentNotificationCenter()
                .addNotificationRequest_withCompletionHandler(&request, Some(&done));
        }
    }
}

/// Fallback: `display notification` (shown as Script Editor's; clicks do nothing useful).
fn notify_with_osascript(title: &str, text: &str) {
    let args = super::notification_args(title, text);
    thread::spawn(move || {
        let status = Command::new("/usr/bin/osascript")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !status.is_ok_and(|s| s.success()) {
            diag::log("notifications: osascript failed");
        }
    });
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

/// The delivery jingle (embedded WAV) through `NSSound`. Main thread (the send loop).
pub fn play_done_jingle() {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let data = NSData::with_bytes(DONE_JINGLE);
    let Some(sound) = NSSound::initWithData(NSSound::alloc(), &data) else {
        diag::log("sound: the jingle could not be decoded");
        return;
    };
    stop_sound();
    if !sound.play() {
        diag::log("sound: NSSound did not play");
    }
    SOUND.with(|s| *s.borrow_mut() = Some(sound));
}

pub fn stop_sound() {
    if let Some(sound) = SOUND.with(|s| s.borrow_mut().take()) {
        sound.stop();
    }
}

pub fn http_get(url: &str, user_agent: &str, sink: &mut HttpSink<'_>) -> Result<()> {
    crate::unix::http::http_get(url, user_agent, sink)
}

/// `/usr/bin/open`: Finder for folders, the browser for URLs. Blocking: workers only.
fn open(target: &std::ffi::OsStr) -> bool {
    Command::new("/usr/bin/open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn open_folder(dir: &Path) -> bool {
    open(dir.as_os_str())
}

pub fn open_url(url: &str) -> bool {
    open(url.as_ref())
}

// ---------------------------------------------------------------- single instance

/// Holds an exclusive lock on `tray.lock` in the settings folder while the tray runs.
pub struct InstanceGuard {
    _file: Option<File>,
}

pub fn single_instance(_name: &str) -> Option<InstanceGuard> {
    let path = match crate::config::config_dir() {
        Ok(dir) => dir.join("tray.lock"),
        Err(e) => {
            diag::log(format!("instance: no settings folder: {e:#}"));
            return Some(InstanceGuard { _file: None });
        }
    };
    let _ = path.parent().map(std::fs::create_dir_all);
    let file = match File::options().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(e) => {
            diag::log(format!("instance: cannot open the lock file: {e}"));
            return Some(InstanceGuard { _file: None });
        }
    };
    match file.try_lock() {
        Ok(()) => Some(InstanceGuard { _file: Some(file) }),
        Err(std::fs::TryLockError::WouldBlock) => None,
        Err(std::fs::TryLockError::Error(e)) => {
            diag::log(format!("instance: cannot lock: {e}"));
            Some(InstanceGuard { _file: Some(file) })
        }
    }
}

/// Asks the running Hoppy.app to show itself: Launch Services sends it a "reopen".
pub fn signal_existing(_window_title: &str) -> bool {
    if !in_bundle() {
        return false;
    }
    Command::new("/usr/bin/open")
        .args(["-b", super::BUNDLE_ID])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

// ---------------------------------------------------------------- dialogs

/// Error box. Blocks until closed; any thread (runs on the main thread).
pub fn message_box_error(title: &str, body: &str) {
    let (title, body) = (title.to_owned(), body.to_owned());
    run_on_main(move |mtm| {
        let app = app(mtm);
        let alert = NSAlert::new(mtm);
        alert.setAlertStyle(NSAlertStyle::Critical);
        alert.setMessageText(&NSString::from_str(&title));
        alert.setInformativeText(&NSString::from_str(&body));
        alert.addButtonWithTitle(&NSString::from_str("OK"));
        activate(&app);
        alert.runModal();
        after_modal();
    });
}

/// The open accept question (0 = none), so the timeout closes only that one.
static OPEN_ASK: AtomicU64 = AtomicU64::new(0);
static ASK_SEQ: AtomicU64 = AtomicU64::new(0);

/// The accept question for an incoming offer: "Decline" is the default (Return) and the
/// answer after `ASK_TIMEOUT` without a click. Blocking: called on transfer threads.
pub fn message_box_yes_no(title: &str, body: &str) -> bool {
    let (title, body) = (title.to_owned(), body.to_owned());
    let s = gui::strings();
    run_on_main(move |mtm| {
        let app = app(mtm);
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&title));
        alert.setInformativeText(&NSString::from_str(&body));
        // First button = default (Return); never "Accept".
        alert.addButtonWithTitle(&NSString::from_str(s.decline));
        alert.addButtonWithTitle(&NSString::from_str(s.accept));
        // Seen even when macOS does not activate Hoppy (no click started this): on every
        // Space and over full-screen apps.
        alert.window().setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        let seq = ASK_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        OPEN_ASK.store(seq, Ordering::Release);
        thread::spawn(move || {
            thread::sleep(ASK_TIMEOUT);
            DispatchQueue::main().exec_async(move || {
                if OPEN_ASK.load(Ordering::Acquire) == seq {
                    diag::log("receive: no answer in time, declined");
                    // SAFETY: blocks on the main queue run on the main thread.
                    let mtm = unsafe { MainThreadMarker::new_unchecked() };
                    NSApplication::sharedApplication(mtm).abortModal();
                }
            });
        });
        activate(&app);
        let answer = alert.runModal();
        OPEN_ASK.store(0, Ordering::Release);
        after_modal();
        answer == NSAlertSecondButtonReturn
    })
}

pub fn pick_files_unowned(title: &str) -> Result<Vec<PathBuf>, TooManyFiles> {
    let Some(mtm) = MainThreadMarker::new() else {
        diag::log("pick: not on the main thread");
        return Ok(Vec::new());
    };
    let app = app(mtm);
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(true);
    panel.setResolvesAliases(true);
    panel.setMessage(Some(&NSString::from_str(title)));
    activate(&app);
    let answer = panel.runModal();
    after_modal();
    if answer != NSModalResponseOK {
        return Ok(Vec::new());
    }
    let files: Vec<PathBuf> = panel.URLs().iter().filter_map(|u| url_path(&u)).collect();
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
    /// The receiver: bunny icon, tip as tooltip.
    Receiver,
    /// `hoppyw send`: bunny icon + the tip as title (progress visible in the menu bar).
    Send,
}

/// The menu-bar icon. Main thread only.
pub struct Tray {
    kind: Kind,
    item: Retained<NSStatusItem>,
    mtm: MainThreadMarker,
    timers: Arc<Mutex<HashMap<usize, u64>>>,
}

/// The bunny as a template image (black + alpha; macOS tints it for light/dark menu bars).
fn template_icon() -> Option<Retained<NSImage>> {
    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(TEMPLATE_ICON))?;
    image.setSize(NSSize::new(18.0, 18.0));
    image.setTemplate(true);
    image.setAccessibilityDescription(Some(&NSString::from_str("Hoppy")));
    Some(image)
}

impl Tray {
    /// `window_title` = `gui::TRAY_WINDOW` for the receiver; anything else is a sender.
    pub fn new(window_title: &str, tip: &str) -> Result<Tray> {
        let mtm = MainThreadMarker::new().context("the menu-bar icon needs the main thread")?;
        let kind = if window_title == gui::TRAY_WINDOW {
            Kind::Receiver
        } else {
            Kind::Send
        };
        let app = app(mtm);
        if kind == Kind::Receiver {
            // Finder "Send with Hoppy" (Info.plist NSServices → `sendFiles:userData:error:`).
            // Only the long-running receiver: a `hoppyw send` process would take the
            // service port with it when it exits.
            let provider = delegate(mtm);
            let provider: &AnyObject = &provider;
            // SAFETY: the delegate implements the service method and lives as long as the app.
            unsafe { app.setServicesProvider(Some(provider)) };
        }
        start_notifications();
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let button = item
            .button(mtm)
            .context("the menu bar has no room for Hoppy")?;
        match template_icon() {
            Some(icon) => button.setImage(Some(&icon)),
            None => button.setTitle(&NSString::from_str("Hoppy")),
        }
        let target: &AnyObject = &delegate(mtm);
        // SAFETY: the delegate implements `statusClicked:` and lives as long as the app.
        unsafe {
            button.setTarget(Some(target));
            button.setAction(Some(sel!(statusClicked:)));
        }
        button.sendActionOn(NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown);
        let tray = Tray {
            kind,
            item,
            mtm,
            timers: Arc::new(Mutex::new(HashMap::new())),
        };
        tray.set_tip(tip);
        Ok(tray)
    }

    pub fn set_tip(&self, tip: &str) {
        let Some(button) = self.item.button(self.mtm) else {
            return;
        };
        button.setToolTip(Some(&NSString::from_str(tip)));
        if self.kind == Kind::Send {
            let title = format!(" {}", super::status_title(tip));
            button.setTitle(&NSString::from_str(&title));
        }
    }

    /// Menus are built at each click (`NATIVE_POPUPS`): nothing to keep.
    pub fn set_menu(&self, _items: &[MenuItem<'_>]) {}

    /// A notification; clicking it yields `TrayEvent::BalloonClick`. `sound`: the system
    /// sound (off when our jingle plays).
    pub fn balloon(&self, title: &str, text: &str, _error: bool, sound: bool) {
        notify(title, text, sound);
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

    /// A menu under the menu-bar icon (`above`: the icon was clicked) or at the mouse
    /// pointer (the device list). `None` = dismissed.
    pub fn popup_menu(
        &self,
        items: &[MenuItem<'_>],
        above: bool,
        _anchor: Option<(i32, i32)>,
    ) -> Option<u32> {
        let mtm = self.mtm;
        let app = app(mtm);
        let menu = build_menu(mtm, items);
        let target = delegate(mtm);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        PICKED.with(|p| p.set(None));
        MENU_SHOWN.with(|m| m.set(false));
        MENU_OPEN.with(|m| m.set(true));
        activate(&app);
        if above && let Some(button) = self.item.button(mtm) {
            // The status item shows its own menu, placed and highlighted by the system.
            self.item.setMenu(Some(&menu));
            // SAFETY: a plain click on our own button.
            unsafe { button.performClick(None) };
            self.item.setMenu(None);
        }
        if !MENU_SHOWN.with(Cell::get) {
            // At the pointer: the device list, or the icon's menu if the system did not
            // show it from the click above.
            let at = NSEvent::mouseLocation();
            menu.popUpMenuPositioningItem_atLocation_inView(None, at, None);
        }
        MENU_OPEN.with(|m| m.set(false));
        after_modal();
        let picked = PICKED.with(Cell::take);
        diag::log(format!(
            "menu: {} entries, picked={}",
            items.len(),
            picked.is_some()
        ));
        picked
            .and_then(|t| u32::try_from(t).ok())
            .filter(|&id| id != 0)
    }

    pub fn pick_files(&self, title: &str) -> Result<Vec<PathBuf>, TooManyFiles> {
        pick_files_unowned(title)
    }

    pub fn next_event(&self) -> Option<TrayEvent> {
        let app = app(self.mtm);
        loop {
            if let Ok(event) = lock(&events().rx).try_recv() {
                return Some(event);
            }
            pump_once(&app);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}

/// An `NSMenu` for `items`; picks land in `PICKED` (target `menuPicked:`).
fn build_menu(mtm: MainThreadMarker, items: &[MenuItem<'_>]) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let target = delegate(mtm);
    let target: &AnyObject = &target;
    let add = |title: &str, tag: Option<u32>| -> Retained<NSMenuItem> {
        let action = tag.map(|_| sel!(menuPicked:));
        // SAFETY: `menuPicked:` is implemented by the target set below.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                action,
                &NSString::from_str(""),
            )
        };
        if let Some(tag) = tag {
            item.setTag(tag as isize);
            // SAFETY: the delegate lives as long as the app.
            unsafe { item.setTarget(Some(target)) };
        }
        item.setEnabled(tag.is_some());
        menu.addItem(&item);
        item
    };
    for entry in items {
        match *entry {
            MenuItem::Action(id, text) => {
                add(text, Some(id));
            }
            MenuItem::Check(id, text, on) => {
                let item = add(text, Some(id));
                if on {
                    item.setState(NSControlStateValueOn);
                }
            }
            MenuItem::Bold(id, text) => {
                let item = add(text, Some(id));
                item.setAttributedTitle(Some(&bold(text)));
            }
            MenuItem::Label(text) => {
                add(text, None);
            }
            MenuItem::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
        }
    }
    menu
}

/// Menu text in the bold menu font (the update entry).
fn bold(text: &str) -> Retained<NSAttributedString> {
    let size = NSFont::menuFontOfSize(0.0).pointSize();
    let font = NSFont::boldSystemFontOfSize(size);
    // SAFETY: a valid attribute name constant.
    let key = unsafe { NSFontAttributeName };
    let value: &AnyObject = &font;
    let attributes = NSDictionary::from_slices(&[key], &[value]);
    // SAFETY: NSFontAttributeName takes an NSFont.
    unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&attributes),
        )
    }
}

// ---------------------------------------------------------------- CI smoke test

/// `hoppyw --smoke-test` (CI on macos-latest, debug build: objc2 checks every message's
/// types against the runtime): exercises the AppKit pieces that do not need a person.
/// Panics on failure.
pub fn smoke_test() -> i32 {
    let mtm = MainThreadMarker::new().expect("main thread");
    SMOKE.with(|s| s.set(true));
    let app = app(mtm);
    println!("language italian: {}", ui_is_italian());
    println!("in bundle: {}", in_bundle());

    // Status items (receiver + sender) with every kind of menu entry.
    let tray = Tray::new(gui::TRAY_WINDOW, "Hoppy · smoke test").expect("receiver icon");
    let send = Tray::new("Hoppy send", "Cerco dispositivi…").expect("send icon");
    send.set_tip("Invio a Pixel di Marco con un nome lungo… 42%");
    let menu = build_menu(
        mtm,
        &[
            MenuItem::Bold(5, "Update to 9.9.9…"),
            MenuItem::Separator,
            MenuItem::Label("Hoppy · receiving as smoke"),
            MenuItem::Action(1, "Send files…"),
            MenuItem::Check(4, "♪ Sound when delivered", true),
        ],
    );
    assert_eq!(menu.numberOfItems(), 5);
    assert!(template_icon().is_some(), "menu-bar icon PNG");
    assert!(
        NSSound::initWithData(NSSound::alloc(), &NSData::with_bytes(DONE_JINGLE)).is_some(),
        "jingle WAV"
    );

    // Pasteboard → files (the Finder service path).
    let pboard = NSPasteboard::pasteboardWithUniqueName();
    pboard.clearContents();
    let url = NSURL::fileURLWithPath(&NSString::from_str("/tmp/Hoppy smoke ✓.txt"));
    let objects = NSArray::from_retained_slice(&[url]);
    // SAFETY: NSURL implements NSPasteboardWriting.
    let written: bool = unsafe { msg_send![&pboard, writeObjects: &*objects] };
    assert!(written, "pasteboard write");
    assert_eq!(
        pasteboard_files(&pboard),
        [PathBuf::from("/tmp/Hoppy smoke ✓.txt")]
    );

    // Wake-ups from other threads, timers, work sent to the main thread.
    let waker = tray.waker();
    thread::spawn(move || waker.wake());
    assert_eq!(tray.next_event(), Some(TrayEvent::Wake));
    tray.set_timer(9, 50);
    assert_eq!(tray.next_event(), Some(TrayEvent::Timer(9)));
    tray.kill_timer(9);
    let answer = thread::spawn(|| {
        run_on_main(|_| {
            wake_main(); // ends the pump below
            42
        })
    });
    pump_once(&app);
    assert_eq!(answer.join().expect("run_on_main"), 42);
    while lock(&events().rx).try_recv().is_ok() {}

    if in_bundle() {
        // Only reads the settings: no permission prompt.
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let seen = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&seen);
        let block = RcBlock::new(
            move |_settings: std::ptr::NonNull<
                objc2_user_notifications::UNNotificationSettings,
            >| {
                flag.store(true, Ordering::Release);
                wake_main();
            },
        );
        center.getNotificationSettingsWithCompletionHandler(&block);
        for _ in 0..3 {
            if seen.load(Ordering::Acquire) {
                break;
            }
            pump_once(&app);
        }
        println!(
            "notification settings read: {}",
            seen.load(Ordering::Acquire)
        );
        println!("login item: {:?}", super::login::enabled());
    }
    drop(send);
    drop(tray);
    println!("smoke test OK");
    0
}
