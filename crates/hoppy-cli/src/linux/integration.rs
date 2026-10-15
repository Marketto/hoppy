//! Per-user desktop integration, written by `hoppy install` and by the tray on its first
//! run of each version (unless the user ran `hoppy uninstall`), removed by
//! `hoppy uninstall`:
//!
//! | What | File |
//! | --- | --- |
//! | Nautilus (Ubuntu Files): right-click → Scripts → "Send with Hoppy" | `~/.local/share/nautilus/scripts/<label>` |
//! | Nemo (Linux Mint): right-click → "Send with Hoppy" | `~/.local/share/nemo/actions/hoppy.nemo_action` |
//! | Dolphin (KDE): right-click → "Send with Hoppy" (top level) | `~/.local/share/kio/servicemenus/hoppy.desktop` |
//! | App launcher, "Open with → Hoppy", drop on the dock icon (only without the .deb's) | `~/.local/share/applications/hoppy.desktop` |
//! | Tray at login | `~/.config/autostart/hoppy.desktop` |
//!
//! Autostart is created only by `hoppy install` and the tray's very first run; upgrades
//! refresh it only if it still exists, and never touch one the user disabled
//! (`Hidden=true`, `X-GNOME-Autostart-enabled=false`, e.g. GNOME "Startup Applications").
//!
//! Every file carries `MARKER`; uninstall removes only files that have it, and install
//! never overwrites a file without it (the user's own). Paths go into the files quoted
//! for their format (`exec_arg`, `shell_quote`), never raw.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config;
use crate::update::CURRENT_VERSION;

pub const MARKER: &str = "# Written by Hoppy (hoppy install); removed by hoppy uninstall.";
/// `desktop_integration` after `hoppy uninstall`.
pub const OFF: &str = "off";
/// The Nautilus script's file name is its menu label: one per UI language.
pub const SCRIPT_NAMES: [&str; 2] = ["Invia con Hoppy", "Send with Hoppy"];
/// Installed by the .deb (apps/linux/hoppy.desktop); a per-user copy would shadow it.
pub const SYSTEM_LAUNCHERS: [&str; 2] = [
    "/usr/share/applications/hoppy.desktop",
    "/usr/local/share/applications/hoppy.desktop",
];

/// Base folders (XDG data and config homes).
pub struct Places {
    pub data: PathBuf,
    pub config: PathBuf,
}

impl Places {
    pub fn user() -> Result<Places> {
        Ok(Places {
            data: dirs::data_dir().context("no data folder (XDG_DATA_HOME)")?,
            config: dirs::config_dir().context("no config folder (XDG_CONFIG_HOME)")?,
        })
    }

    fn script(&self, name: &str) -> PathBuf {
        self.data.join("nautilus/scripts").join(name)
    }
    fn nemo(&self) -> PathBuf {
        self.data.join("nemo/actions/hoppy.nemo_action")
    }
    fn dolphin(&self) -> PathBuf {
        self.data.join("kio/servicemenus/hoppy.desktop")
    }
    fn launcher(&self) -> PathBuf {
        self.data.join("applications/hoppy.desktop")
    }
    fn autostart(&self) -> PathBuf {
        self.config.join("autostart/hoppy.desktop")
    }

    /// Every file Hoppy may have written.
    pub fn all(&self) -> Vec<PathBuf> {
        let mut all: Vec<PathBuf> = SCRIPT_NAMES.iter().map(|n| self.script(n)).collect();
        all.extend([
            self.nemo(),
            self.dolphin(),
            self.launcher(),
            self.autostart(),
        ]);
        all
    }
}

#[derive(Debug)]
pub struct File {
    pub path: PathBuf,
    pub content: String,
    pub executable: bool,
}

fn checked(s: &str) -> Result<&str> {
    if s.chars().any(char::is_control) {
        bail!("the program path contains control characters");
    }
    Ok(s)
}

/// One `Exec=` argument (desktop entry spec): quoted when needed (`"`, `` ` ``, `$`, `\`
/// escaped inside), `%` doubled, then the string-level backslash escape.
pub fn exec_arg(arg: &str) -> Result<String> {
    let arg = checked(arg)?;
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+,:@".contains(c));
    let mut out = String::new();
    if plain {
        out.push_str(arg);
    } else {
        out.push('"');
        for c in arg.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
    }
    Ok(out.replace('%', "%%").replace('\\', "\\\\"))
}

/// A desktop-entry string value (`TryExec=`): backslashes escaped.
pub fn string_value(s: &str) -> Result<String> {
    Ok(checked(s)?.replace('\\', "\\\\"))
}

