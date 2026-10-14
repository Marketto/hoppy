//! Persistent device identity: `<config dir>/hoppy/device.json`.
//! `HOPPY_CONFIG_DIR` overrides the location (tests, portable installs).

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use hoppy_core::DeviceInfo;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub name: String,
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
