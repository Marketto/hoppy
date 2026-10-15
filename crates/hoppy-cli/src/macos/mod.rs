//! macOS backend of `hoppyw`: the same safe API as `platform::win`, exported as
//! `platform::ui`, so `gui.rs` runs unchanged on a Mac (macOS 12 Monterey and later, Intel
//! from 2015 and Apple silicon; one universal binary).
//!
//! - Menu-bar icon: `NSStatusItem` with a template image of the bunny; a click pops the
//!   same menu as on Windows (`NATIVE_POPUPS = true`). The app is `LSUIElement` (no Dock
//!   icon). `hoppyw send` shows its own status item with the progress as its title.
//! - Device menu for sending: `NSMenu` at the mouse pointer.
//! - Accept question: `NSAlert`, "Decline" is the default button (Return); never
//!   auto-accepted (no answer within `ui::ASK_TIMEOUT` = declined).
//! - Notifications: `UNUserNotificationCenter` when running from the app bundle (a click
//!   on "Received…" opens the folder); `osascript display notification` when that API is
//!   unavailable (not in a bundle, or refused for the ad-hoc signature). If the user turned
//!   Hoppy's notifications off, nothing is shown.
//! - Finder: the "Send with Hoppy" service (Info.plist `NSServices`, message
//!   [`SERVICE_MESSAGE`]), files dropped on the app icon and "Open With → Hoppy"
//!   (`application:openURLs:`) all start `hoppyw send FILE…`.
//! - "Open at login": `SMAppService.mainApp` (macOS 13+), else a LaunchAgent plist written
//!   by Hoppy (`login.rs`).
//! - Jingle: `NSSound` from the embedded WAV; update check: `/usr/bin/curl`
//!   (`crate::unix::http`); folders and URLs: `/usr/bin/open`.
//!
//! This file is pure Rust (bundle paths, plists, argv builders) and is unit-tested on Linux;
//! the AppKit code (`ui.rs`, `login.rs`) only compiles on macOS (CI: the macos job).
//! Packaging: `apps/macos/` (Info.plist, `build-app.sh`).

#[cfg(target_os = "macos")]
pub mod login;
#[cfg(target_os = "macos")]
pub mod ui;

#[cfg(target_os = "macos")]
pub use login::{first_run as first_run_integration, install, uninstall};
#[cfg(target_os = "macos")]
pub use ui::ui_is_italian;

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// `CFBundleIdentifier` in `apps/macos/Info.plist`.
pub const BUNDLE_ID: &str = "app.hoppy.desktop";
/// `NSMessage` of the Finder service in Info.plist: AppKit calls
/// `sendFiles:userData:error:` on the services provider (`ui.rs`).
pub const SERVICE_MESSAGE: &str = "sendFiles";
/// Label (and file name) of the LaunchAgent used for "Open at login" on macOS 12.
pub const LAUNCH_AGENT_LABEL: &str = "app.hoppy.desktop.login";
/// Every file Hoppy writes outside its own folders carries this; files without it are
/// never overwritten or deleted.
pub const MARKER: &str = "Written by Hoppy (hoppy install); removed by hoppy uninstall.";
/// Longest status-item title of `hoppyw send` (characters): the menu bar is shared.
pub const STATUS_TITLE_MAX: usize = 30;

/// `X.app` when `exe` is `X.app/Contents/MacOS/<name>` (both `hoppyw` and the bundled
/// `hoppy` CLI live there); `None` for a bare binary (`cargo run`, Homebrew-style installs).
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let is_app = bundle
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"));
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && is_app)
        .then(|| bundle.to_path_buf())
}

/// Where the app bundle runs from, for "Open at login".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// A normal folder (Applications, home…).
    Installed,
    /// Gatekeeper App Translocation: a quarantined app opened where it was downloaded runs
    /// from a random read-only copy that disappears, so a login item would break.
    Translocated,
    /// Still on the mounted .dmg (`/Volumes/…`).
    DiskImage,
}

pub fn bundle_location(bundle: &Path) -> Location {
    let translocated = bundle
        .components()
        .any(|c| matches!(c, Component::Normal(n) if n == "AppTranslocation"));
    if translocated {
        Location::Translocated
    } else if bundle.starts_with("/Volumes") {
        Location::DiskImage
    } else {
        Location::Installed
    }
}

/// `~/Library/LaunchAgents/app.hoppy.desktop.login.plist`.
pub fn launch_agent_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LAUNCH_AGENT_LABEL}.plist"))
}

/// Escapes text for an XML element.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// LaunchAgent that opens the bundle at login through Launch Services (`open -a`), so it
/// starts like a double-click: one instance, Finder service registered.
pub fn launch_agent_plist(bundle: &Path) -> String {
    let bundle = xml_escape(&bundle.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- {MARKER} -->
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LAUNCH_AGENT_LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>/usr/bin/open</string>
		<string>-a</string>
		<string>{bundle}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>LimitLoadToSessionType</key>
	<string>Aqua</string>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
"#
    )
}

