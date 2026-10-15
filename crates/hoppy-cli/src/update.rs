//! Update check for the `hoppyw` tray (GitHub Releases of Marketto/hoppy).
//!
//! Pure logic only, unit-tested on every OS: version comparison, release JSON and
//! `SHA256SUMS.txt` parsing, install-location check, and a download that hashes while it
//! writes. The HTTP transport is the `Http` trait (WinHTTP in `platform::win::http_get`,
//! curl/wget on Linux and macOS). The release workflow must keep the asset names
//! `Hoppy-Setup-x64.exe`, `Hoppy-linux-amd64.deb`, `Hoppy-macos-universal.dmg` and
//! `SHA256SUMS.txt` stable (docs/release.md).
//!
//! Linux and macOS only announce a new version and link to the download portal (the .deb
//! is installed by the system's software installer, with the user's password; the Mac app
//! is not notarized, so each new copy goes through Gatekeeper's first-launch steps): no
//! download.

use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

pub use crate::platform::HttpSink;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const USER_AGENT: &str = concat!("Hoppy/", env!("CARGO_PKG_VERSION"));
pub const LATEST_API: &str = "https://api.github.com/repos/Marketto/hoppy/releases/latest";
pub const DOWNLOAD_BASE: &str = "https://github.com/Marketto/hoppy/releases/download";
pub const PORTAL_URL: &str = "https://marketto.github.io/hoppy/";
pub const INSTALLER_ASSET: &str = "Hoppy-Setup-x64.exe";
/// Ubuntu/Debian package (release.yml `linux` job).
pub const DEB_ASSET: &str = "Hoppy-linux-amd64.deb";
/// macOS universal disk image (release.yml `macos` job).
pub const MACOS_ASSET: &str = "Hoppy-macos-universal.dmg";
/// A release is offered only once this platform's package is attached.
pub const PLATFORM_ASSET: &str = if cfg!(windows) {
    INSTALLER_ASSET
} else if cfg!(target_os = "macos") {
    MACOS_ASSET
} else {
    DEB_ASSET
};
pub const SUMS_ASSET: &str = "SHA256SUMS.txt";
/// Inno Setup flags. `/VERYSILENT`: no window, so no Cancel button. `/UPDATE=1` makes
/// hoppy.iss relaunch the tray when done, and also if setup fails.
pub const INSTALLER_ARGS: [&str; 4] = [
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/UPDATE=1",
];
/// Each download gets its own fresh `%TEMP%\Hoppy-update-<uuid>` folder.
pub const UPDATE_DIR_PREFIX: &str = "Hoppy-update-";
/// Older update folders are removed when the tray starts (the installer that ran from one
/// cannot be deleted while it runs, so the tray never removes the folder right away).
pub const STALE_UPDATE_DIR: Duration = Duration::from_secs(24 * 3600);

/// First check this long after the tray starts (let login settle), then every `CHECK_EVERY`.
pub const FIRST_CHECK: Duration = Duration::from_secs(30);
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

const MAX_JSON: usize = 1 << 20;
const MAX_SUMS: usize = 64 * 1024;
const MAX_INSTALLER: u64 = 256 << 20;

/// `major.minor.patch`; `release` is false for a pre-release (`-rc1`, `-dev`), which sorts
/// before the release with the same numbers (field order makes the derived `Ord` do that).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub release: bool,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.release {
            f.write_str("-pre")?;
        }
        Ok(())
    }
}

/// Parses `v1.2.3` / `1.2.3` / `1.2.3-rc1`. Anything else (build metadata, missing or
/// non-digit parts) is `None`.
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let (core, pre) = match s.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (s, None),
    };
    if pre.is_some_and(str::is_empty) {
        return None;
    }
    let mut parts = core.split('.');
    let mut num = || {
        parts
            .next()
            .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|p| p.parse::<u64>().ok())
    };
    let (major, minor, patch) = (num()?, num()?, num()?);
    if parts.next().is_some() {
        return None;
    }
    Some(Version {
        major,
        minor,
        patch,
        release: pre.is_none(),
    })
}

/// True when `latest` is a release (never offer pre-releases) newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l.release && l > c,
        _ => false,
    }
}

