//! The themed send window of `hoppyw send` (ADR 0011): one HTML/CSS/JS page in the
//! system webview (WebView2 / WKWebView / WebKitGTK, `window.rs`), the same on Windows,
//! macOS and Linux, in the theme chosen in the tray (`crate::theme`).
//!
//! This module is the pure part, tested on Linux: the screens and their transitions
//! ([`Flow`]), what the page shows ([`View`], JSON for `hoppy.render`), and the messages the
//! page may send ([`Action`], validated by [`parse_action`]). `gui.rs` runs the workers
//! (search, send, trust store) and turns their results into [`Flow`] inputs; the
//! [`Effect`]s say what to do next. The page never sees a path or an address: only cleaned
//! display names (`display_*`), sizes and the strings below.
//!
//! Screens: searching → devices | nobody found → sending (waiting for "Accept", then
//! progress; the 4-emoji verification panel inline) → delivered (auto-close) | declined |
//! failed | key changed ("Forget device").

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
pub mod smoke;
pub mod web;
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
pub mod window;
/// Other systems have no webview backend: the native picker is used.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub mod window {
    use super::{Action, Init, View};
    use crate::theme::Theme;
    use std::time::Duration;

    #[derive(Clone)]
    pub struct Waker;
    impl Waker {
        pub fn wake(&self) {}
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Input {
        Started,
        Ready,
        Action(Action),
        Wake,
    }
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Unavailable(pub String);
    impl std::fmt::Display for Unavailable {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    pub struct Control<'a>(std::marker::PhantomData<&'a ()>);
    impl Control<'_> {
        pub fn render(&mut self, _view: &View) {}
        pub fn set_on_top(&self, _on: bool) {}
        pub fn close_after(&mut self, _delay: Duration) {}
        pub fn exit(&mut self) {}
        pub fn waker(&self) -> Waker {
            Waker
        }
    }
    pub fn run(
        _title: &str,
        _theme: Theme,
        _init: &Init,
        _handler: impl FnMut(&mut Control<'_>, Input),
    ) -> Result<(), Unavailable> {
        Err(Unavailable("no webview on this system".into()))
    }
}

use std::time::Duration;

use hoppy_core::{SessionInfo, TrustEntry, TrustState};
use serde::{Deserialize, Serialize};

use crate::gui::{Strings, code_view, fill, peer_label};
use crate::theme::Theme;
use crate::verify::CodeView;
use crate::{display_name, display_text};

/// The delivered screen closes itself after this.
pub const DONE_CLOSE_AFTER: Duration = Duration::from_millis(3200);
/// Longest IPC message accepted from the page (bytes); real ones are < 40.
pub const IPC_MAX: usize = 256;
/// Most device cards shown (the list scrolls; a LAN rarely has more).
pub const MAX_DEVICES: usize = 64;

/// What the page may ask. Anything else (unknown type, extra fields, too long, an index
/// out of range, an action that makes no sense on the current screen) is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// The page loaded and can render.
    Ready,
    /// A device card (index into the shown list).
    Pick {
        i: u32,
    },
    Retry,
    /// "Cancel" / "Cancel sending" / the window's close button.
    Cancel,
    Close,
    /// Esc: the current screen's way out (Cancel or Close).
    Escape,
    /// "They match" in the verification panel.
    Match,
    /// "Later" in the verification panel.
    Later,
    /// "Forget device" on the key-changed screen.
    Forget,
}