/// A file Hoppy wrote (carries [`MARKER`]).
pub fn is_ours(text: &str) -> bool {
    text.contains(MARKER)
}

/// `path` is a regular file (not a symlink, which is never followed) carrying [`MARKER`].
pub fn is_our_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
        && std::fs::read_to_string(path).is_ok_and(|t| is_ours(&t))
}

/// Writes a marked file (`contents` must carry [`MARKER`]) without ever writing through
/// something else: refuses a symlink, a directory or a file of the user's at `path`, writes
/// a new temporary file next to it and renames it into place (atomic, replaces a symlink
/// planted meanwhile instead of following it).
pub fn write_marked_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    use std::io::Write;
    debug_assert!(is_ours(contents));
    match std::fs::symlink_metadata(path) {
        Ok(m) if !m.file_type().is_file() => {
            bail!("{} is not a regular file: left unchanged", path.display())
        }
        Ok(_) if !is_our_file(path) => {
            bail!(
                "{} exists and is not Hoppy's: left unchanged",
                path.display()
            )
        }
        _ => {}
    }
    let dir = path.parent().context("no parent folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = path.file_name().context("no file name")?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp); // a leftover (or planted) entry is replaced
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| f.write_all(contents.as_bytes()).and_then(|()| f.sync_all()));
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing {}", path.display()));
    }
    Ok(())
}

