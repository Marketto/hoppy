//! Code shared by the two desktop binaries:
//! - `hoppy` (`src/main.rs`): the CLI, also used by scripts and tests;
//! - `hoppyw` (`src/bin/hoppyw.rs`): the desktop tray receiver + "Send with Hoppy" GUI
//!   (Windows, Linux and macOS).
//!
//! Win32 FFI lives only in `platform.rs`; the Linux desktop backend in `linux/`, the macOS
//! one in `macos/` (AppKit FFI only in its `#[cfg(target_os = "macos")]` modules); `gui.rs`
//! is safe, platform-neutral Rust on top of `platform::ui`.

pub mod config;
pub mod diag;
pub mod firewall;
pub mod gui;
// Two attributes on purpose: tools that flip `cfg(not(windows))` to type-check the Windows
// code on Linux drop these modules entirely.
#[cfg(not(windows))]
#[cfg(not(target_os = "macos"))]
pub mod linux;
/// Pure parts are compiled (and tested) on Linux too; the AppKit backend only on macOS.
#[cfg(not(windows))]
pub mod macos;
pub mod platform;
#[cfg(not(windows))]
pub mod unix;
pub mod update;

use std::net::{IpAddr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use hoppy_core::protocol::{DISCOVERY_PORT, TRANSFER_PORT};
use hoppy_core::transfer::serve;
use hoppy_core::{DeviceInfo, Discovery, DiscoveryConfig, ReceiveHandler, accept_callbacks};

/// `--seed` values: `IP` or `IP:PORT` (discovery port by default).
pub fn parse_seeds(raw: &[String]) -> Result<Vec<SocketAddr>> {
    raw.iter()
        .map(|s| {
            s.parse::<SocketAddr>()
                .or_else(|_| {
                    s.parse::<IpAddr>()
                        .map(|ip| SocketAddr::new(ip, DISCOVERY_PORT))
                })
                .with_context(|| format!("invalid --seed {s:?}"))
        })
        .collect()
}

/// `IP` or `IP:PORT` given as a send target (transfer port by default).
pub fn parse_direct(s: &str) -> Option<SocketAddr> {
    s.parse::<SocketAddr>().ok().or_else(|| {
        s.parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, TRANSFER_PORT))
    })
}

/// `transfer_port == 0` → send-only node on an ephemeral UDP port (spec §2 rule 6).
///
/// Senders stay on an ephemeral port even though the callback fallback (spec §6) wants to
/// hear the receiver's multicast announces: sharing 47470 with a receiver on the same PC
/// (the tray) would split unicast between the two processes. While a callback is pending,
/// hoppy-core listens to the group on a second, reuse-bound socket instead.
pub fn start_discovery(
    me: &DeviceInfo,
    transfer_port: u16,
    seeds: &[SocketAddr],
) -> Result<Discovery> {
    start_discovery_on(me, transfer_port, seeds, DISCOVERY_PORT)
}

/// Like [`start_discovery`], with the UDP port of a receiving node chosen (tests/interop).
pub fn start_discovery_on(
    me: &DeviceInfo,
    transfer_port: u16,
    seeds: &[SocketAddr],
    udp_port: u16,
) -> Result<Discovery> {
    let mut cfg = DiscoveryConfig::new(me.clone(), transfer_port);
    cfg.seeds = seeds.to_vec();
    cfg.bind_port = if transfer_port == 0 { 0 } else { udp_port };
    Discovery::start(cfg).context("starting discovery (is the UDP port blocked?)")
}

/// Receives forever: direct connections on `listener` and callbacks (spec §6) through
/// `discovery`, with the same folder and handler, so the handler's one-offer-at-a-time
/// guard covers both paths.
pub fn receive_forever(
    listener: TcpListener,
    discovery: &Discovery,
    dir: PathBuf,
    handler: Arc<dyn ReceiveHandler>,
) -> ! {
    accept_callbacks(discovery, dir.clone(), Arc::clone(&handler));
    serve(listener, dir, handler)
}

/// Where received files go unless `--dir` says otherwise: `Downloads/Hoppy`.
pub fn default_receive_dir() -> Result<PathBuf> {
    Ok(dirs::download_dir()
        .or_else(dirs::home_dir)
        .context("no Downloads folder")?
        .join("Hoppy"))
}

/// TCP listener for receiving; falls back to an ephemeral port if `port` is taken
/// (discovery announces the real one).
pub fn bind_receiver(port: u16) -> Result<TcpListener> {
    TcpListener::bind(("0.0.0.0", port))
        .or_else(|_| TcpListener::bind(("0.0.0.0", 0)))
        .context("cannot open a TCP port for receiving")
}

pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// Holds an "a dialog is open" flag; `try_take` fails if it is already held.
pub struct DialogGuard<'a>(&'a AtomicBool);

impl<'a> DialogGuard<'a> {
    pub fn try_take(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| DialogGuard(flag))
    }
}

impl Drop for DialogGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Display caps (characters) for text that comes from the network.
pub const SHOW_NAME_MAX: usize = 48;
pub const SHOW_PLATFORM_MAX: usize = 16;
pub const SHOW_FILE_MAX: usize = 80;
pub const SHOW_TEXT_MAX: usize = 200;
/// Shown instead of a file name that the §4 sanitizer rejects.
pub const INVALID_NAME: &str = "(?)";

