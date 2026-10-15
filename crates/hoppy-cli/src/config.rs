//! Persistent device identity and preferences: `<config dir>/hoppy/device.json`.
//! `HOPPY_CONFIG_DIR` overrides the location (tests, portable installs).
//! New fields need `#[serde(default)]` so files written by older versions still load
//! (older versions ignore unknown fields).

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use hoppy_core::DeviceInfo;
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

/// Loads the identity, creating it on first run.
pub fn load_or_create() -> Result<Identity> {
    let path = config_dir()?.join("device.json");
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
    };
    save(&identity)?;
    Ok(identity)
}

pub fn save(identity: &Identity) -> Result<()> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    fs::write(
        dir.join("device.json"),
        serde_json::to_vec_pretty(identity)?,
    )?;
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

pub fn device_info(identity: &Identity) -> DeviceInfo {
    DeviceInfo {
        id: identity.id.clone(),
        name: identity.name.clone(),
        platform: DeviceInfo::current_platform().to_string(),
    }
}

fn default_name() -> String {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
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
        let off: Identity =
            serde_json::from_str(r#"{"id":"x","name":"PC","music":false}"#).unwrap();
        assert!(!off.music);
    }
}