/// Dev builds never look for updates.
pub fn is_dev_version(version: &str) -> bool {
    version.contains("-dev")
}

/// The installer puts `hoppyw.exe` in `%LOCALAPPDATA%\Programs\Hoppy`; anywhere else is a
/// portable copy (the installer would not replace it). Case- and separator-insensitive.
pub fn is_setup_install(exe_dir: &str, local_app_data: &str) -> bool {
    let norm = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    !local_app_data.trim().is_empty()
        && norm(exe_dir) == format!("{}\\programs\\hoppy", norm(local_app_data))
}

/// A release we can offer: the tag passed `parse_version` as a release, so it is only
/// `v`, digits and dots (safe in URLs and file names).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    tag: String,
}

impl Release {
    pub fn asset_url(&self, asset: &str) -> String {
        format!("{DOWNLOAD_BASE}/{}/{asset}", self.tag)
    }

    pub fn installer_file_name(&self) -> String {
        format!("Hoppy-Setup-{}.exe", self.version)
    }
}

/// Reads `/releases/latest`. `None` unless the tag is a plain release and both the
/// installer and the checksum file are attached (a release still uploading is skipped).
pub fn parse_latest(json: &[u8]) -> Option<Release> {
    parse_latest_for(json, INSTALLER_ASSET)
}

/// Like [`parse_latest`], requiring `asset` (this platform's package) instead.
pub fn parse_latest_for(json: &[u8], asset: &str) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let tag = v.get("tag_name")?.as_str()?;
    let version = parse_version(tag).filter(|v| v.release)?;
    if tag.contains('+') || tag.trim() != tag {
        return None;
    }
    let names: Vec<&str> = v
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(|a| a.get("name")?.as_str())
        .collect();
    if !names.contains(&asset) || !names.contains(&SUMS_ASSET) {
        return None;
    }
    Some(Release {
        version,
        tag: tag.to_owned(),
    })
}

/// Finds `asset` in `sha256sum` output (`<hex>  <name>`, `*name` in binary mode).
pub fn expected_sha256(sums: &str, asset: &str) -> Option<[u8; 32]> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        let name = name.trim_start();
        let name = name.strip_prefix('*').unwrap_or(name);
        (name == asset).then(|| parse_hex32(hash)).flatten()
    })
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// HTTPS GET; `sink` receives each chunk plus the Content-Length when known. Non-200 is
/// an error.
pub trait Http {
    fn get(&self, url: &str, sink: &mut HttpSink<'_>) -> Result<()>;
}

fn get_small(http: &dyn Http, url: &str, max: usize) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    http.get(url, &mut |chunk, _| {
        if body.len() + chunk.len() > max {
            bail!("response too large");
        }
        body.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok(body)
}

/// The latest release if it is newer than `current`.
pub fn check(http: &dyn Http, current: &str) -> Result<Option<Release>> {
    let json = get_small(http, LATEST_API, MAX_JSON)?;
    Ok(parse_latest_for(&json, PLATFORM_ASSET)
        .filter(|r| is_newer(&r.version.to_string(), current)))
}

/// The downloaded installer does not match `SHA256SUMS.txt`; it has been deleted.
#[derive(Debug)]
pub struct HashMismatch;

impl fmt::Display for HashMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the downloaded installer does not match SHA256SUMS.txt")
    }
}

impl std::error::Error for HashMismatch {}

/// Downloads the installer of `release` into `dir`, verifying it against the release's
/// `SHA256SUMS.txt`. Written as `*.part` and renamed only when the hash matches; on any
/// error nothing is left behind. `progress` gets 0..=100 when the size is known.
pub fn download(
    http: &dyn Http,
    release: &Release,
    dir: &Path,
    progress: &mut dyn FnMut(u64),
) -> Result<PathBuf> {
    let sums = get_small(http, &release.asset_url(SUMS_ASSET), MAX_SUMS)?;
    let sums = String::from_utf8_lossy(&sums);
    let expected = expected_sha256(&sums, INSTALLER_ASSET)
        .with_context(|| format!("{INSTALLER_ASSET} is not listed in {SUMS_ASSET}"))?;

    let dest = dir.join(release.installer_file_name());
    let part = dir.join(format!("{}.part", release.installer_file_name()));
    let result = fetch_to(http, &release.asset_url(INSTALLER_ASSET), &part, progress);
    // `fetch_to` closed the file: Windows cannot rename or delete open files.
    let verified = result.and_then(|actual| {
        if actual != expected {
            return Err(HashMismatch.into());
        }
        fs::rename(&part, &dest).context("saving the installer")?;
        Ok(dest)
    });
    if verified.is_err() {
        let _ = fs::remove_file(&part);
    }
    verified
}

