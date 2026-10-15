//! Installs an update on macOS and Linux after `update::download_asset_fresh` fetched it
//! and checked its SHA-256 against the same release's `SHA256SUMS.txt`; the tray then exits
//! and a small helper starts the new version once this process is gone ([`relaunch_args`]).
//! Windows runs the Inno Setup installer instead (`gui.rs`).
//!
//! - **macOS** ([`install_mac`]): only for `/Applications/Hoppy.app` or
//!   `~/Applications/Hoppy.app` in a folder this user can write ([`mac_target`]); else the
//!   tray links to the portal. `hdiutil attach -nobrowse -readonly` on a private mount point,
//!   `ditto` the app into a hidden folder next to the installed one (same volume),
//!   `hdiutil detach`, then check the copy (`codesign --verify --deep --strict`, ad-hoc
//!   signatures pass; bundle id and version through `PlistBuddy`), swap it in with two
//!   renames (old → `.hoppy-old-…`, new → `Hoppy.app`, undone if the second fails) and
//!   remove the old one. Downloaded by curl, so no quarantine attribute: no Gatekeeper
//!   prompt for the update.
//! - **Linux** ([`install_deb`]): only when `hoppyw` runs as `/usr/bin/hoppyw` from the
//!   `hoppy` package ([`deb_installed`]). `pkexec apt-get install -y FILE.deb` (the user's
//!   password through the desktop's polkit agent; `dpkg -i` without apt-get). No pkexec, or
//!   no polkit agent: the .deb is opened with the system's software installer
//!   (`xdg-open`); dismissing the password prompt cancels.
//!
//! Every program runs through [`Runner`] with an argv; values are never put into shell text
//! (the relaunch helper gets them as positional parameters), so the logic is unit-tested on
//! Linux with a fake runner.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::macos::{BUNDLE_ID, Location, bundle_location, bundle_of};
use crate::update;

pub const HDIUTIL: &str = "/usr/bin/hdiutil";
pub const DITTO: &str = "/usr/bin/ditto";
pub const CODESIGN: &str = "/usr/bin/codesign";
pub const PLIST_BUDDY: &str = "/usr/libexec/PlistBuddy";
pub const OPEN: &str = "/usr/bin/open";
/// The bundle name inside the .dmg and the only one the updater replaces.
pub const APP_NAME: &str = "Hoppy.app";
/// Where the .deb puts the desktop binary.
pub const DEB_HOPPYW: &str = "/usr/bin/hoppyw";
/// dpkg's file list of the installed `hoppy` package.
pub const DPKG_LIST: &str = "/var/lib/dpkg/info/hoppy.list";
/// Hidden folders next to the app: the staged copy and the replaced one.
pub const STAGE_PREFIX: &str = ".hoppy-update-";
pub const OLD_PREFIX: &str = ".hoppy-old-";
/// Leftovers of an interrupted update older than this are removed at tray start.
pub const LEFTOVER_AGE: Duration = Duration::from_secs(3600);
/// pkexec: the authentication dialog was dismissed.
pub const PKEXEC_DISMISSED: i32 = 126;
/// pkexec: not authorized, or no polkit authentication agent is running.
pub const PKEXEC_NOT_AUTHORIZED: i32 = 127;

/// What a program printed and how it ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    /// `None` = killed by a signal.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Runs external programs (fake in tests).
pub trait Runner {
    fn run(&self, program: &OsStr, args: &[OsString]) -> std::io::Result<Output>;
}

/// The real thing: no stdin, output captured.
pub struct System;