/// Parses one IPC message from the page (see [`Action`]).
pub fn parse_action(raw: &str) -> Option<Action> {
    if raw.len() > IPC_MAX {
        return None;
    }
    // serde ignores extra fields next to a unit variant's tag: count them here.
    let serde_json::Value::Object(map) = serde_json::from_str(raw).ok()? else {
        return None;
    };
    let fields = if map.get("t")?.as_str()? == "pick" {
        2
    } else {
        1
    };
    if map.len() != fields {
        return None;
    }
    serde_json::from_value(serde_json::Value::Object(map)).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Screen {
    Searching,
    Devices,
    Empty,
    Sending,
    Done,
    Declined,
    Failed,
    KeyChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Badge {
    /// ✓ verified with the emoji code.
    Verified,
    /// 🔒 protocol v1 (encrypted), not verified.
    Encrypted,
    /// ⚠ an old app without encryption.
    Plain,
}

/// One device card (all text cleaned).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceCard {
    pub name: String,
    pub platform: String,
    /// The avatar letter.
    pub initial: String,
    pub badge: Badge,
    pub badge_label: String,
    /// Screen-reader label of the card ("Send to X, Android, Verified").
    pub aria: String,
}

/// The card for a found device; `known` is its trust-store entry.
pub fn device_card(s: &Strings, peer: &hoppy_core::Peer, known: Option<&TrustEntry>) -> DeviceCard {
    let name = display_name(&peer.device.name);
    let label = peer_label(peer);
    let platform = label
        .rsplit_once(" · ")
        .map(|(_, p)| p.to_owned())
        .unwrap_or_default();
    let badge = match known {
        Some(entry) if entry.verified => Badge::Verified,
        Some(_) => Badge::Encrypted,
        None if peer.crypto.is_some_and(|c| c >= 1) => Badge::Encrypted,
        None => Badge::Plain,
    };
    let badge_label = match badge {
        Badge::Verified => s.ui_badge_verified,
        Badge::Encrypted => s.ui_badge_encrypted,
        Badge::Plain => s.ui_badge_plain,
    }
    .to_owned();
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".to_owned());
    DeviceCard {
        aria: fill(
            s.ui_send_to,
            &[
                ("name", &name),
                ("platform", &platform),
                ("badge", &badge_label),
            ],
        ),
        name,
        platform,
        initial,
        badge,
        badge_label,
    }
}

/// "Invia «foto.jpg» +2 (3,4 MB) a": the first file's cleaned name (never a path).
pub fn header(s: &Strings, first_file: &str, count: usize, size: &str) -> String {
    let first = display_text(first_file, crate::gui::HEADER_FILE_MAX);
    let first = if first.is_empty() { "?".into() } else { first };
    match count.saturating_sub(1) {
        0 => fill(s.ui_header_one, &[("file", &first), ("size", size)]),
        more => fill(
            s.ui_header_more,
            &[
                ("file", &first),
                ("more", &more.to_string()),
                ("size", size),
            ],
        ),
    }
}

/// How a send ended (from `send_to_peer`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Delivered,
    /// The receiver said no; `Some(reason)` for a protocol limit (cleaned text).
    Declined(Option<String>),
    /// Neither the direct connection nor the callback reached it.
    Unreachable,
    /// A remembered device presented another key: nothing was sent.
    KeyChanged,
    Failed(String),
}

/// What `gui.rs` must do after an input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Search again (refresh discovery, new search worker).
    Search,
    /// Send the files to the device at this index of the last search.
    Send(usize),
    /// "They match": mark the receiver verified (worker), then [`Flow::verify_result`].
    MarkVerified,
    /// "Forget device" (worker), then [`Flow::forget_result`].
    Forget,
    /// Play the delivery jingle (unless muted).
    Jingle,
    /// Keep the window above others while the user chooses.
    OnTop(bool),
    /// Close the window after this long (delivered).
    CloseAfter(Duration),
    /// Close the window and end `hoppyw send` (aborts a running transfer).
    Exit,
}

/// The verification panel (sending to a New/Known encrypted device).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Panel {
    Open,
    /// "They match" pressed, saving.
    Saving,
    /// Saved (`true`) or failed (`false`), with the message.
    Result(bool, String),
    /// "Later", or nothing to compare.
    Closed,
}

/// The send window's state machine. Inputs come from the page ([`Flow::action`]) and from
/// the workers ([`Flow::found`], [`Flow::session`], [`Flow::progress`], [`Flow::outcome`],
/// [`Flow::verify_result`], [`Flow::forget_result`]).
#[derive(Debug, Clone)]
pub struct Flow {
    screen: Screen,
    devices: Vec<DeviceCard>,
    /// Display name of the device being sent to.
    target: String,
    session: Option<SessionInfo>,
    accepted: bool,
    pct: Option<u8>,
    fallback: bool,
    panel: Panel,
    /// Declined/failed details (cleaned).
    detail: String,
    /// Key changed: `None` = not forgotten yet; `Some((ok, message))` after the attempt;
    /// `forgetting` while it runs.
    forgot: Option<(bool, String)>,
    forgetting: bool,
}

impl Default for Flow {
    fn default() -> Self {
        Self::new()
    }
}