/// POSIX shell single quotes.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The app launcher. `managed`: the per-user copy (with `MARKER`); the .deb ships the
/// unmanaged one with `hoppyw` from PATH (apps/linux/hoppy.desktop, kept equal by a test).
pub fn launcher_entry(hoppyw: &str, managed: bool) -> Result<String> {
    let exec = exec_arg(hoppyw)?;
    let try_exec = string_value(hoppyw)?;
    let marker = if managed {
        format!("{MARKER}\n")
    } else {
        String::new()
    };
    Ok(format!(
        "[Desktop Entry]
{marker}Type=Application
Name=Hoppy
GenericName=File sharing
GenericName[it]=Condivisione file
Comment=Send files to nearby devices
Comment[it]=Invia file ai dispositivi vicini
Exec={exec} open %F
TryExec={try_exec}
Icon=hoppy
Terminal=false
StartupNotify=false
Categories=Network;FileTransfer;
Keywords=send;share;transfer;phone;android;
Keywords[it]=invia;condividi;trasferisci;telefono;android;
MimeType=application/octet-stream;
Actions=send;

[Desktop Action send]
Name=Send files…
Name[it]=Invia file…
Exec={exec} send
"
    ))
}

/// Every file to write for `hoppyw` at `hoppyw`. `script_name`: the Nautilus label;
/// `launcher`: also the per-user app launcher (when the .deb's is missing).
pub fn files(
    places: &Places,
    hoppyw: &Path,
    script_name: &str,
    launcher: bool,
) -> Result<Vec<File>> {
    let raw = hoppyw
        .to_str()
        .context("the program path is not valid UTF-8")?;
    let exec = exec_arg(raw)?;
    let try_exec = string_value(raw)?;
    let mut out = vec![
        File {
            path: places.script(script_name),
            content: format!(
                "#!/bin/sh\n{MARKER}\n# Nautilus runs this with the selected files as arguments.\n\
                 exec {} send \"$@\"\n",
                shell_quote(checked(raw)?)
            ),
            executable: true,
        },
        File {
            path: places.nemo(),
            content: format!(
                "[Nemo Action]
{MARKER}
Name=Send with Hoppy
Name[it]=Invia con Hoppy
Comment=Send the selected files to a nearby device
Comment[it]=Invia i file selezionati a un dispositivo vicino
Exec={exec} send %F
Icon-Name=hoppy
Dependencies={try_exec};
Selection=notnone
Extensions=nodirs;
"
            ),
            executable: false,
        },
        File {
            path: places.dolphin(),
            content: format!(
                "[Desktop Entry]
{MARKER}
Type=Service
MimeType=all/allfiles;
X-KDE-ServiceTypes=KonqPopupMenu/Plugin
X-KDE-Priority=TopLevel
X-KDE-StartupNotify=false
TryExec={try_exec}
Actions=hoppySend;

[Desktop Action hoppySend]
Name=Send with Hoppy
Name[it]=Invia con Hoppy
Icon=hoppy
Exec={exec} send %F
"
            ),
            // Dolphin (KF 5.85+) ignores user service menus that are not executable.
            executable: true,
        },
        File {
            path: places.autostart(),
            content: format!(
                "[Desktop Entry]
{MARKER}
Type=Application
Name=Hoppy
Comment=Receive files from nearby devices
Comment[it]=Ricevi file dai dispositivi vicini
Exec={exec}
TryExec={try_exec}
Icon=hoppy
Terminal=false
X-GNOME-Autostart-enabled=true
X-GNOME-Autostart-Delay=5
X-KDE-autostart-after=panel
"
            ),
            executable: false,
        },
    ];
    if launcher {
        out.push(File {
            path: places.launcher(),
            content: launcher_entry(raw, true)?,
            executable: false,
        });
    }
    Ok(out)
}

/// An autostart entry the user switched off.
pub fn autostart_disabled(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .any(|l| l == "Hidden=true" || l == "X-GNOME-Autostart-enabled=false")
}

fn ours(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|t| t.lines().any(|l| l == MARKER))
}

/// Writes `file` unless a file of the user's (without `MARKER`) is in the way: `Ok(false)`.
fn write_file(file: &File) -> Result<bool> {
    if file.path.symlink_metadata().is_ok() && !ours(&file.path) {
        return Ok(false);
    }
    let dir = file.path.parent().context("no parent folder")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = file.path.file_name().context("no file name")?;
    let tmp = dir.join(format!(".{}.hoppy-tmp", name.to_string_lossy()));
    let _ = fs::remove_file(&tmp);
    let mode = if file.executable { 0o755 } else { 0o644 };
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
    f.write_all(file.content.as_bytes())?;
    f.set_permissions(fs::Permissions::from_mode(mode))?;
    drop(f);
    fs::rename(&tmp, &file.path).with_context(|| format!("writing {}", file.path.display()))?;
    Ok(true)
}

