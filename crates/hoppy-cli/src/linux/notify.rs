//! freedesktop notifications (`org.freedesktop.Notifications`) over the session bus, with
//! actions: the tray's "balloons", the accept question (Accept/Decline buttons) and the
//! send progress (updated in place, "Cancel sending" button).
//!
//! One connection per process, opened on first use. A dispatcher thread reads the
//! `ActionInvoked`/`NotificationClosed` signals and hands them to whoever waits for that
//! notification (`listen`), else to the fallback handler (the tray), else keeps the last
//! few for a `listen` that comes a moment later.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

use crate::diag;

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// Shown by notification servers as the sender; matches the `.desktop` file id.
pub const APP_NAME: &str = "Hoppy";
pub const APP_ICON: &str = "hoppy";
/// Action key of a click on the notification body.
pub const DEFAULT_ACTION: &str = "default";
const ORPHANS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    Action(String),
    /// Reason: 1 expired, 2 dismissed by the user, 3 closed by us, 4 other.
    Closed(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgency {
    Low = 0,
    Normal = 1,
    /// Stays on screen until answered (GNOME, KDE).
    Critical = 2,
}

/// One notification. `actions`: (key, label) pairs, shown as buttons; `DEFAULT_ACTION`
/// is the body click (no button).
pub struct Note<'a> {
    pub summary: &'a str,
    pub body: &'a str,
    pub actions: &'a [(&'a str, &'a str)],
    pub urgency: Urgency,
    /// Not kept in the notification list once it goes away (progress, searching).
    pub transient: bool,
    /// `Some(false)`: no sound (our jingle plays instead).
    pub sound: Option<bool>,
    /// Update this notification in place (0 = new).
    pub replaces: u32,
    /// Milliseconds; -1 = server default, 0 = never expires.
    pub timeout_ms: i32,
    /// Progress 0..=100 (shown as a bar by servers that know the hint).
    pub value: Option<u8>,
}

impl Default for Note<'_> {
    fn default() -> Self {
        Note {
            summary: "",
            body: "",
            actions: &[],
            urgency: Urgency::Normal,
            transient: false,
            sound: None,
            replaces: 0,
            timeout_ms: -1,
            value: None,
        }
    }
}

type Fallback = Box<dyn Fn(u32, &Signal) + Send>;

#[derive(Default)]
struct Routes {
    listeners: HashMap<u32, Sender<Signal>>,
    fallback: Option<Fallback>,
    orphans: VecDeque<(u32, Signal)>,
}

pub struct Notifier {
    proxy: Proxy<'static>,
    caps: Vec<String>,
    routes: Arc<Mutex<Routes>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The process-wide notifier; `None` without a session bus or notification server (logged
/// once).
pub fn get() -> Option<&'static Notifier> {
    static NOTIFIER: OnceLock<Option<Notifier>> = OnceLock::new();
    NOTIFIER
        .get_or_init(|| match Notifier::connect() {
            Ok(n) => {
                diag::log(format!("notify: server capabilities {:?}", n.caps));
                Some(n)
            }
            Err(e) => {
                diag::log(format!("notify: unavailable: {e}"));
                None
            }
        })
        .as_ref()
}

/// Escapes text for servers that render body markup (a device called `<b>` stays literal).
pub fn escape_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// The flat `as` action list of the Notify call.
pub fn action_list<'a>(actions: &[(&'a str, &'a str)]) -> Vec<&'a str> {
    actions.iter().flat_map(|(k, l)| [*k, *l]).collect()
}

impl Notifier {
    pub fn connect() -> zbus::Result<Notifier> {
        let conn = Connection::session()?;
        let proxy: Proxy<'static> = zbus::blocking::proxy::Builder::new(&conn)
            .destination(DEST)?
            .path(PATH)?
            .interface(DEST)?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()?;
        // Subscribe before the first Notify, so no answer can be missed.
        let signals = proxy.receive_all_signals()?;
        let caps: Vec<String> = proxy.call("GetCapabilities", &())?;
        let routes = Arc::new(Mutex::new(Routes::default()));
        let dispatch = Arc::clone(&routes);
        thread::spawn(move || {
            for msg in signals {
                let header = msg.header();
                let member = header.member().map(|m| m.as_str().to_owned());
                let parsed = match member.as_deref() {
                    Some("ActionInvoked") => msg
                        .body()
                        .deserialize::<(u32, String)>()
                        .ok()
                        .map(|(id, key)| (id, Signal::Action(key))),
                    Some("NotificationClosed") => msg
                        .body()
                        .deserialize::<(u32, u32)>()
                        .ok()
                        .map(|(id, reason)| (id, Signal::Closed(reason))),
                    _ => None,
                };
                if let Some((id, signal)) = parsed {
                    route(&dispatch, id, signal);
                }
            }
        });
        Ok(Notifier {
            proxy,
            caps,
            routes,
        })
    }

