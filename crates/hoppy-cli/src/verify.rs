//! What the desktop verification dialogs show (spec §7.6–7.7), independent of the toolkit.
//!
//! `gui.rs` describes a dialog with a [`DialogView`] (text, the 4-emoji code with its names,
//! buttons) and hands it to `ui::show_dialog` / `ui::ask_offer`: a custom Win32 window on
//! Windows, an `NSAlert` with an accessory view on macOS, zenity on Linux. People compare the
//! 4 emoji by eye with the other screen; there is no other form of the code. While a dialog
//! is open, workers can close it through a [`DialogLive`] (the transfer ended).
//!
//! Pure Rust, compiled and tested on every platform.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How often an open dialog checks its [`DialogLive`].
pub const LIVE_POLL: Duration = Duration::from_millis(200);

/// A dialog to show. The first button is the default one (Enter/Return) and the answer when
/// the dialog is closed, times out or is closed through [`DialogLive::close`]: it is always
/// the safe choice ("Decline", "Close", "Cancel"), so a stray Enter never accepts files.
#[derive(Debug, Clone, Default)]
pub struct DialogView {
    pub title: String,
    /// Main text (already cleaned with `display_*`; plain text, may have line breaks).
    pub text: String,
    /// Security state and warnings, shown after `text` (may be empty).
    pub note: String,
    pub code: Option<CodeView>,
    /// `(id, label)`; at least one. Ids are non-zero.
    pub buttons: Vec<(u32, String)>,
    /// Linux: label of the notification button that opens the full dialog (the accept
    /// question is a notification first). Unused elsewhere.
    pub details: Option<String>,
    /// No answer within this = the default button.
    pub timeout: Option<Duration>,
}

impl DialogView {
    /// The default (first) button's id; 0 if there are no buttons.
    pub fn default_id(&self) -> u32 {
        self.buttons.first().map_or(0, |b| b.0)
    }

    /// Everything as plain text, for places that can show only text (a notification body,
    /// kdialog, the log): text, the note, the code with the emoji names.
    pub fn plain_text(&self) -> String {
        let mut parts = vec![self.text.clone()];
        if !self.note.is_empty() {
            parts.push(self.note.clone());
        }
        if let Some(code) = &self.code {
            parts.push(code.one_line());
        }
        parts.retain(|p| !p.is_empty());
        parts.join("\n\n")
    }
}

/// The short authentication string as the user compares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeView {
    pub emoji: [&'static str; 4],
    /// Names of the emoji in the UI language (screen readers, and people who cannot tell two
    /// emoji apart: GDI draws them in one colour).
    pub names: [&'static str; 4],
}

impl CodeView {
    pub fn new(sas: &hoppy_core::Sas, italian: bool) -> Self {
        let words = sas.words(italian);
        let names = std::array::from_fn(|i| words.get(i).copied().unwrap_or(""));
        let emoji =
            std::array::from_fn(|i| hoppy_core::SAS_EMOJI[usize::from(sas.emoji[i] & 63)].emoji);
        Self { emoji, names }
    }

    /// "🐘 🐢 🐙 🦋" (spaced so each stands alone).
    pub fn emoji_row(&self) -> String {
        self.emoji.join("  ")
    }

    /// "elefante · tartaruga · polpo · farfalla".
    pub fn names_row(&self) -> String {
        self.names.join(" · ")
    }

    /// "🐘 🐢 🐙 🦋 (elefante, tartaruga, polpo, farfalla)".
    pub fn one_line(&self) -> String {
        format!("{} ({})", self.emoji.join(" "), self.names.join(", "))
    }
}

/// Shared between an open dialog and the code that may change it from another thread.
/// Cheap to clone; every backend polls it every [`LIVE_POLL`].
#[derive(Debug, Clone, Default)]
pub struct DialogLive(Arc<LiveFlags>);

#[derive(Debug, Default)]
struct LiveFlags {
    closed: AtomicBool,
}

impl DialogLive {
    pub fn new() -> Self {
        Self::default()
    }

    /// Close the dialog as if its default button had been chosen.
    pub fn close(&self) {
        self.0.closed.store(true, Ordering::Release);
    }

    pub fn closed(&self) -> bool {
        self.0.closed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_view_shows_the_emoji_with_their_names() {
        let sas = hoppy_core::Sas::from_value(0x05_1234);
        let it = CodeView::new(&sas, true);
        let en = CodeView::new(&sas, false);
        assert_eq!(it.emoji, en.emoji);
        assert_eq!(it.emoji.concat(), sas.text);
        assert_eq!(it.names.to_vec(), sas.words(true));
        assert_eq!(en.names.to_vec(), sas.words(false));
        assert!(it.one_line().starts_with(&it.emoji.join(" ")));
        assert!(it.one_line().ends_with(&format!("{})", it.names[3])));
    }

    #[test]
    fn plain_text_carries_code_and_note() {
        let sas = hoppy_core::Sas::from_value(0);
        let view = DialogView {
            text: "Pixel wants to send you 1 file".into(),
            note: "🔒 Encrypted".into(),
            code: Some(CodeView::new(&sas, false)),
            buttons: vec![(1, "Decline".into()), (2, "Accept".into())],
            ..Default::default()
        };
        assert_eq!(view.default_id(), 1);
        assert_eq!(
            view.plain_text(),
            "Pixel wants to send you 1 file\n\n🔒 Encrypted\n\n🐘 🐘 🐘 🐘 (elephant, elephant, \
             elephant, elephant)"
        );
        assert_eq!(DialogView::default().default_id(), 0);
    }

    #[test]
    fn live_flags_cross_threads() {
        let live = DialogLive::new();
        let other = live.clone();
        assert!(!live.closed());
        std::thread::spawn(move || other.close()).join().unwrap();
        assert!(live.closed());
    }
}