/// Writes the files; returns (written, skipped: the user's own or a disabled autostart).
/// `create_autostart`: false on upgrades (refresh an existing one only).
pub fn install(
    places: &Places,
    hoppyw: &Path,
    script_name: &str,
    launcher: bool,
    create_autostart: bool,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let files = files(places, hoppyw, script_name, launcher)?;
    // The other language's script, and a launcher we no longer need (the .deb has one).
    for stale in SCRIPT_NAMES.iter().filter(|n| **n != script_name) {
        remove_ours(&places.script(stale));
    }
    if !launcher {
        remove_ours(&places.launcher());
    }
    let (mut written, mut skipped) = (Vec::new(), Vec::new());
    for file in &files {
        if file.path == places.autostart() {
            match fs::read_to_string(&file.path) {
                Ok(text) if autostart_disabled(&text) => {
                    skipped.push(file.path.clone());
                    continue;
                }
                Err(_) if !create_autostart => continue,
                _ => {}
            }
        }
        if write_file(file)? {
            written.push(file.path.clone());
        } else {
            skipped.push(file.path.clone());
        }
    }
    Ok((written, skipped))
}

fn remove_ours(path: &Path) -> bool {
    ours(path) && fs::remove_file(path).is_ok()
}

/// Removes every file Hoppy wrote; returns them.
pub fn uninstall(places: &Places) -> Vec<PathBuf> {
    places
        .all()
        .into_iter()
        .filter(|p| remove_ours(p))
        .collect()
}

/// `hoppyw` next to the running program (`hoppy` or `hoppyw` itself).
pub fn hoppyw_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot find the running program")?;
    if exe.file_name().is_some_and(|n| n == "hoppyw") {
        return Ok(exe);
    }
    let gui = exe.with_file_name("hoppyw");
    if !gui.is_file() {
        bail!(
            "{} not found: Hoppy's desktop app (hoppyw) must sit next to hoppy",
            gui.display()
        );
    }
    Ok(gui)
}

fn system_launcher() -> bool {
    SYSTEM_LAUNCHERS.iter().any(|p| Path::new(p).is_file())
}

/// Installs for the current user (the label in the UI language); returns what was written
/// and what was left alone. `create_autostart`: see [`install`].
pub fn install_for_user(create_autostart: bool) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let places = Places::user()?;
    let script = if super::ui_is_italian() {
        SCRIPT_NAMES[0]
    } else {
        SCRIPT_NAMES[1]
    };
    let launcher = !system_launcher();
    let result = install(&places, &hoppyw_path()?, script, launcher, create_autostart)?;
    refresh_mime_cache(&places);
    Ok(result)
}

/// Removes the integration for the current user; returns the removed files.
pub fn uninstall_for_user() -> Result<Vec<PathBuf>> {
    let places = Places::user()?;
    let removed = uninstall(&places);
    refresh_mime_cache(&places);
    Ok(removed)
}

/// Re-reads the per-user launchers' MimeType ("Open with → Hoppy"); best effort.
fn refresh_mime_cache(places: &Places) {
    let apps = places.data.join("applications");
    if let (true, Some(update_db)) = (apps.is_dir(), super::which("update-desktop-database")) {
        let _ = std::process::Command::new(update_db)
            .arg("-q")
            .arg(apps)
            .status();
    }
}

