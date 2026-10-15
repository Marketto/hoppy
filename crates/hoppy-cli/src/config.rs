//! Persistent device identity and preferences (`device.json`), the protocol v1 device key
//! (`identity.key`, owner-only) and trust store (`trust.json`), all in [`state_dir`]:
//! the per-user config folder on Linux/macOS, the **local** (non-roaming) AppData folder on
//! Windows, so a roaming profile never copies the private key to a server or gives two PCs
//! the same id and key (older versions kept them in roaming AppData: moved once, see
//! [`migrate_from_roaming`]). `HOPPY_CONFIG_DIR` overrides the location (tests, portable
//! installs).
//! New fields need `#[serde(default)]` so files written by older versions still load
//! (older versions ignore unknown fields).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

use anyhow::{Context, Result};
use hoppy_core::{DeviceInfo, DeviceKey, Local, Trust};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub name: String,
    /// Windows `hoppyw send` plays a jingle on delivery (`hoppy music on|off`, tray menu
    /// "Sound when delivered"). Named `music` for compatibility with older device.json.
    #[serde(default = "default_music")]
    pub music: bool,
    /// Windows tray: version that last showed the firewall/Public-network balloon (once
    /// per version; the menu item stays while the problem does).
    #[serde(default)]
    pub firewall_hint: String,
    /// Linux desktop integration (right-click entries, launcher, autostart; macOS: services
    /// refresh + "Open at login"): the version that last wrote them, or `"off"` after
    /// `hoppy uninstall` (the tray then never adds them again by itself; `hoppy install`
    /// does).
    #[serde(default)]
    pub desktop_integration: String,
}

fn default_music() -> bool {
    true
}

pub fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("HOPPY_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    Ok(dirs::config_dir()
        .context("no config directory on this system")?
        .join("hoppy"))
}

const DEVICE_FILE: &str = "device.json";

/// Where `device.json`, the key and the trust store live (see the module docs).
pub fn state_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("HOPPY_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if cfg!(windows) {
        return Ok(dirs::data_local_dir()
            .context("no local AppData folder")?
            .join("Hoppy"));
    }
    config_dir()
}

/// Windows: moves files of older versions from roaming to local AppData, once per process.
/// On two PCs sharing a roaming profile the first one takes them; the second creates its
/// own id and key (as it should: they are different devices).
pub fn migrate_from_roaming() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("HOPPY_CONFIG_DIR").is_some() || !cfg!(windows) {
            return;
        }
        let (Ok(old), Ok(new)) = (config_dir(), state_dir()) else {
            return;
        };
        for name in [DEVICE_FILE, KEY_FILE, TRUST_FILE] {
            let (from, to) = (old.join(name), new.join(name));
            if from.exists() && !to.exists() && fs::create_dir_all(&new).is_ok() {
                secure_dir(&new);
                if fs::copy(&from, &to).is_ok() {
                    let _ = fs::remove_file(&from);
                }
            }
        }
    });
}

/// Windows: limits `dir` (and what is created in it) to the current user, once (a marker
/// file remembers it). Best effort: `%LOCALAPPDATA%` is already per-user; this also drops
/// the inherited entries (e.g. Administrators reading the private key without asking).
fn secure_dir(dir: &Path) {
    if !cfg!(windows) || std::env::var_os("HOPPY_CONFIG_DIR").is_some() {
        return;
    }
    let marker = dir.join(".owner-only");
    if marker.exists() {
        return;
    }
    let (Some(domain), Some(user)) = (std::env::var_os("USERDOMAIN"), std::env::var_os("USERNAME"))
    else {
        return;
    };
    let account = format!(
        "{}\\{}:(OI)(CI)F",
        domain.to_string_lossy(),
        user.to_string_lossy()
    );
    let mut cmd = Command::new("icacls");
    cmd.arg(dir)
        .args(["/inheritance:r", "/grant:r"])
        .arg(account)
        .arg("/q");
    crate::platform::no_window(&mut cmd);
    if cmd.status().is_ok_and(|s| s.success()) {
        let _ = fs::write(marker, b"");
    }
}