    pub fn has(&self, capability: &str) -> bool {
        self.caps.iter().any(|c| c == capability)
    }

    /// Shows (or updates) a notification; its id, or `None` on error (logged).
    pub fn show(&self, n: &Note<'_>) -> Option<u32> {
        let body = if self.has("body-markup") {
            escape_markup(n.body)
        } else {
            n.body.to_owned()
        };
        let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
        hints.insert("urgency", Value::from(n.urgency as u8));
        hints.insert("desktop-entry", Value::from("hoppy"));
        if n.transient {
            hints.insert("transient", Value::from(true));
        }
        if n.sound == Some(false) {
            hints.insert("suppress-sound", Value::from(true));
        }
        if let Some(v) = n.value {
            hints.insert("value", Value::from(i32::from(v.min(100))));
        }
        let actions = if self.has("actions") {
            action_list(n.actions)
        } else {
            Vec::new()
        };
        let reply: zbus::Result<u32> = self.proxy.call(
            "Notify",
            &(
                APP_NAME,
                n.replaces,
                APP_ICON,
                n.summary,
                body.as_str(),
                actions,
                hints,
                n.timeout_ms,
            ),
        );
        reply
            .map_err(|e| diag::log(format!("notify: Notify failed: {e}")))
            .ok()
    }

    pub fn close(&self, id: u32) {
        let _: zbus::Result<()> = self.proxy.call("CloseNotification", &(id,));
    }

    /// Signals for notification `id` (also ones that arrived just before this call).
    pub fn listen(&self, id: u32) -> Receiver<Signal> {
        let (tx, rx) = mpsc::channel();
        let mut routes = lock(&self.routes);
        routes.orphans.retain(|(oid, signal)| {
            if *oid == id {
                let _ = tx.send(signal.clone());
                false
            } else {
                true
            }
        });
        routes.listeners.insert(id, tx);
        rx
    }

    pub fn unlisten(&self, id: u32) {
        lock(&self.routes).listeners.remove(&id);
    }

    /// Receives the signals nobody `listen`s to (the tray's balloons and progress).
    pub fn set_fallback(&self, f: impl Fn(u32, &Signal) + Send + 'static) {
        lock(&self.routes).fallback = Some(Box::new(f));
    }
}

fn route(routes: &Mutex<Routes>, id: u32, signal: Signal) {
    let mut r = lock(routes);
    if let Some(tx) = r.listeners.get(&id) {
        let _ = tx.send(signal);
        return;
    }
    if let Some(f) = &r.fallback {
        f(id, &signal);
    }
    if r.orphans.len() == ORPHANS {
        r.orphans.pop_front();
    }
    r.orphans.push_back((id, signal));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_is_escaped() {
        assert_eq!(
            escape_markup("<b>Tom & Jerry</b>"),
            "&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;"
        );
        assert_eq!(escape_markup("Città 🐰"), "Città 🐰");
    }

    #[test]
    fn actions_are_flattened_pairs() {
        assert_eq!(
            action_list(&[("accept", "Accetta"), ("decline", "Rifiuta")]),
            ["accept", "Accetta", "decline", "Rifiuta"]
        );
    }

    #[test]
    fn early_signals_reach_a_late_listener() {
        let routes = Mutex::new(Routes::default());
        route(&routes, 7, Signal::Action("accept".into()));
        route(&routes, 8, Signal::Closed(2));
        let mut r = lock(&routes);
        assert_eq!(r.orphans.len(), 2);
        r.orphans.retain(|(id, _)| *id != 7);
        assert_eq!(r.orphans.front(), Some(&(8, Signal::Closed(2))));
        drop(r);
        for i in 0..40 {
            route(&routes, 100 + i, Signal::Closed(1));
        }
        assert_eq!(lock(&routes).orphans.len(), ORPHANS);
    }
}