/// Downloads like [`download`], but into a new private folder under `temp` (random name,
/// created with `create_dir`, so it can never be an existing or attacker-made folder).
/// On error the folder is removed again.
pub fn download_fresh(
    http: &dyn Http,
    release: &Release,
    temp: &Path,
    progress: &mut dyn FnMut(u64),
) -> Result<PathBuf> {
    let dir = temp.join(format!(
        "{UPDATE_DIR_PREFIX}{}",
        uuid::Uuid::new_v4().simple()
    ));
    create_private_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let result = download(http, release, &dir, progress);
    if result.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    result
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    // Windows: %TEMP% is per-user and the new folder inherits its owner-only ACL.
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(dir) // fails if it already exists
    }
    #[cfg(not(unix))]
    {
        fs::DirBuilder::new().create(dir) // fails if it already exists
    }
}

/// Removes `Hoppy-update-*` folders in `temp` last modified more than `max_age` ago.
/// Best effort; symlinks and plain files with that name are left alone.
pub fn remove_stale_update_dirs(temp: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(temp) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(UPDATE_DIR_PREFIX))
        {
            continue;
        }
        // `DirEntry::metadata` does not follow symlinks.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if meta.is_dir() && old {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn fetch_to(
    http: &dyn Http,
    url: &str,
    path: &Path,
    progress: &mut dyn FnMut(u64),
) -> Result<[u8; 32]> {
    let mut file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut done: u64 = 0;
    http.get(url, &mut |chunk, total| {
        done += chunk.len() as u64;
        if done > MAX_INSTALLER || total.is_some_and(|t| done > t) {
            bail!("the installer is larger than expected");
        }
        file.write_all(chunk)?;
        hasher.update(chunk);
        if let Some(t) = total.filter(|&t| t > 0) {
            progress(done.saturating_mul(100) / t);
        }
        Ok(())
    })?;
    file.sync_all()?;
    drop(file);
    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn v(s: &str) -> Version {
        parse_version(s).unwrap()
    }

    #[test]
    fn parses_versions() {
        assert_eq!(
            v("v0.2.0"),
            Version {
                major: 0,
                minor: 2,
                patch: 0,
                release: true
            }
        );
        assert_eq!(v("1.10.3"), v("V1.10.3"));
        assert!(!v("0.3.0-rc1").release);
        assert!(!v("0.0.0-dev").release);
        for bad in [
            "", "v", "1.2", "1.2.3.4", "1..3", "1.2.x", "1.2.3-", "+1.2.3", "1.2.3+b",
        ] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
        assert_eq!(v("v1.2.3").to_string(), "1.2.3");
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.9")); // numeric, not lexicographic
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.2.0", "0.2.0-rc1")); // pre-release is older
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("v0.1.9", "0.2.0"));
        assert!(!is_newer("0.3.0-rc1", "0.2.0")); // never offer pre-releases
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("0.2.0", "garbage"));
        assert!(is_dev_version("0.0.0-dev"));
        assert!(!is_dev_version("0.2.0"));
    }

    #[test]
    fn setup_install_location() {
        let lad = r"C:\Users\x\AppData\Local";
        assert!(is_setup_install(
            r"C:\Users\x\AppData\Local\Programs\Hoppy",
            lad
        ));
        assert!(is_setup_install(
            r"c:\users\x\appdata\local\programs\hoppy\",
            lad
        ));
        assert!(is_setup_install(
            "C:/Users/x/AppData/Local/Programs/Hoppy",
            lad
        ));
        assert!(!is_setup_install(
            r"C:\Users\x\Downloads\Hoppy-windows-x64",
            lad
        ));
        assert!(!is_setup_install(
            r"C:\Users\x\AppData\Local\Programs\Hoppy\sub",
            lad
        ));
        assert!(!is_setup_install(r"\Programs\Hoppy", ""));
    }

    fn release_json(tag: &str, assets: &[&str]) -> Vec<u8> {
        let assets: Vec<_> = assets
            .iter()
            .map(|n| serde_json::json!({ "name": n, "size": 1 }))
            .collect();
        serde_json::to_vec(&serde_json::json!({ "tag_name": tag, "assets": assets })).unwrap()
    }

    #[test]
    fn parses_latest_release() {
        let both = [INSTALLER_ASSET, SUMS_ASSET, "Hoppy-windows-x64.zip"];
        let r = parse_latest(&release_json("v0.2.0", &both)).unwrap();
        assert_eq!(r.version, v("0.2.0"));
        assert_eq!(
            r.asset_url(INSTALLER_ASSET),
            "https://github.com/Marketto/hoppy/releases/download/v0.2.0/Hoppy-Setup-x64.exe"
        );
        assert_eq!(r.installer_file_name(), "Hoppy-Setup-0.2.0.exe");
        assert!(parse_latest(&release_json("v0.2.0", &[INSTALLER_ASSET])).is_none());
        assert!(parse_latest(&release_json("v0.3.0-rc1", &both)).is_none());
        assert!(parse_latest(&release_json("v0.2.0/../x", &both)).is_none());
        assert!(parse_latest(&release_json(" v0.2.0", &both)).is_none());
        assert!(parse_latest(b"{").is_none());
        assert!(parse_latest(b"{\"tag_name\": 3}").is_none());
        // Linux waits for the .deb, Windows for the installer.
        let deb = [DEB_ASSET, SUMS_ASSET];
        assert!(parse_latest_for(&release_json("v0.2.0", &deb), DEB_ASSET).is_some());
        assert!(parse_latest_for(&release_json("v0.2.0", &both), DEB_ASSET).is_none());
        assert!(parse_latest(&release_json("v0.2.0", &deb)).is_none());
        // macOS waits for the disk image.
        let dmg = [MACOS_ASSET, SUMS_ASSET];
        assert!(parse_latest_for(&release_json("v0.2.0", &dmg), MACOS_ASSET).is_some());
        assert!(parse_latest_for(&release_json("v0.2.0", &deb), MACOS_ASSET).is_none());
    }

    const H: &str = "00112233445566778899aabbccddeeff00112233445566778899AABBCCDDEEFF";

    #[test]
    fn reads_sha256sums() {
        let sums = format!(
            "{}  Hoppy-windows-x64.zip\n{H}  {INSTALLER_ASSET}\r\n",
            "f".repeat(64)
        );
        let want = parse_hex32(H).unwrap();
        assert_eq!(want[0], 0x00);
        assert_eq!(want[31], 0xff);
        assert_eq!(expected_sha256(&sums, INSTALLER_ASSET), Some(want));
        assert_eq!(
            expected_sha256(&format!("{H} *{INSTALLER_ASSET}"), INSTALLER_ASSET),
            Some(want)
        );
        assert_eq!(expected_sha256(&sums, "other.exe"), None);
        assert_eq!(
            expected_sha256(&format!("{}  {INSTALLER_ASSET}", &H[1..]), INSTALLER_ASSET),
            None
        );
    }

    /// In-memory "GitHub" that serves bytes in small chunks.
    struct Fake(HashMap<String, Vec<u8>>);

    impl Http for Fake {
        fn get(&self, url: &str, sink: &mut HttpSink<'_>) -> Result<()> {
            let body = self.0.get(url).context("404")?;
            for chunk in body.chunks(7) {
                sink(chunk, Some(body.len() as u64))?;
            }
            Ok(())
        }
    }

    fn fake(tag: &str, installer: &[u8], listed_hash: &str) -> Fake {
        let base = format!("{DOWNLOAD_BASE}/{tag}");
        let mut m = HashMap::new();
        m.insert(
            LATEST_API.to_owned(),
            release_json(tag, &[INSTALLER_ASSET, DEB_ASSET, SUMS_ASSET]),
        );
        m.insert(format!("{base}/{INSTALLER_ASSET}"), installer.to_vec());
        m.insert(
            format!("{base}/{SUMS_ASSET}"),
            format!("{listed_hash}  {INSTALLER_ASSET}\n").into_bytes(),
        );
        Fake(m)
    }

    fn hex(data: &[u8]) -> String {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn check_offers_only_newer() {
        let http = fake("v0.2.0", b"x", &hex(b"x"));
        assert_eq!(check(&http, "0.1.0").unwrap().unwrap().version, v("0.2.0"));
        assert!(check(&http, "0.2.0").unwrap().is_none());
        assert!(check(&Fake(HashMap::new()), "0.1.0").is_err());
    }

    #[test]
    fn download_verifies_and_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let installer = b"MZ fake installer bytes, long enough for a few chunks";
        let http = fake("v0.2.0", installer, &hex(installer));
        let release = check(&http, "0.1.0").unwrap().unwrap();
        let mut seen = Vec::new();
        let path = download(&http, &release, dir.path(), &mut |p| seen.push(p)).unwrap();
        assert_eq!(path, dir.path().join("Hoppy-Setup-0.2.0.exe"));
        assert_eq!(fs::read(&path).unwrap(), installer);
        assert_eq!(seen.last(), Some(&100));
        assert!(seen.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1); // no .part left
    }

    #[test]
    fn download_rejects_a_bad_hash_and_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let http = fake("v0.2.0", b"tampered", &hex(b"original"));
        let release = check(&http, "0.1.0").unwrap().unwrap();
        let err = download(&http, &release, dir.path(), &mut |_| {}).unwrap_err();
        assert!(err.is::<HashMismatch>());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

        let http = fake("v0.2.0", b"x", "not-a-hash");
        let err = download(&http, &release, dir.path(), &mut |_| {}).unwrap_err();
        assert!(!err.is::<HashMismatch>());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn download_fresh_uses_a_new_folder_and_cleans_up_on_error() {
        let temp = tempfile::tempdir().unwrap();
        let installer = b"MZ fake";
        let http = fake("v0.2.0", installer, &hex(installer));
        let release = check(&http, "0.1.0").unwrap().unwrap();
        let a = download_fresh(&http, &release, temp.path(), &mut |_| {}).unwrap();
        let b = download_fresh(&http, &release, temp.path(), &mut |_| {}).unwrap();
        assert_ne!(a.parent(), b.parent());
        for p in [&a, &b] {
            let parent = p.parent().unwrap();
            assert_eq!(parent.parent(), Some(temp.path()));
            let name = parent.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with(UPDATE_DIR_PREFIX));
            assert_eq!(fs::read(p).unwrap(), installer);
            assert_eq!(fs::read_dir(parent).unwrap().count(), 1);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(parent).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o700);
            }
        }

        let bad = fake("v0.2.0", b"tampered", &hex(b"original"));
        let err = download_fresh(&bad, &release, temp.path(), &mut |_| {}).unwrap_err();
        assert!(err.is::<HashMismatch>());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn removes_only_stale_update_folders() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join(format!("{UPDATE_DIR_PREFIX}old"));
        let other = temp.path().join("not-hoppy");
        let file = temp.path().join(format!("{UPDATE_DIR_PREFIX}file"));
        fs::create_dir(&old).unwrap();
        fs::write(old.join("Hoppy-Setup-0.2.0.exe"), b"x").unwrap();
        fs::create_dir(&other).unwrap();
        fs::write(&file, b"x").unwrap();

        // Fresh folders survive a 1-day limit...
        remove_stale_update_dirs(temp.path(), STALE_UPDATE_DIR);
        assert!(old.exists());
        // ...and go once older than the limit (zero here; mtime is in the past).
        std::thread::sleep(Duration::from_millis(20));
        remove_stale_update_dirs(temp.path(), Duration::from_millis(1));
        assert!(!old.exists());
        assert!(other.exists());
        assert!(file.exists());
    }
}