/// `osascript` arguments that show a notification. The texts are passed as script
/// arguments after a fixed first argument, never inside the script: the first argument
/// that is not an option ends option parsing, so a device name like "-e …" stays text.
pub fn notification_args(title: &str, text: &str) -> Vec<OsString> {
    [
        "-e",
        "on run argv",
        "-e",
        "display notification (item 3 of argv) with title (item 2 of argv)",
        "-e",
        "end run",
        "hoppy",
        title,
        text,
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

/// UI language from the first entry of the user's preferred languages ("it", "it-IT",
/// "en-GB"…).
pub fn language_is_italian(first: Option<&str>) -> bool {
    first.is_some_and(|l| l == "it" || l.starts_with("it-") || l.starts_with("it_"))
}

/// Title of the `hoppyw send` status item: the tip cut to [`STATUS_TITLE_MAX`] characters
/// (keeping a trailing "NN%" visible).
pub fn status_title(tip: &str) -> String {
    let count = tip.chars().count();
    if count <= STATUS_TITLE_MAX {
        return tip.to_owned();
    }
    // "Invio a Nome lunghissimo… 42%": keep the percentage, shorten the name.
    let pct = tip
        .rsplit_once(' ')
        .map(|(_, last)| last)
        .filter(|last| last.ends_with('%') && last.chars().count() <= 4);
    let keep = |s: &str, n: usize| -> String {
        let mut out: String = s.chars().take(n).collect();
        out.truncate(out.trim_end().len());
        out
    };
    match pct {
        Some(pct) => {
            let room = STATUS_TITLE_MAX - pct.chars().count() - 2;
            format!("{}… {pct}", keep(tip, room))
        }
        None => format!("{}…", keep(tip, STATUS_TITLE_MAX - 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_paths() {
        let exe = Path::new("/Applications/Hoppy.app/Contents/MacOS/hoppyw");
        assert_eq!(
            bundle_of(exe),
            Some(PathBuf::from("/Applications/Hoppy.app"))
        );
        let cli = Path::new("/Users/x/Apps/Hoppy.app/Contents/MacOS/hoppy");
        assert_eq!(
            bundle_of(cli),
            Some(PathBuf::from("/Users/x/Apps/Hoppy.app"))
        );
        assert_eq!(bundle_of(Path::new("/usr/local/bin/hoppyw")), None);
        assert_eq!(bundle_of(Path::new("target/debug/hoppyw")), None);
        assert_eq!(bundle_of(Path::new("/x/Hoppy/Contents/MacOS/hoppyw")), None);
        assert_eq!(bundle_of(Path::new("hoppyw")), None);
    }

    #[test]
    fn bundle_locations() {
        assert_eq!(
            bundle_location(Path::new("/Applications/Hoppy.app")),
            Location::Installed
        );
        assert_eq!(
            bundle_location(Path::new(
                "/private/var/folders/x/T/AppTranslocation/1234-ABCD/d/Hoppy.app"
            )),
            Location::Translocated
        );
        assert_eq!(
            bundle_location(Path::new("/Volumes/Hoppy/Hoppy.app")),
            Location::DiskImage
        );
        assert_eq!(
            bundle_location(Path::new("/Users/x/VolumesHoppy.app")),
            Location::Installed
        );
    }

    #[test]
    fn launch_agent_is_marked_and_escaped() {
        let home = Path::new("/Users/x");
        assert_eq!(
            launch_agent_path(home),
            PathBuf::from("/Users/x/Library/LaunchAgents/app.hoppy.desktop.login.plist")
        );
        let plist = launch_agent_plist(Path::new("/Users/x/A & <B>/Hoppy.app"));
        assert!(is_ours(&plist));
        assert!(plist.contains("<string>/Users/x/A &amp; &lt;B&gt;/Hoppy.app</string>"));
        assert!(plist.contains(&format!("<string>{LAUNCH_AGENT_LABEL}</string>")));
        assert!(!is_ours("<plist/>"));
    }

    #[test]
    fn marked_files_never_write_through_others() {
        let dir = tempfile::tempdir().unwrap();
        let path = launch_agent_path(dir.path());
        let plist = launch_agent_plist(Path::new("/Applications/Hoppy.app"));
        write_marked_file(&path, &plist).unwrap();
        assert!(is_our_file(&path));
        write_marked_file(&path, &plist).unwrap(); // ours: replaced
        let leftovers = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary file left");

        std::fs::write(&path, "<plist>the user's</plist>").unwrap();
        assert!(!is_our_file(&path));
        assert!(write_marked_file(&path, &plist).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "<plist>the user's</plist>"
        );

        // A symlink (even to a file of ours) is never followed or replaced.
        let target = dir.path().join("elsewhere.plist");
        std::fs::write(&target, &plist).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(!is_our_file(&path));
        assert!(write_marked_file(&path, &plist).is_err());
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn notification_texts_stay_arguments() {
        let args = notification_args("-e do shell script \"x\"", "a\" & b");
        let n = args.len();
        // The fixed sentinel comes first, then the texts unchanged; the script never
        // contains them.
        assert_eq!(args[n - 3], "hoppy");
        assert_eq!(args[n - 2], "-e do shell script \"x\"");
        assert_eq!(args[n - 1], "a\" & b");
        let script: Vec<&OsString> = args[..n - 3].iter().collect();
        assert!(
            script
                .iter()
                .all(|a| !a.to_string_lossy().contains("shell"))
        );
        assert_eq!(
            script
                .iter()
                .filter(|a| *a == &&OsString::from("-e"))
                .count(),
            3
        );
    }

    #[test]
    fn italian_from_preferred_languages() {
        assert!(language_is_italian(Some("it")));
        assert!(language_is_italian(Some("it-IT")));
        assert!(language_is_italian(Some("it-CH")));
        assert!(!language_is_italian(Some("en-IT")));
        assert!(!language_is_italian(Some("ita")));
        assert!(!language_is_italian(None));
    }

    #[test]
    fn status_titles_are_short() {
        assert_eq!(status_title("Cerco dispositivi…"), "Cerco dispositivi…");
        let t = status_title("Invio a Pixel di Marco con un nome lungo… 42%");
        assert!(t.chars().count() <= STATUS_TITLE_MAX, "{t}");
        assert!(t.ends_with("… 42%"), "{t}");
        let t = status_title(&"x".repeat(80));
        assert_eq!(t.chars().count(), STATUS_TITLE_MAX);
        assert!(t.ends_with('…'));
    }

    /// Info.plist and the code agree (bundle id, service message, minimum macOS).
    #[test]
    fn info_plist_matches_the_code() {
        let plist = include_str!("../../../../apps/macos/Info.plist");
        let value = |key: &str| -> Option<String> {
            let at = plist.find(&format!("<key>{key}</key>"))?;
            let rest = &plist[at..];
            let start = rest.find("<string>")? + "<string>".len();
            let end = rest.find("</string>")?;
            Some(rest[start..end].to_owned())
        };
        assert_eq!(value("CFBundleIdentifier").as_deref(), Some(BUNDLE_ID));
        assert_eq!(value("NSMessage").as_deref(), Some(SERVICE_MESSAGE));
        assert_eq!(value("CFBundleExecutable").as_deref(), Some("hoppyw"));
        assert_eq!(value("LSMinimumSystemVersion").as_deref(), Some("12.0"));
        assert_eq!(value("CFBundleIconFile").as_deref(), Some("Hoppy"));
        assert!(plist.contains("<key>LSUIElement</key>\n\t<true/>"));
        assert!(plist.contains("@VERSION@"));
        for key in [
            "NSLocalNetworkUsageDescription",
            "NSDownloadsFolderUsageDescription",
            "NSDesktopFolderUsageDescription",
            "NSDocumentsFolderUsageDescription",
        ] {
            assert!(value(key).is_some_and(|v| !v.is_empty()), "{key}");
        }
        // Every localised Info.plist string exists in Info.plist itself.
        let it = include_str!("../../../../apps/macos/it.lproj/InfoPlist.strings");
        for line in it.lines().filter(|l| l.starts_with('"')) {
            let key = line.split('"').nth(1).unwrap_or_default();
            assert!(value(key).is_some(), "{key} is not in Info.plist");
        }
        let services = include_str!("../../../../apps/macos/it.lproj/ServicesMenu.strings");
        let title = value("default").unwrap_or_default();
        assert!(services.contains(&format!("\"{title}\" = \"Invia con Hoppy\";")));
    }
}