impl Flow {
    /// Searching (the window opens while the first search runs).
    pub fn new() -> Self {
        Flow {
            screen: Screen::Searching,
            devices: Vec::new(),
            target: String::new(),
            session: None,
            accepted: false,
            pct: None,
            fallback: false,
            panel: Panel::Closed,
            detail: String::new(),
            forgot: None,
            forgetting: false,
        }
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// The search finished.
    pub fn found(&mut self, mut devices: Vec<DeviceCard>) -> Vec<Effect> {
        if self.screen != Screen::Searching {
            return Vec::new();
        }
        devices.truncate(MAX_DEVICES);
        self.screen = if devices.is_empty() {
            Screen::Empty
        } else {
            Screen::Devices
        };
        self.devices = devices;
        vec![Effect::OnTop(true)]
    }

    /// The session with the receiver is set up (spec §7): what to show while it decides.
    pub fn session(&mut self, session: SessionInfo, italian_names: bool) {
        if self.screen != Screen::Sending || self.session.is_some() {
            return;
        }
        let has_code = self.code_for(&session, italian_names).is_some();
        self.panel = if has_code { Panel::Open } else { Panel::Closed };
        self.session = Some(session);
    }

    fn code_for(&self, session: &SessionInfo, italian: bool) -> Option<CodeView> {
        let s = if italian {
            &crate::gui::IT
        } else {
            &crate::gui::EN
        };
        code_view(s, session)
    }

    /// Transfer progress: accepted (first file started), percentage, callback fallback.
    pub fn progress(&mut self, accepted: bool, pct: Option<u64>, fallback: bool) {
        if self.screen != Screen::Sending {
            return;
        }
        self.accepted |= accepted;
        if let Some(p) = pct {
            self.pct = Some(p.min(100) as u8);
        }
        self.fallback = fallback;
    }

    /// The send ended.
    pub fn outcome(&mut self, outcome: Outcome) -> Vec<Effect> {
        if self.screen != Screen::Sending {
            return Vec::new();
        }
        if matches!(self.panel, Panel::Open) {
            self.panel = Panel::Closed; // the code window closes with the transfer
        }
        self.detail.clear();
        match outcome {
            Outcome::Delivered => {
                self.screen = Screen::Done;
                self.pct = Some(100);
                return vec![Effect::Jingle, Effect::CloseAfter(DONE_CLOSE_AFTER)];
            }
            Outcome::Declined(reason) => {
                self.screen = Screen::Declined;
                self.detail = reason.unwrap_or_default();
            }
            Outcome::Unreachable => {
                self.screen = Screen::Failed;
                self.detail = String::from("unreachable");
            }
            Outcome::KeyChanged => {
                self.screen = Screen::KeyChanged;
                self.forgot = None;
                self.forgetting = false;
            }
            Outcome::Failed(e) => {
                self.screen = Screen::Failed;
                self.detail = format!("error:{e}");
            }
        }
        vec![Effect::OnTop(true)]
    }

    /// "They match" was saved (`ok`) or not; `message` is what to show.
    pub fn verify_result(&mut self, ok: bool, message: String) {
        if self.panel == Panel::Saving {
            self.panel = Panel::Result(ok, message);
        }
    }

    /// "Forget device" finished.
    pub fn forget_result(&mut self, ok: bool, message: String) {
        if self.screen == Screen::KeyChanged && self.forgetting {
            self.forgetting = false;
            self.forgot = Some((ok, message));
        }
    }

    /// A message from the page. Actions that do not fit the screen are ignored.
    pub fn action(&mut self, action: Action) -> Vec<Effect> {
        match (action, self.screen) {
            (Action::Ready, _) => Vec::new(),
            (Action::Pick { i }, Screen::Devices) => {
                let Some(card) = self.devices.get(i as usize) else {
                    return Vec::new();
                };
                self.target = card.name.clone();
                self.screen = Screen::Sending;
                self.session = None;
                self.accepted = false;
                self.pct = None;
                self.fallback = false;
                self.panel = Panel::Closed;
                vec![Effect::OnTop(false), Effect::Send(i as usize)]
            }
            (
                Action::Retry,
                Screen::Devices | Screen::Empty | Screen::Declined | Screen::Failed,
            ) => self.search_again(),
            (Action::Retry, Screen::KeyChanged) if matches!(self.forgot, Some((true, _))) => {
                self.search_again()
            }
            (Action::Cancel | Action::Close | Action::Escape, _) => vec![Effect::Exit],
            (Action::Match, Screen::Sending) if self.panel == Panel::Open => {
                self.panel = Panel::Saving;
                vec![Effect::MarkVerified]
            }
            (Action::Later, Screen::Sending) if self.panel == Panel::Open => {
                self.panel = Panel::Closed;
                Vec::new()
            }
            (Action::Forget, Screen::KeyChanged) if self.forgot.is_none() && !self.forgetting => {
                self.forgetting = true;
                vec![Effect::Forget]
            }
            _ => Vec::new(),
        }
    }

    fn search_again(&mut self) -> Vec<Effect> {
        self.screen = Screen::Searching;
        self.devices.clear();
        self.session = None;
        self.forgot = None;
        self.forgetting = false;
        vec![Effect::OnTop(true), Effect::Search]
    }

    /// What the page shows now. `header`: [`header`]'s line about the files.
    pub fn view(&self, s: &Strings, header: &str) -> View {
        let name = self.target.as_str();
        let mut v = View {
            screen: self.screen,
            mascot: Mascot::Idle,
            decor: "",
            header: header.to_owned(),
            title: String::new(),
            text: String::new(),
            tone: Tone::Normal,
            busy: false,
            tips: Vec::new(),
            list_label: s.ui_devices.to_owned(),
            devices: Vec::new(),
            progress: None,
            note: String::new(),
            code: None,
            buttons: Vec::new(),
        };
        let button = |action, label: &str, primary| Button {
            action,
            label: label.to_owned(),
            primary,
        };
        match self.screen {
            Screen::Searching => {
                v.decor = "だれかいる？";
                v.title = s.ui_searching.to_owned();
                v.text = s.ui_searching_text.to_owned();
                v.busy = true;
                v.buttons = vec![button("cancel", s.cancel, false)];
            }
            Screen::Devices => {
                v.decor = "近くにいるよ";
                v.title = s.ui_pick.to_owned();
                v.devices = self.devices.clone();
                v.buttons = vec![
                    button("retry", s.retry, false),
                    button("cancel", s.cancel, false),
                ];
            }
            Screen::Empty => {
                v.decor = "だれかいる？";
                v.mascot = Mascot::Look;
                v.title = s.ui_none_title.to_owned();
                v.text = s.ui_none_text.to_owned();
                v.tips = vec![
                    s.ui_tip_open.to_owned(),
                    s.ui_tip_wifi.to_owned(),
                    s.ui_tip_firewall.to_owned(),
                ];
                v.buttons = vec![
                    button("retry", s.retry, true),
                    button("cancel", s.cancel, false),
                ];
            }
            Screen::Sending => {
                v.decor = "送信中";
                if self.accepted {
                    v.title = fill(s.ui_sending, &[("name", name)]);
                    v.progress = Some(ProgressView {
                        pct: Some(self.pct.unwrap_or(0)),
                    });
                } else {
                    v.title = if self.fallback {
                        fill(s.fallback, &[("name", name)])
                    } else {
                        fill(s.waiting, &[("name", name)])
                    };
                    v.progress = Some(ProgressView { pct: None });
                }
                if let Some(session) = &self.session {
                    self.security(s, session, &mut v);
                }
                v.buttons = vec![button("cancel", s.cancel_send, false)];
            }
            Screen::Done => {
                v.decor = "とどいた！";
                v.mascot = Mascot::Happy;
                v.tone = Tone::Ok;
                v.title = fill(s.delivered, &[("name", name)]);
                v.text = s.ui_done_text.to_owned();
                v.progress = Some(ProgressView { pct: Some(100) });
                v.buttons = vec![button("close", s.close, true)];
            }
            Screen::Declined => {
                v.tone = Tone::Warn;
                v.title = fill(s.declined, &[("name", name)]);
                v.text = if self.detail.is_empty() {
                    s.ui_declined_text.to_owned()
                } else {
                    self.detail.clone()
                };
                v.buttons = vec![
                    button("retry", s.ui_retry, true),
                    button("close", s.close, false),
                ];
            }
            Screen::Failed => {
                v.tone = Tone::Error;
                if self.detail == "unreachable" {
                    v.title = fill(s.unreachable_title, &[("name", name)]);
                    v.text = s.unreachable_text.to_owned();
                } else {
                    v.title = fill(s.send_failed, &[("name", name)]);
                    v.text = self
                        .detail
                        .strip_prefix("error:")
                        .unwrap_or_default()
                        .to_owned();
                }
                v.buttons = vec![
                    button("retry", s.ui_retry, true),
                    button("close", s.close, false),
                ];
            }
            Screen::KeyChanged => {
                v.tone = Tone::Error;
                v.title = fill(s.key_changed_title, &[("name", name)]);
                match &self.forgot {
                    None => {
                        v.text = fill(s.key_changed_text, &[("name", name)]);
                        // Close first: forgetting is offered, never the default.
                        v.buttons = vec![
                            button("close", s.close, false),
                            button("forget", s.forget_device, false),
                        ];
                    }
                    Some((true, message)) => {
                        v.tone = Tone::Normal;
                        v.text = message.clone();
                        v.buttons = vec![
                            button("retry", s.ui_retry, true),
                            button("close", s.close, false),
                        ];
                    }
                    Some((false, message)) => {
                        v.text = message.clone();
                        v.buttons = vec![button("close", s.close, false)];
                    }
                }
            }
        }
        v
    }

    /// The security line and the verification panel while sending.
    fn security(&self, s: &Strings, session: &SessionInfo, v: &mut View) {
        let name = self.target.as_str();
        let mut note = match (session.encrypted, session.trust) {
            (true, Some(TrustState::Verified)) => fill(s.sec_verified, &[("name", name)]),
            (true, _) => String::new(), // the panel (or its result) says it
            (false, _) => {
                v.tone = Tone::Warn;
                fill(s.sec_plain, &[("name", name)])
            }
        };
        if session.name_matches_known {
            v.tone = Tone::Warn;
            if !note.is_empty() {
                note.push('\n');
            }
            note.push_str(s.sec_name_clash);
        }
        v.note = note;
        let Some(code) = code_view(s, session) else {
            return;
        };
        let state = match &self.panel {
            Panel::Closed => return,
            Panel::Open => PanelState::Open,
            Panel::Saving => PanelState::Saving,
            Panel::Result(true, _) => PanelState::Done,
            Panel::Result(false, _) => PanelState::Error,
        };
        let result = match &self.panel {
            Panel::Result(_, m) => m.clone(),
            _ => String::new(),
        };
        v.code = Some(CodePanelView {
            label: s.ui_code.to_owned(),
            intro: fill(s.ui_verify, &[("name", name)]),
            emoji: code
                .emoji
                .iter()
                .zip(code.names)
                .map(|(e, n)| EmojiView {
                    emoji: (*e).to_owned(),
                    name: n.to_owned(),
                })
                .collect(),
            match_label: s.match_only.to_owned(),
            later_label: s.later.to_owned(),
            state,
            result,
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mascot {
    /// The theme's bunny, bobbing (Night: the one with sunglasses).
    Idle,
    /// Looking around (nobody found).
    Look,
    /// "^ ^" celebrating a delivery.
    Happy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Normal,
    Ok,
    Warn,
    Error,
}

/// A footer button; `action` is the [`Action`] it sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Button {
    pub action: &'static str,
    pub label: String,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProgressView {
    /// `None` = indeterminate (waiting for "Accept").
    pub pct: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelState {
    Open,
    Saving,
    Done,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmojiView {
    pub emoji: String,
    /// Shown under the emoji and read by screen readers.
    pub name: String,
}

/// The inline verification panel: the same 4 emoji as on the receiver's screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodePanelView {
    pub label: String,
    pub intro: String,
    pub emoji: Vec<EmojiView>,
    pub match_label: String,
    pub later_label: String,
    pub state: PanelState,
    /// "X is now verified ✓" or the error, once saved.
    pub result: String,
}

/// Everything the page shows; serialised to JSON for `hoppy.render(view)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct View {
    pub screen: Screen,
    pub mascot: Mascot,
    /// Japanese decor sticker (docs/design/themes.md, not translated; hidden from screen
    /// readers).
    pub decor: &'static str,
    pub header: String,
    pub title: String,
    pub text: String,
    pub tone: Tone,
    /// The searching animation.
    pub busy: bool,
    pub tips: Vec<String>,
    pub list_label: String,
    pub devices: Vec<DeviceCard>,
    pub progress: Option<ProgressView>,
    /// Security line while sending (not encrypted / verified / name clash).
    pub note: String,
    pub code: Option<CodePanelView>,
    pub buttons: Vec<Button>,
}

/// Sent once when the page is ready: theme and language.
#[derive(Debug, Clone, Serialize)]
pub struct Init {
    pub theme: &'static str,
    pub lang: &'static str,
    pub title: String,
}

impl Init {
    pub fn new(s: &Strings, theme: Theme) -> Self {
        Init {
            theme: theme.id(),
            lang: if s.italian { "it" } else { "en" },
            title: s.ui_window_title.to_owned(),
        }
    }
}

/// The script that hands `value` to the page function `func` (`hoppy.render`/`hoppy.init`).
/// JSON is a valid JS expression; serde escapes quotes and control characters.
pub fn script_call(func: &str, value: &impl Serialize) -> String {
    let json = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
    format!("window.hoppy&&window.hoppy.{func}({json});")
}

/// Every screen with made-up data, in flow order: the smoke test (`hoppyw
/// --smoke-test-window`), the screenshots and `tools/sendui_audit.py` render these.
pub fn sample_views(s: &Strings) -> Vec<(&'static str, View)> {
    use hoppy_core::{PeerKey, Sas};
    let header = header(s, "foto-mare.jpg", 3, "3.4 MB");
    let card = |name: &str, platform: &str, badge, label: &str| DeviceCard {
        name: name.into(),
        platform: platform.into(),
        initial: name.chars().next().unwrap_or('?').to_uppercase().collect(),
        badge,
        badge_label: label.into(),
        aria: fill(
            s.ui_send_to,
            &[("name", name), ("platform", platform), ("badge", label)],
        ),
    };
    let devices = vec![
        card(
            "Pixel di Marco",
            "Android",
            Badge::Verified,
            s.ui_badge_verified,
        ),
        card(
            "MacBook Air",
            "macOS",
            Badge::Encrypted,
            s.ui_badge_encrypted,
        ),
        card("PC-Ufficio", "Windows", Badge::Plain, s.ui_badge_plain),
    ];
    let session = |trust, encrypted| SessionInfo {
        encrypted,
        trust: encrypted.then_some(trust),
        sas: encrypted.then(|| Sas::from_value(0x05_1234)),
        peer: encrypted.then(|| PeerKey {
            id: "sample".into(),
            name: "MacBook Air".into(),
            platform: "macos".into(),
            fingerprint: "ABCDEFGH".into(),
        }),
        name_matches_known: false,
    };
    let mut out = Vec::new();
    let mut f = Flow::new();
    out.push(("searching", f.view(s, &header)));
    f.found(devices.clone());
    out.push(("devices", f.view(s, &header)));
    let mut empty = Flow::new();
    empty.found(Vec::new());
    out.push(("empty", empty.view(s, &header)));
    f.action(Action::Pick { i: 1 });
    f.session(session(TrustState::New, true), s.italian);
    out.push(("verify", f.view(s, &header)));
    f.action(Action::Match);
    f.verify_result(true, fill(s.verified_title, &[("name", "MacBook Air")]));
    f.progress(true, Some(42), false);
    out.push(("sending", f.view(s, &header)));
    let mut plain = Flow::new();
    plain.found(devices.clone());
    plain.action(Action::Pick { i: 2 });
    plain.session(session(TrustState::New, false), s.italian);
    out.push(("waiting_plain", plain.view(s, &header)));
    f.outcome(Outcome::Delivered);
    out.push(("done", f.view(s, &header)));
    let ended = |outcome: Outcome| {
        let mut g = Flow::new();
        g.found(devices.clone());
        g.action(Action::Pick { i: 0 });
        g.outcome(outcome);
        g
    };
    out.push(("declined", ended(Outcome::Declined(None)).view(s, &header)));
    out.push(("unreachable", ended(Outcome::Unreachable).view(s, &header)));
    out.push(("key_changed", ended(Outcome::KeyChanged).view(s, &header)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::{EN, IT};
    use hoppy_core::{DeviceInfo, Peer, PeerKey, Sas};

    fn card(name: &str, badge: Badge) -> DeviceCard {
        DeviceCard {
            name: name.into(),
            platform: "Android".into(),
            initial: name[..1].to_uppercase(),
            badge,
            badge_label: "x".into(),
            aria: format!("Send to {name}"),
        }
    }

    fn session(trust: Option<TrustState>, encrypted: bool) -> SessionInfo {
        SessionInfo {
            encrypted,
            trust,
            sas: encrypted.then(|| Sas::from_value(0x05_1234)),
            peer: Some(PeerKey {
                id: "id-1".into(),
                name: "Pixel".into(),
                platform: "android".into(),
                fingerprint: "ABCDEFGHIJKL".into(),
            }),
            name_matches_known: false,
        }
    }

    fn sending_to_pixel() -> Flow {
        let mut f = Flow::new();
        f.found(vec![
            card("Pixel", Badge::Encrypted),
            card("PC", Badge::Plain),
        ]);
        assert_eq!(
            f.action(Action::Pick { i: 0 }),
            [Effect::OnTop(false), Effect::Send(0)]
        );
        f
    }

    #[test]
    fn samples_cover_every_screen() {
        for s in [&IT, &EN] {
            let views = sample_views(s);
            let screens: std::collections::HashSet<_> =
                views.iter().map(|(_, v)| v.screen).collect();
            assert_eq!(screens.len(), 8, "every Screen variant");
            for (name, v) in &views {
                assert!(!v.title.is_empty(), "{name}");
                assert!(!v.buttons.is_empty(), "{name}: a way out");
            }
            let verify = &views.iter().find(|(n, _)| *n == "verify").unwrap().1;
            assert_eq!(verify.code.as_ref().unwrap().emoji.len(), 4);
        }
    }

    #[test]
    fn ipc_messages_are_validated() {
        assert_eq!(parse_action(r#"{"t":"ready"}"#), Some(Action::Ready));
        assert_eq!(
            parse_action(r#"{"t":"pick","i":3}"#),
            Some(Action::Pick { i: 3 })
        );
        assert_eq!(parse_action(r#"{"t":"escape"}"#), Some(Action::Escape));
        for bad in [
            "",
            "null",
            r#"{"t":"pick"}"#,
            r#"{"t":"pick","i":-1}"#,
            r#"{"t":"pick","i":"0"}"#,
            r#"{"t":"pick","i":1,"path":"/etc"}"#,
            r#"{"t":"ready","x":1}"#,
            r#"{"t":"open","url":"https://example.org"}"#,
            r#"{"t":"READY"}"#,
            r#"[{"t":"ready"}]"#,
        ] {
            assert_eq!(parse_action(bad), None, "{bad}");
        }
        let long = format!(r#"{{"t":"ready"{}}}"#, " ".repeat(IPC_MAX));
        assert_eq!(parse_action(&long), None);
    }

    #[test]
    fn search_finds_devices_or_nobody() {
        let mut f = Flow::new();
        assert_eq!(f.screen(), Screen::Searching);
        assert_eq!(f.found(vec![]), [Effect::OnTop(true)]);
        assert_eq!(f.screen(), Screen::Empty);
        // A stale search result after leaving the searching screen is ignored.
        assert!(f.found(vec![card("Late", Badge::Plain)]).is_empty());
        assert_eq!(
            f.action(Action::Retry),
            [Effect::OnTop(true), Effect::Search]
        );
        assert_eq!(f.screen(), Screen::Searching);
        f.found(vec![card("Pixel", Badge::Verified)]);
        assert_eq!(f.screen(), Screen::Devices);
        let many: Vec<DeviceCard> = (0..100)
            .map(|i| card(&format!("D{i}"), Badge::Plain))
            .collect();
        let mut g = Flow::new();
        g.found(many);
        assert_eq!(g.view(&EN, "h").devices.len(), MAX_DEVICES);
    }

    #[test]
    fn picking_checks_the_index_and_the_screen() {
        let mut f = Flow::new();
        // Nothing to pick while searching.
        assert!(f.action(Action::Pick { i: 0 }).is_empty());
        f.found(vec![card("Pixel", Badge::Encrypted)]);
        assert!(f.action(Action::Pick { i: 1 }).is_empty(), "out of range");
        assert!(f.action(Action::Pick { i: u32::MAX }).is_empty());
        assert_eq!(f.screen(), Screen::Devices);
        assert_eq!(
            f.action(Action::Pick { i: 0 }),
            [Effect::OnTop(false), Effect::Send(0)]
        );
        assert_eq!(f.screen(), Screen::Sending);
        // A second pick (double click) does not start a second send.
        assert!(f.action(Action::Pick { i: 0 }).is_empty());
        assert!(f.action(Action::Retry).is_empty(), "no retry while sending");
    }

    #[test]
    fn every_screen_has_a_way_out() {
        for a in [Action::Cancel, Action::Close, Action::Escape] {
            let mut f = sending_to_pixel();
            assert_eq!(f.action(a), [Effect::Exit]);
            assert_eq!(Flow::new().action(a), [Effect::Exit]);
        }
    }

    #[test]
    fn delivery_celebrates_and_closes() {
        let mut f = sending_to_pixel();
        f.progress(true, Some(42), false);
        let v = f.view(&IT, "Invia «a.jpg» (1 KB) a");
        assert_eq!(v.title, "Invio a Pixel…");
        assert_eq!(v.progress, Some(ProgressView { pct: Some(42) }));
        assert_eq!(v.buttons[0].label, IT.cancel_send);
        let fx = f.outcome(Outcome::Delivered);
        assert_eq!(fx, [Effect::Jingle, Effect::CloseAfter(DONE_CLOSE_AFTER)]);
        let v = f.view(&IT, "");
        assert_eq!(
            (v.screen, v.mascot, v.tone),
            (Screen::Done, Mascot::Happy, Tone::Ok)
        );
        assert_eq!(v.title, "Consegnato a Pixel ✓");
        assert_eq!(v.decor, "とどいた！");
        // A second outcome (stale) changes nothing.
        assert!(f.outcome(Outcome::Unreachable).is_empty());
    }

    #[test]
    fn waiting_shows_fallback_and_indeterminate_progress() {
        let mut f = sending_to_pixel();
        let v = f.view(&EN, "");
        assert_eq!(v.title, "Waiting for Pixel to accept…");
        assert_eq!(v.progress, Some(ProgressView { pct: None }));
        f.progress(false, None, true);
        assert_eq!(
            f.view(&EN, "").title,
            "Pixel is not answering, trying another way…"
        );
    }

    #[test]
    fn verification_panel_inline() {
        let mut f = sending_to_pixel();
        f.session(session(Some(TrustState::New), true), false);
        let v = f.view(&EN, "");
        let code = v.code.expect("panel for a new device");
        assert_eq!(code.state, PanelState::Open);
        assert_eq!(code.emoji.len(), 4);
        assert!(
            code.emoji
                .iter()
                .all(|e| !e.emoji.is_empty() && !e.name.is_empty())
        );
        assert_eq!(code.match_label, "They match");
        assert_eq!(code.later_label, "Later");
        assert!(code.intro.contains("Pixel"));
        // A second session event does not reopen anything.
        f.session(session(Some(TrustState::New), true), false);
        assert_eq!(f.action(Action::Match), [Effect::MarkVerified]);
        assert!(f.action(Action::Match).is_empty(), "only once");
        assert_eq!(f.view(&EN, "").code.unwrap().state, PanelState::Saving);
        f.verify_result(true, "Pixel is now verified ✓".into());
        let code = f.view(&EN, "").code.unwrap();
        assert_eq!(
            (code.state, code.result.as_str()),
            (PanelState::Done, "Pixel is now verified ✓")
        );
        // The transfer goes on: progress with the panel's result still visible.
        f.progress(true, Some(10), false);
        assert!(f.view(&EN, "").code.is_some());

        // "Later" closes it for this send.
        let mut f = sending_to_pixel();
        f.session(session(Some(TrustState::Known), true), true);
        assert_eq!(f.view(&IT, "").code.unwrap().match_label, "Coincide");
        assert!(f.action(Action::Later).is_empty());
        assert!(f.view(&IT, "").code.is_none());
        assert!(f.action(Action::Match).is_empty(), "closed panel");
    }

    #[test]
    fn security_notes_without_panel() {
        let mut f = sending_to_pixel();
        f.session(session(Some(TrustState::Verified), true), false);
        let v = f.view(&EN, "");
        assert!(v.code.is_none());
        assert!(v.note.contains("verified"), "{}", v.note);
        assert!(f.action(Action::Match).is_empty());

        let mut f = sending_to_pixel();
        f.session(session(None, false), false);
        let v = f.view(&EN, "");
        assert!(v.code.is_none());
        assert_eq!(v.tone, Tone::Warn);
        assert!(v.note.starts_with("⚠ Not encrypted"), "{}", v.note);

        let mut f = sending_to_pixel();
        let mut clash = session(Some(TrustState::New), true);
        clash.name_matches_known = true;
        f.session(clash, false);
        let v = f.view(&EN, "");
        assert_eq!(v.tone, Tone::Warn);
        assert_eq!(v.note, EN.sec_name_clash);
        assert!(v.code.is_some());
    }

    #[test]
    fn failures_offer_retry_and_close() {
        let mut f = sending_to_pixel();
        f.session(session(Some(TrustState::New), true), false);
        f.outcome(Outcome::Declined(None));
        let v = f.view(&IT, "");
        assert_eq!((v.screen, v.tone), (Screen::Declined, Tone::Warn));
        assert_eq!(v.title, "Pixel ha rifiutato");
        assert!(v.code.is_none(), "panel closes with the transfer");
        let actions: Vec<&str> = v.buttons.iter().map(|b| b.action).collect();
        assert_eq!(actions, ["retry", "close"]);
        assert_eq!(
            f.action(Action::Retry),
            [Effect::OnTop(true), Effect::Search]
        );

        let mut f = sending_to_pixel();
        f.outcome(Outcome::Unreachable);
        let v = f.view(&EN, "");
        assert_eq!(v.title, "Can't reach \"Pixel\"");
        assert_eq!(v.text, EN.unreachable_text);

        let mut f = sending_to_pixel();
        f.outcome(Outcome::Failed("connection reset".into()));
        let v = f.view(&EN, "");
        assert_eq!(
            (v.title.as_str(), v.text.as_str()),
            ("Sending to Pixel failed", "connection reset")
        );

        let mut f = sending_to_pixel();
        f.outcome(Outcome::Declined(Some("too many files".into())));
        assert_eq!(f.view(&EN, "").text, "too many files");
    }

    #[test]
    fn key_change_offers_forget_never_by_default() {
        let mut f = sending_to_pixel();
        f.outcome(Outcome::KeyChanged);
        let v = f.view(&EN, "");
        assert_eq!((v.screen, v.tone), (Screen::KeyChanged, Tone::Error));
        assert_eq!(v.buttons[0].action, "close");
        assert!(!v.buttons.iter().any(|b| b.primary));
        assert!(f.action(Action::Retry).is_empty(), "not before forgetting");
        assert_eq!(f.action(Action::Forget), [Effect::Forget]);
        assert!(f.action(Action::Forget).is_empty(), "once");
        f.forget_result(true, "“Pixel” forgotten".into());
        let v = f.view(&EN, "");
        assert_eq!(v.text, "“Pixel” forgotten");
        assert_eq!(v.buttons[0].action, "retry");
        assert_eq!(
            f.action(Action::Retry),
            [Effect::OnTop(true), Effect::Search]
        );

        let mut f = sending_to_pixel();
        f.outcome(Outcome::KeyChanged);
        f.action(Action::Forget);
        f.forget_result(false, "Could not forget".into());
        let v = f.view(&EN, "");
        assert_eq!(v.buttons.len(), 1);
        assert!(f.action(Action::Retry).is_empty());
    }

    #[test]
    fn device_cards_from_peers() {
        let peer = |name: &str, platform: &str, crypto| Peer {
            device: DeviceInfo {
                id: format!("id-{name}"),
                name: name.into(),
                platform: platform.into(),
            },
            addr: "192.0.2.1:47471".parse().unwrap(),
            crypto,
        };
        let c = device_card(&IT, &peer("pixel\u{202E}8", "android", Some(1)), None);
        assert_eq!(c.name, "pixel8", "bidi controls removed");
        assert_eq!((c.platform.as_str(), c.initial.as_str()), ("Android", "P"));
        assert_eq!(
            (c.badge, c.badge_label.as_str()),
            (Badge::Encrypted, "Cifrato")
        );
        assert_eq!(c.aria, "Invia a pixel8, Android, Cifrato");
        let old = device_card(&EN, &peer("🐰 Den", "linux", None), None);
        assert_eq!((old.badge, old.initial.as_str()), (Badge::Plain, "D"));
        let json = serde_json::to_string(&old).unwrap();
        assert!(
            !json.contains("192.0.2.1"),
            "no addresses in the page: {json}"
        );
    }

    #[test]
    fn header_and_view_json_carry_no_paths() {
        let h = header(&IT, "foto\u{202E}gpj.exe", 3, "3,4 MB");
        assert_eq!(h, "Invia «fotogpj.exe» +2 (3,4 MB) a");
        assert_eq!(header(&EN, "", 1, "1 KB"), "Send “?” (1 KB) to");
        let f = Flow::new();
        let js = script_call("render", &f.view(&IT, &h));
        assert!(js.starts_with("window.hoppy&&window.hoppy.render({\"screen\":\"searching\""));
        let init = script_call("init", &Init::new(&IT, Theme::Yume));
        assert!(init.contains("\"theme\":\"yume\"") && init.contains("\"lang\":\"it\""));
        // Hostile text stays inside a JSON string.
        let evil = header(&EN, "\"</script><img src=x onerror=alert(1)>", 1, "1 B");
        let js = script_call("render", &evil);
        assert!(js.contains(r#"\"</script>"#), "{js}");
    }
}