/// Tray start: (re)writes the integration once per version unless the user turned it
/// off with `hoppy uninstall`. Returns the updated settings.
pub fn first_run() -> Result<config::Identity> {
    let identity = config::load_or_create()?;
    if identity.desktop_integration == CURRENT_VERSION || identity.desktop_integration == OFF {
        return Ok(identity);
    }
    // Autostart only on the very first run; after that it is the user's to remove.
    let first = identity.desktop_integration.is_empty();
    let (written, skipped) = install_for_user(first)?;
    crate::diag::log(format!(
        "integration: {} file(s) written, {} left alone (not ours)",
        written.len(),
        skipped.len()
    ));
    config::set_desktop_integration(CURRENT_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(root: &Path) -> Places {
        Places {
            data: root.join("data"),
            config: root.join("config"),
        }
    }

    #[test]
    fn exec_arguments_are_quoted() {
        assert_eq!(exec_arg("/usr/bin/hoppyw").unwrap(), "/usr/bin/hoppyw");
        assert_eq!(
            exec_arg("/home/a b/hoppyw").unwrap(),
            "\"/home/a b/hoppyw\""
        );
        assert_eq!(
            exec_arg(r#"/x/$HOME"`100%\y"#).unwrap(),
            r#""/x/\\$HOME\\"\\`100%%\\\\y""#
        );
        assert!(exec_arg("/x\ny").is_err());
        assert_eq!(shell_quote("/it's/hoppyw"), r"'/it'\''s/hoppyw'");
        assert_eq!(string_value(r"C:\x").unwrap(), r"C:\\x");
    }

    #[test]
    fn deb_launcher_matches_the_generator() {
        let shipped = include_str!("../../../../apps/linux/hoppy.desktop");
        assert_eq!(shipped, launcher_entry("hoppyw", false).unwrap());
    }

    #[test]
    fn autostart_is_created_once_and_respects_the_user() {
        let tmp = tempfile::tempdir().unwrap();
        let p = places(tmp.path());
        let exe = Path::new("/usr/bin/hoppyw");
        let auto = p.config.join("autostart/hoppy.desktop");
        // Upgrade with no autostart: not recreated.
        install(&p, exe, "Send with Hoppy", false, false).unwrap();
        assert!(!auto.exists());
        install(&p, exe, "Send with Hoppy", false, true).unwrap();
        assert!(auto.exists());
        // The user turned it off (GNOME rewrites the file keeping our marker).
        let off = fs::read_to_string(&auto).unwrap().replace(
            "X-GNOME-Autostart-enabled=true",
            "X-GNOME-Autostart-enabled=false",
        );
        fs::write(&auto, &off).unwrap();
        let (_, skipped) = install(&p, exe, "Send with Hoppy", false, true).unwrap();
        assert_eq!(skipped, std::slice::from_ref(&auto));
        assert_eq!(fs::read_to_string(&auto).unwrap(), off);
        assert!(autostart_disabled("[Desktop Entry]\n Hidden=true\n"));
        let nemo = fs::read_to_string(p.data.join("nemo/actions/hoppy.nemo_action")).unwrap();
        assert!(nemo.contains("\nDependencies=/usr/bin/hoppyw;\n"));
    }

    #[test]
    fn install_and_uninstall_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = places(tmp.path());
        let exe = Path::new("/opt/My Apps/hoppyw");
        let (written, skipped) = install(&p, exe, "Invia con Hoppy", true, true).unwrap();
        assert_eq!(written.len(), 5);
        assert!(skipped.is_empty());
        let script = p.data.join("nautilus/scripts/Invia con Hoppy");
        let text = fs::read_to_string(&script).unwrap();
        assert!(text.ends_with("exec '/opt/My Apps/hoppyw' send \"$@\"\n"));
        let mode = |f: &Path| fs::metadata(f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&script), 0o755);
        assert_eq!(mode(&p.data.join("kio/servicemenus/hoppy.desktop")), 0o755);
        assert_eq!(mode(&p.config.join("autostart/hoppy.desktop")), 0o644);
        let autostart = fs::read_to_string(p.config.join("autostart/hoppy.desktop")).unwrap();
        assert!(autostart.contains("\nExec=\"/opt/My Apps/hoppyw\"\n"));
        let dolphin = fs::read_to_string(p.data.join("kio/servicemenus/hoppy.desktop")).unwrap();
        assert!(dolphin.contains("\nExec=\"/opt/My Apps/hoppyw\" send %F\n"));
        assert!(dolphin.contains("X-KDE-Priority=TopLevel"));

        // Switching language replaces the script; the .deb's launcher replaces ours.
        let (written, _) = install(&p, exe, "Send with Hoppy", false, false).unwrap();
        assert_eq!(written.len(), 4);
        assert!(!script.exists());
        assert!(!p.data.join("applications/hoppy.desktop").exists());

        // A file of the user's is never touched.
        let mine = p.data.join("nemo/actions/hoppy.nemo_action");
        fs::write(&mine, "[Nemo Action]\nName=mine\n").unwrap();
        let (_, skipped) = install(&p, exe, "Send with Hoppy", false, false).unwrap();
        assert_eq!(skipped, std::slice::from_ref(&mine));

        let removed = uninstall(&p);
        assert_eq!(removed.len(), 3);
        assert!(mine.exists());
        assert!(p.all().iter().filter(|f| **f != mine).all(|f| !f.exists()));
    }
}