/// Loads the identity, creating it on first run.
pub fn load_or_create() -> Result<Identity> {
    migrate_from_roaming();
    let path = state_dir()?.join(DEVICE_FILE);
    if let Ok(raw) = fs::read_to_string(&path)
        && let Ok(identity) = serde_json::from_str::<Identity>(&raw)
    {
        return Ok(identity);
    }
    let identity = Identity {
        id: new_id(),
        name: default_name(),
        music: default_music(),
        firewall_hint: String::new(),
        desktop_integration: String::new(),
    };
    save(&identity)?;
    Ok(identity)
}

pub fn save(identity: &Identity) -> Result<()> {
    let dir = state_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    secure_dir(&dir);
    fs::write(dir.join(DEVICE_FILE), serde_json::to_vec_pretty(identity)?)?;
    Ok(())
}

/// Turns the delivery jingle on/off. Re-reads the file first so a long-running tray does not
/// overwrite a name changed meanwhile with `hoppy name`.
pub fn set_music(on: bool) -> Result<Identity> {
    let mut identity = load_or_create()?;
    identity.music = on;
    save(&identity)?;
    Ok(identity)
}

/// Remembers that the tray showed the firewall hint for `version` (re-reads the file first,
/// like `set_music`).
pub fn set_firewall_hint(version: &str) -> Result<Identity> {
    let mut identity = load_or_create()?;
    identity.firewall_hint = version.to_owned();
    save(&identity)?;
    Ok(identity)
}

/// Records the Linux/macOS desktop integration state (re-reads the file first, like `set_music`).
pub fn set_desktop_integration(state: &str) -> Result<Identity> {
    let mut identity = load_or_create()?;
    identity.desktop_integration = state.to_owned();
    save(&identity)?;
    Ok(identity)
}

pub fn device_info(identity: &Identity) -> DeviceInfo {
    DeviceInfo {
        id: identity.id.clone(),
        name: identity.name.clone(),
        platform: DeviceInfo::current_platform().to_string(),
    }
}

/// Device key file (spec §7.5): created on first run, never leaves this machine.
pub const KEY_FILE: &str = "identity.key";
/// Trust store (spec §7.5): remembered and verified devices.
pub const TRUST_FILE: &str = "trust.json";

/// This device for protocol v1 sessions: `identity` + the key and trust store in
/// [`state_dir`] (created on first use). A damaged trust store is set aside, not fatal:
/// `local.trust.take_warning()` says so (show or log it).
pub fn local(identity: &Identity) -> Result<Local> {
    migrate_from_roaming();
    let dir = state_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    secure_dir(&dir);
    let key = DeviceKey::load_or_create(&dir.join(KEY_FILE))
        .with_context(|| format!("device key in {}", dir.display()))?;
    let trust = Trust::open(&dir.join(TRUST_FILE))
        .with_context(|| format!("trust store {}", dir.join(TRUST_FILE).display()))?;
    Ok(Local::new(device_info(identity), key, trust))
}

fn default_name() -> String {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    // macOS host names end in the Bonjour suffix ("Marcos-MacBook-Air.local").
    let host = host.strip_suffix(".local").unwrap_or(&host);
    let name: String = host.chars().take(64).collect();
    if name.is_empty() {
        "Hoppy device".into()
    } else {
        name
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_device_json_loads_with_music_on() {
        let old: Identity = serde_json::from_str(r#"{"id":"x","name":"PC"}"#).unwrap();
        assert!(old.music);
        assert!(old.firewall_hint.is_empty());
        assert!(old.desktop_integration.is_empty());
        let off: Identity =
            serde_json::from_str(r#"{"id":"x","name":"PC","music":false}"#).unwrap();
        assert!(!off.music);
    }
}
