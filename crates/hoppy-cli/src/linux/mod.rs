//! Linux (freedesktop) backend of `hoppyw`: the same safe API as `platform::win`, exported
//! as `platform::ui`, so `gui.rs` runs unchanged on Ubuntu, Mint, Fedora, KDE…
//!
//! - Tray icon: StatusNotifierItem over D-Bus (`sni.rs`, the `ksni` crate; Ubuntu's
//!   AppIndicator extension and KDE show it). Without a host, `hoppyw` keeps receiving and
//!   uses notifications only.
//! - Balloons, the accept question and send progress: freedesktop notifications
//!   (`notify.rs`, zbus, pure Rust, no libdbus).
//! - Device list, file picker, error boxes and the fallback accept question: `zenity`
//!   (GNOME, preinstalled on Ubuntu) or `kdialog` (KDE) (`dialog.rs`).
//! - Right-click "Send with Hoppy" (Nautilus/Nemo/Dolphin), launcher and autostart:
//!   per-user files written by `integration.rs`.
//! - Done jingle: `pw-play`/`paplay`/`aplay` (`sound.rs`); update check: `curl`/`wget`
//!   (`http.rs`).
//!
//! Compiled on every Unix target except macOS (which has its own backend, `crate::macos`).

pub mod dialog;
pub mod integration;
pub mod notify;
mod sni;
pub mod sound;
pub mod ui;

pub use crate::unix::{http, which};

use anyhow::Result;

/// `hoppy install`: right-click entries (Nautilus, Nemo, Dolphin), launcher and autostart
/// for this user (`integration`).
pub fn install() -> Result<()> {
    let (written, skipped) = integration::install_for_user(true)?;
    crate::config::set_desktop_integration(crate::update::CURRENT_VERSION)?;
    for path in written {
        println!("✓ {}", path.display());
    }
    for path in skipped {
        println!("- {} (yours, left unchanged)", path.display());
    }
    Ok(())
}

/// `hoppy uninstall`: removes them; the tray then never adds them back by itself.
pub fn uninstall() -> Result<()> {
    for path in integration::uninstall_for_user()? {
        println!("✓ removed {}", path.display());
    }
    crate::config::set_desktop_integration(integration::OFF)?;
    Ok(())
}

/// Tray start: the per-user integration, once per version.
pub fn first_run_integration() -> Result<crate::config::Identity> {
    integration::first_run()
}

/// UI language: Italian when the message locale is Italian (gettext order: `LANGUAGE`
/// unless the locale is C/POSIX, then `LC_ALL`, `LC_MESSAGES`, `LANG`).
pub fn ui_is_italian() -> bool {
    locale_is_italian(|k| std::env::var(k).ok())
}

pub fn locale_is_italian(get: impl Fn(&str) -> Option<String>) -> bool {
    let var = |k: &str| get(k).filter(|v| !v.is_empty());
    let messages = var("LC_ALL")
        .or_else(|| var("LC_MESSAGES"))
        .or_else(|| var("LANG"));
    let c_locale = messages
        .as_deref()
        .is_none_or(|m| m == "C" || m == "POSIX" || m.starts_with("C."));
    if !c_locale
        && let Some(first) = var("LANGUAGE")
            .as_deref()
            .and_then(|l| l.split(':').find(|p| !p.is_empty()))
    {
        return first.starts_with("it");
    }
    messages.is_some_and(|m| m.starts_with("it"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn italian_locale_follows_gettext_order() {
        assert!(locale_is_italian(env(&[("LANG", "it_IT.UTF-8")])));
        assert!(!locale_is_italian(env(&[("LANG", "en_US.UTF-8")])));
        assert!(locale_is_italian(env(&[
            ("LANG", "en_US.UTF-8"),
            ("LC_MESSAGES", "it_IT.UTF-8")
        ])));
        assert!(!locale_is_italian(env(&[
            ("LANG", "it_IT.UTF-8"),
            ("LC_ALL", "C.UTF-8")
        ])));
        // LANGUAGE wins, except under the C locale.
        assert!(!locale_is_italian(env(&[
            ("LANG", "it_IT.UTF-8"),
            ("LANGUAGE", "en_GB:it")
        ])));
        assert!(locale_is_italian(env(&[
            ("LANG", "en_US.UTF-8"),
            ("LANGUAGE", ":it:en")
        ])));
        assert!(!locale_is_italian(env(&[("LANGUAGE", "it")])));
        assert!(!locale_is_italian(env(&[])));
    }
}