/// Characters that must never reach a dialog: controls (Cc), line/paragraph separators,
/// and format characters (Cf) — bidi overrides/isolates/marks can make "gpj.exe" look
/// like "exe.jpg". ZWJ/ZWNJ (U+200C/D) are kept: they only join glyphs (emoji, Indic,
/// Persian) and cannot reorder text.
fn is_hidden_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061C}'
                | '\u{06DD}'
                | '\u{070F}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08E2}'
                | '\u{180E}'
                | '\u{200B}'
                | '\u{200E}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206F}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{110BD}'
                | '\u{110CD}'
                | '\u{13430}'..='\u{1343F}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0001}'
                | '\u{E0020}'..='\u{E007F}'
        )
}

/// Network text made safe to show: hidden characters removed, trimmed, and cut to
/// `max` characters (ending in "…" when cut). May be empty.
pub fn display_text(raw: &str, max: usize) -> String {
    let clean: String = raw.chars().filter(|&c| !is_hidden_char(c)).collect();
    let clean = clean.trim();
    if clean.chars().count() <= max {
        return clean.to_owned();
    }
    let mut out: String = clean.chars().take(max.saturating_sub(1)).collect();
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

/// A peer's device name for dialogs, balloons and menus ("?" if nothing is left).
pub fn display_name(raw: &str) -> String {
    let s = display_text(raw, SHOW_NAME_MAX);
    if s.is_empty() { "?".to_owned() } else { s }
}

/// A peer's platform string ("android", …) for dialogs.
pub fn display_platform(raw: &str) -> String {
    let s = display_text(raw, SHOW_PLATFORM_MAX);
    if s.is_empty() { "?".to_owned() } else { s }
}

/// An offered file name as it will be saved (§4 sanitizer), then made safe to show.
pub fn display_file_name(raw: &str) -> String {
    let s = hoppy_core::sanitize::sanitize_file_name(raw)
        .map(|n| display_text(&n, SHOW_FILE_MAX))
        .unwrap_or_default();
    if s.is_empty() {
        INVALID_NAME.to_owned()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_dialog_at_a_time() {
        let flag = AtomicBool::new(false);
        let first = DialogGuard::try_take(&flag);
        assert!(first.is_some());
        assert!(DialogGuard::try_take(&flag).is_none());
        drop(first);
        assert!(DialogGuard::try_take(&flag).is_some());
    }

    #[test]
    fn display_text_strips_hidden_and_bidi_chars() {
        // RLO trick: "photo\u{202E}gpj.exe" would render as "photoexe.jpg".
        assert_eq!(display_text("photo\u{202E}gpj.exe", 80), "photogpj.exe");
        let hidden = "a\u{0}\u{7}\u{1b}\u{85}\u{200E}\u{200F}\u{202A}\u{202B}\u{202C}\u{202D}\
                      \u{2066}\u{2067}\u{2068}\u{2069}\u{061C}\u{200B}\u{FEFF}\u{2028}\u{AD}\
                      \u{E0041}b\nc\td";
        assert_eq!(display_text(hidden, 80), "abcd");
        // Joiners stay (emoji ZWJ sequences), as do ordinary non-ASCII names.
        assert_eq!(display_text("👩\u{200D}💻 Città", 80), "👩\u{200D}💻 Città");
        assert_eq!(display_text("  \u{202E} ", 80), "");
    }

    #[test]
    fn display_text_caps_length() {
        assert_eq!(display_text("abcdef", 6), "abcdef");
        assert_eq!(display_text("abcdefg", 6), "abcde…");
        assert_eq!(display_text("abcd efg", 6), "abcd…");
        assert_eq!(display_text(&"🐰".repeat(100), 10).chars().count(), 10);
        assert_eq!(
            display_name(&"x".repeat(500)).chars().count(),
            SHOW_NAME_MAX
        );
        assert_eq!(display_platform("android\u{202E}xx"), "androidxx");
        assert_eq!(display_name("\u{2066}\u{2069}"), "?");
    }

    #[test]
    fn display_file_names_follow_the_sanitizer() {
        assert_eq!(display_file_name("../../etc/passwd"), "passwd");
        assert_eq!(display_file_name(r"C:\x\a:b.txt"), "a_b.txt");
        assert_eq!(display_file_name("CON.txt"), "_CON.txt");
        assert_eq!(display_file_name("evil\u{202E}fdp.exe"), "evilfdp.exe");
        assert_eq!(display_file_name(".."), INVALID_NAME);
        assert_eq!(display_file_name("\u{202E}"), INVALID_NAME);
        assert_eq!(
            display_file_name(&"n".repeat(300)).chars().count(),
            SHOW_FILE_MAX
        );
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn direct_targets() {
        assert_eq!(
            parse_direct("192.168.1.5"),
            Some("192.168.1.5:47471".parse().unwrap())
        );
        assert_eq!(
            parse_direct("10.0.0.2:5000"),
            Some("10.0.0.2:5000".parse().unwrap())
        );
        assert_eq!(parse_direct("Pixel"), None);
    }

    #[test]
    fn seeds_default_to_discovery_port() {
        let seeds = parse_seeds(&["10.0.0.3".into(), "10.0.0.4:9".into()]).unwrap();
        assert_eq!(seeds[0].port(), DISCOVERY_PORT);
        assert_eq!(seeds[1].port(), 9);
        assert!(parse_seeds(&["nope".into()]).is_err());
    }
}