impl Runner for System {
    fn run(&self, program: &OsStr, args: &[OsString]) -> std::io::Result<Output> {
        let out = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()?;
        Ok(Output {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

fn os(args: &[&dyn AsRef<OsStr>]) -> Vec<OsString> {
    args.iter().map(|a| a.as_ref().to_owned()).collect()
}

/// Runs and fails unless the exit code is 0 (the end of stderr goes into the error).
fn must(runner: &dyn Runner, program: &str, args: Vec<OsString>) -> Result<Output> {
    let out = runner
        .run(program.as_ref(), &args)
        .with_context(|| format!("starting {program}"))?;
    if !out.ok() {
        bail!(
            "{program} failed ({}): {}",
            out.code
                .map_or("signal".to_owned(), |c| format!("exit {c}")),
            tail(&out.stderr)
        );
    }
    Ok(out)
}

/// Last line of a program's error output, short.
fn tail(text: &str) -> String {
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    line.trim().chars().take(200).collect()
}

/// A hidden sibling folder name that nobody else uses.
fn unique(prefix: &str) -> String {
    format!("{prefix}{}", uuid::Uuid::new_v4().simple())
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(dir) // fails if it exists
}

// ---------------------------------------------------------------- macOS

/// The installed `Hoppy.app` an update may replace: the bundle `exe` runs from, when it is
/// `/Applications/Hoppy.app` or `~/Applications/Hoppy.app` (not translocated, not on the
/// .dmg). Writability is checked separately ([`dir_writable`]).
pub fn mac_target(exe: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let bundle = bundle_of(exe)?;
    if bundle.file_name()? != APP_NAME || bundle_location(&bundle) != Location::Installed {
        return None;
    }
    let parent = bundle.parent()?;
    let allowed = parent == Path::new("/Applications")
        || home.is_some_and(|h| parent == h.join("Applications"));
    allowed.then_some(bundle)
}

/// This user can create (and so rename) entries in `dir`: probed by creating and removing
/// a hidden folder, which also covers ACLs and read-only volumes.
pub fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(unique(STAGE_PREFIX));
    let ok = create_private_dir(&probe).is_ok();
    if ok {
        let _ = fs::remove_dir(&probe);
    }
    ok
}

/// Removes `.hoppy-update-*` / `.hoppy-old-*` folders in `dir` (an update interrupted by a
/// crash or power cut) older than `max_age`. Symlinks and files are left alone.
pub fn remove_leftovers(dir: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let ours = name
            .to_str()
            .is_some_and(|n| n.starts_with(STAGE_PREFIX) || n.starts_with(OLD_PREFIX));
        let Ok(meta) = entry.metadata() else { continue };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if ours && meta.is_dir() && old {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// `CFBundleShortVersionString` the .dmg of `version` must carry (build-app.sh drops a
/// pre-release suffix).
fn bundle_version(version: &str) -> &str {
    version.split('-').next().unwrap_or(version)
}

fn plist_value(runner: &dyn Runner, plist: &Path, key: &str) -> Result<String> {
    let command = format!("Print :{key}");
    let out = must(runner, PLIST_BUDDY, os(&[&"-c", &command, &plist]))?;
    Ok(out.stdout.trim().to_owned())
}

/// Replaces `app` with the `Hoppy.app` inside `dmg` (see the module docs). `work` is the
/// private folder the .dmg was downloaded to (the mount point goes there). On error the
/// installed app is unchanged.
pub fn install_mac(
    runner: &dyn Runner,
    dmg: &Path,
    work: &Path,
    app: &Path,
    version: &str,
) -> Result<()> {
    let parent = app.parent().context("the app has no parent folder")?;
    let mount = work.join("mnt");
    create_private_dir(&mount).context("creating the mount point")?;
    must(
        runner,
        HDIUTIL,
        os(&[
            &"attach",
            &"-nobrowse",
            &"-readonly",
            &"-noautoopen",
            &"-mountpoint",
            &mount,
            &dmg,
        ]),
    )?;
    let stage = parent.join(unique(STAGE_PREFIX));
    let copied = (|| {
        let source = mount.join(APP_NAME);
        let meta = fs::symlink_metadata(&source).context("no Hoppy.app in the disk image")?;
        if !meta.is_dir() {
            bail!("Hoppy.app in the disk image is not a folder");
        }
        create_private_dir(&stage)
            .with_context(|| format!("cannot write to {}", parent.display()))?;
        let staged = stage.join(APP_NAME);
        must(runner, DITTO, os(&[&source, &staged]))?;
        Ok(staged)
    })();
    // Always detach (forcing it if a Finder window or Spotlight still holds it).
    let detach = |force: bool| {
        let mut args = os(&[&"detach"]);
        if force {
            args.push("-force".into());
        }
        args.push(mount.clone().into_os_string());
        runner.run(HDIUTIL.as_ref(), &args).is_ok_and(|o| o.ok())
    };
    if !detach(false) {
        detach(true);
    }
    let _ = fs::remove_dir(&mount);
    let result = copied.and_then(|staged| {
        check_staged_app(runner, &staged, version)?;
        swap_in(&staged, app)
    });
    let _ = fs::remove_dir_all(&stage);
    result
}

/// The copy is a whole, signed Hoppy of the expected version.
fn check_staged_app(runner: &dyn Runner, staged: &Path, version: &str) -> Result<()> {
    must(
        runner,
        CODESIGN,
        os(&[&"--verify", &"--deep", &"--strict", &staged]),
    )
    .context("the new app's signature does not verify")?;
    let plist = staged.join("Contents/Info.plist");
    let id = plist_value(runner, &plist, "CFBundleIdentifier")?;
    if id != BUNDLE_ID {
        bail!("the disk image holds another app ({id})");
    }
    let found = plist_value(runner, &plist, "CFBundleShortVersionString")?;
    if found != bundle_version(version) {
        bail!("the disk image holds version {found}, not {version}");
    }
    let exe = staged.join("Contents/MacOS/hoppyw");
    if !fs::symlink_metadata(&exe).is_ok_and(|m| m.is_file()) {
        bail!("the new app has no hoppyw executable");
    }
    Ok(())
}

/// old → `.hoppy-old-…` (same folder, so both renames are atomic), new → `app`; the old
/// one is put back if the second rename fails, else removed (this process keeps running
/// from the open files until it exits).
fn swap_in(staged: &Path, app: &Path) -> Result<()> {
    let parent = app.parent().context("the app has no parent folder")?;
    let old = parent.join(unique(OLD_PREFIX));
    fs::rename(app, &old).with_context(|| format!("moving {} aside", app.display()))?;
    if let Err(e) = fs::rename(staged, app) {
        let _ = fs::rename(&old, app);
        return Err(e).with_context(|| format!("putting the new app at {}", app.display()));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

// ---------------------------------------------------------------- Linux

/// `hoppyw` is the one the `hoppy` package installed (a copy elsewhere, e.g. built from
/// source, only gets the portal link).
pub fn deb_installed(exe: &Path, dpkg_list: &Path) -> bool {
    exe == Path::new(DEB_HOPPYW) && dpkg_list.is_file()
}

/// How a .deb update ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebOutcome {
    /// Installed: restart the tray.
    Installed,
    /// The user dismissed the password prompt.
    Cancelled,
    /// No pkexec / no polkit agent: the package was opened with the software installer.
    OpenedInstaller,
}

/// `pkexec <apt-get|dpkg> …` for `deb` (absolute path, so never taken for an option).
pub fn deb_install_args(apt_get: Option<&Path>, dpkg: &Path, deb: &Path) -> Vec<OsString> {
    match apt_get {
        Some(apt) => os(&[&apt, &"install", &"-y", &deb]),
        None => os(&[&dpkg, &"-i", &deb]),
    }
}

/// Installs `deb` as root through pkexec (see the module docs). `which` finds programs on
/// `PATH` (fake in tests).
pub fn install_deb(
    runner: &dyn Runner,
    deb: &Path,
    which: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<DebOutcome> {
    if !deb.is_absolute() {
        bail!("the package path must be absolute");
    }
    let open_installer = || -> Result<DebOutcome> {
        let out = runner.run("xdg-open".as_ref(), &os(&[&deb]))?;
        if !out.ok() {
            bail!("no software installer could open the package");
        }
        Ok(DebOutcome::OpenedInstaller)
    };
    let Some(pkexec) = which("pkexec") else {
        return open_installer();
    };
    let apt = which("apt-get");
    let dpkg = which("dpkg").unwrap_or_else(|| PathBuf::from("/usr/bin/dpkg"));
    let args = deb_install_args(apt.as_deref(), &dpkg, deb);
    let out = runner
        .run(pkexec.as_os_str(), &args)
        .context("starting pkexec")?;
    match out.code {
        Some(0) => Ok(DebOutcome::Installed),
        Some(PKEXEC_DISMISSED) => Ok(DebOutcome::Cancelled),
        Some(PKEXEC_NOT_AUTHORIZED) => open_installer(),
        code => bail!(
            "the package could not be installed ({}): {}",
            code.map_or("signal".to_owned(), |c| format!("exit {c}")),
            tail(&out.stderr)
        ),
    }
}

// ---------------------------------------------------------------- the tray's entry points

/// What this copy can replace with a newer version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    MacApp(PathBuf),
    Deb,
}

/// `None` = only the portal link (see the module docs). Touches the disk (probe, dpkg
/// list): call it before the tray's event loop or from a worker.
pub fn target() -> Option<Target> {
    let exe = std::env::current_exe().ok()?;
    if cfg!(target_os = "macos") {
        let app = mac_target(&exe, dirs::home_dir().as_deref())?;
        dir_writable(app.parent()?).then_some(Target::MacApp(app))
    } else {
        deb_installed(&exe, Path::new(DPKG_LIST)).then_some(Target::Deb)
    }
}

/// Worker thread: downloads this platform's package of `release` into a fresh private
/// folder under `temp` (SHA-256 checked), calls `installing` once it starts installing,
/// installs it. The folder is removed afterwards, except when the system installer still
/// has to read the package.
pub fn fetch_and_install(
    http: &dyn update::Http,
    release: &update::Release,
    temp: &Path,
    progress: &mut dyn FnMut(u64),
    installing: &mut dyn FnMut(),
) -> Result<update::UpdateReady> {
    let target = target().context("this copy of Hoppy cannot update itself")?;
    let asset = match target {
        Target::MacApp(_) => update::MACOS_ASSET,
        Target::Deb => update::DEB_ASSET,
    };
    let package = update::download_asset_fresh(http, release, asset, temp, progress)?;
    let work = package
        .parent()
        .context("no download folder")?
        .to_path_buf();
    installing();
    let result = match target {
        Target::MacApp(app) => {
            install_mac(&System, &package, &work, &app, &release.version.to_string())
                .map(|()| update::UpdateReady::Restart)
        }
        Target::Deb => install_deb(&System, &package, &|p| super::which(p)).map(|o| match o {
            DebOutcome::Installed => update::UpdateReady::Restart,
            DebOutcome::Cancelled => update::UpdateReady::Cancelled,
            DebOutcome::OpenedInstaller => update::UpdateReady::OpenedInstaller,
        }),
    };
    if !matches!(result, Ok(update::UpdateReady::OpenedInstaller)) {
        let _ = fs::remove_dir_all(&work);
    }
    result
}

/// Starts the updated app once this process has exited: `open -n Hoppy.app` on macOS, the
/// package's `/usr/bin/hoppyw` on Linux.
pub fn relaunch_after_exit() -> Result<()> {
    let command = match target().context("no updated copy to start")? {
        Target::MacApp(app) => os(&[&OPEN, &"-n", &app]),
        Target::Deb => os(&[&DEB_HOPPYW]),
    };
    spawn_relaunch(&command)
}

/// Tray start: removes what an interrupted macOS update left next to the app.
pub fn remove_app_leftovers() {
    if let Some(Target::MacApp(app)) = target()
        && let Some(dir) = app.parent()
    {
        remove_leftovers(dir, LEFTOVER_AGE);
    }
}

// ---------------------------------------------------------------- restart

/// Script of the relaunch helper: waits (at most 20 s) until process `$1` is gone, then
/// runs the rest of its arguments. Values arrive as positional parameters only.
pub const RELAUNCH_SCRIPT: &str = "i=0; while kill -0 \"$1\" 2>/dev/null && [ \"$i\" -lt 200 ]; \
                                   do sleep 0.1; i=$((i+1)); done; shift; exec \"$@\"";

/// `/bin/sh` arguments that start `command` once process `pid` has exited (the new tray
/// would otherwise find the old one still running and quit).
pub fn relaunch_args(pid: u32, command: &[OsString]) -> Vec<OsString> {
    let mut args = os(&[&"-c", &RELAUNCH_SCRIPT, &"hoppy-relaunch", &pid.to_string()]);
    args.extend(command.iter().cloned());
    args
}

/// Starts the helper (detached; it outlives this process).
pub fn spawn_relaunch(command: &[OsString]) -> Result<()> {
    Command::new("/bin/sh")
        .args(relaunch_args(std::process::id(), command))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("starting the relaunch helper")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    type Answer = Box<dyn Fn(&str, &[String]) -> Output>;

    /// Records every call; answers through a closure, which can have side effects (e.g.
    /// `ditto` creating the copy).
    struct Fake {
        calls: RefCell<Vec<Vec<String>>>,
        answer: Answer,
    }

    impl Fake {
        fn new(answer: impl Fn(&str, &[String]) -> Output + 'static) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                answer: Box::new(answer),
            }
        }

        fn programs(&self) -> Vec<String> {
            self.calls
                .borrow()
                .iter()
                .map(|c| format!("{} {}", c[0], c.get(1).cloned().unwrap_or_default()))
                .collect()
        }
    }

    impl Runner for Fake {
        fn run(&self, program: &OsStr, args: &[OsString]) -> std::io::Result<Output> {
            let program = program.to_string_lossy().into_owned();
            let args: Vec<String> = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let mut call = vec![program.clone()];
            call.extend(args.iter().cloned());
            self.calls.borrow_mut().push(call);
            Ok((self.answer)(&program, &args))
        }
    }

    fn ok(stdout: &str) -> Output {
        Output {
            code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn fail(code: i32, stderr: &str) -> Output {
        Output {
            code: Some(code),
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn mac_target_only_replaces_applications() {
        let home = Path::new("/Users/m");
        let exe = |b: &str| PathBuf::from(format!("{b}/Contents/MacOS/hoppyw"));
        assert_eq!(
            mac_target(&exe("/Applications/Hoppy.app"), Some(home)),
            Some(PathBuf::from("/Applications/Hoppy.app"))
        );
        assert_eq!(
            mac_target(&exe("/Users/m/Applications/Hoppy.app"), Some(home)),
            Some(PathBuf::from("/Users/m/Applications/Hoppy.app"))
        );
        for other in [
            "/Users/m/Downloads/Hoppy.app",
            "/Applications/Hoppy 2.app",
            "/Volumes/Hoppy/Hoppy.app",
            "/private/var/folders/x/AppTranslocation/y/d/Hoppy.app",
        ] {
            assert_eq!(mac_target(&exe(other), Some(home)), None, "{other}");
        }
        assert_eq!(
            mac_target(Path::new("/usr/local/bin/hoppyw"), Some(home)),
            None
        );
    }

    /// A fake installed app and a fake "mounted" disk image: `hdiutil attach` fills the
    /// mount point, `ditto` copies, PlistBuddy answers from a map.
    fn mac_fixture(
        version_in_dmg: &'static str,
        codesign_ok: bool,
    ) -> (tempfile::TempDir, PathBuf, PathBuf, Fake) {
        let root = tempfile::tempdir().unwrap();
        let apps = root.path().join("Applications");
        let app = apps.join(APP_NAME);
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(app.join("Contents/MacOS/hoppyw"), b"old").unwrap();
        let work = root.path().join("work");
        create_private_dir(&work).unwrap();
        let fake = Fake::new(move |program, args| match program {
            HDIUTIL if args[0] == "attach" => {
                let mnt = PathBuf::from(&args[args.len() - 2]);
                let new = mnt.join(APP_NAME).join("Contents/MacOS");
                fs::create_dir_all(&new).unwrap();
                fs::write(new.join("hoppyw"), b"new").unwrap();
                ok("")
            }
            HDIUTIL => {
                let mnt = PathBuf::from(args.last().unwrap());
                fs::remove_dir_all(mnt.join(APP_NAME)).unwrap();
                ok("")
            }
            DITTO => {
                let (src, dst) = (PathBuf::from(&args[0]), PathBuf::from(&args[1]));
                fs::create_dir_all(dst.join("Contents/MacOS")).unwrap();
                fs::copy(
                    src.join("Contents/MacOS/hoppyw"),
                    dst.join("Contents/MacOS/hoppyw"),
                )
                .unwrap();
                ok("")
            }
            CODESIGN if codesign_ok => ok(""),
            CODESIGN => fail(1, "code object is not signed at all"),
            PLIST_BUDDY if args[1].ends_with("CFBundleIdentifier") => ok("app.hoppy.desktop\n"),
            PLIST_BUDDY => ok(&format!("{version_in_dmg}\n")),
            _ => fail(99, "unexpected"),
        });
        (root, app, work, fake)
    }

    #[test]
    fn mac_update_swaps_the_app_and_cleans_up() {
        let (_root, app, work, fake) = mac_fixture("0.3.0", true);
        install_mac(&fake, &work.join("x.dmg"), &work, &app, "0.3.0").unwrap();
        assert_eq!(fs::read(app.join("Contents/MacOS/hoppyw")).unwrap(), b"new");
        let siblings: Vec<_> = fs::read_dir(app.parent().unwrap()).unwrap().collect();
        assert_eq!(siblings.len(), 1, "staging and old copies are gone");
        assert!(!work.join("mnt").exists());
        let ditto = format!("/usr/bin/ditto {}", work.join("mnt/Hoppy.app").display());
        assert_eq!(
            fake.programs(),
            [
                "/usr/bin/hdiutil attach",
                ditto.as_str(),
                "/usr/bin/hdiutil detach",
                "/usr/bin/codesign --verify",
                "/usr/libexec/PlistBuddy -c",
                "/usr/libexec/PlistBuddy -c",
            ]
        );
        let attach = &fake.calls.borrow()[0];
        assert!(
            attach.contains(&"-nobrowse".to_owned()) && attach.contains(&"-readonly".to_owned())
        );
    }

    #[test]
    fn mac_update_refuses_bad_copies_and_keeps_the_old_app() {
        for (version, signed, why) in [
            ("0.2.9", true, "version 0.2.9"),
            ("0.3.0", false, "signature"),
        ] {
            let (_root, app, work, fake) = mac_fixture(version, signed);
            let err =
                install_mac(&fake, &work.join("x.dmg"), &work, &app, "0.3.0-rc1").unwrap_err();
            assert!(format!("{err:#}").contains(why), "{err:#}");
            assert_eq!(fs::read(app.join("Contents/MacOS/hoppyw")).unwrap(), b"old");
            let siblings: Vec<_> = fs::read_dir(app.parent().unwrap()).unwrap().collect();
            assert_eq!(siblings.len(), 1, "no staging folder left behind");
            // Detached even though the install failed.
            assert!(
                fake.programs()
                    .iter()
                    .any(|p| p == "/usr/bin/hdiutil detach")
            );
        }
    }

    #[test]
    fn leftovers_are_removed_only_when_old_and_ours() {
        let dir = tempfile::tempdir().unwrap();
        for name in [".hoppy-update-a", ".hoppy-old-b", "Hoppy.app"] {
            fs::create_dir(dir.path().join(name)).unwrap();
        }
        fs::write(dir.path().join(".hoppy-old-file"), b"x").unwrap();
        remove_leftovers(dir.path(), LEFTOVER_AGE);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 4);
        std::thread::sleep(Duration::from_millis(20));
        remove_leftovers(dir.path(), Duration::from_millis(1));
        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, [".hoppy-old-file", "Hoppy.app"]);
        assert!(dir_writable(dir.path()));
        assert!(!dir_writable(&dir.path().join("missing")));
    }

    #[test]
    fn deb_install_uses_pkexec_and_maps_its_exit_codes() {
        let deb = Path::new("/tmp/Hoppy-update-x/Hoppy-linux-amd64.deb");
        let all = |p: &str| Some(PathBuf::from(format!("/usr/bin/{p}")));
        for (code, want) in [
            (0, Ok(DebOutcome::Installed)),
            (PKEXEC_DISMISSED, Ok(DebOutcome::Cancelled)),
            (PKEXEC_NOT_AUTHORIZED, Ok(DebOutcome::OpenedInstaller)),
            (100, Err("exit 100")),
        ] {
            let fake = Fake::new(move |program, _| match program {
                "/usr/bin/pkexec" if code == 0 => ok(""),
                "/usr/bin/pkexec" => fail(code, "E: Unable to locate package"),
                _ => ok(""),
            });
            let got = install_deb(&fake, deb, &all);
            match want {
                Ok(outcome) => assert_eq!(got.unwrap(), outcome),
                Err(text) => assert!(format!("{:#}", got.unwrap_err()).contains(text)),
            }
            assert_eq!(
                fake.calls.borrow()[0],
                [
                    "/usr/bin/pkexec",
                    "/usr/bin/apt-get",
                    "install",
                    "-y",
                    deb.to_str().unwrap()
                ]
            );
        }
        // No apt-get: dpkg -i; no pkexec: straight to the software installer.
        let no_apt = |p: &str| (p != "apt-get").then(|| PathBuf::from(format!("/usr/bin/{p}")));
        let fake = Fake::new(|_, _| ok(""));
        install_deb(&fake, deb, &no_apt).unwrap();
        assert_eq!(fake.calls.borrow()[0][1..3], ["/usr/bin/dpkg", "-i"]);
        let none = |_: &str| None;
        let fake = Fake::new(|_, _| ok(""));
        assert_eq!(
            install_deb(&fake, deb, &none).unwrap(),
            DebOutcome::OpenedInstaller
        );
        assert_eq!(fake.programs(), [format!("xdg-open {}", deb.display())]);
        assert!(install_deb(&fake, Path::new("x.deb"), &all).is_err());
    }

    #[test]
    fn deb_detection_needs_the_package_binary() {
        let dir = tempfile::tempdir().unwrap();
        let list = dir.path().join("hoppy.list");
        assert!(!deb_installed(Path::new(DEB_HOPPYW), &list));
        fs::write(&list, "/usr/bin/hoppyw\n").unwrap();
        assert!(deb_installed(Path::new(DEB_HOPPYW), &list));
        assert!(!deb_installed(
            Path::new("/home/m/hoppy/target/release/hoppyw"),
            &list
        ));
    }

    #[test]
    fn relaunch_helper_waits_for_the_process_then_runs_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started; rm -rf x");
        // A process that lives 0.5 s; the helper must start `touch` only afterwards.
        let mut sleeper = Command::new("sleep").arg("0.5").spawn().unwrap();
        let started = std::time::Instant::now();
        let mut helper = Command::new("/bin/sh")
            .args(relaunch_args(
                sleeper.id(),
                &[OsString::from("touch"), marker.clone().into_os_string()],
            ))
            .spawn()
            .unwrap();
        sleeper.wait().unwrap();
        assert!(helper.wait().unwrap().success());
        assert!(
            marker.exists(),
            "the path was passed as one argument, not shell text"
        );
        assert!(started.elapsed() >= Duration::from_millis(450));
    }
}
