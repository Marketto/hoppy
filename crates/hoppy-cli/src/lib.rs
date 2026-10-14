//! Code shared by the two desktop binaries:
//! - `hoppy` (`src/main.rs`): the CLI, also used by scripts, tests and Linux/macOS;
//! - `hoppyw` (`src/bin/hoppyw.rs`): the Windows tray receiver + "Send to → Hoppy" GUI.
//!
//! Win32 FFI lives only in `platform.rs`; `gui.rs` is safe Rust on top of it.

pub mod config;
pub mod gui;
pub mod platform;

use std::net::{IpAddr, SocketAddr, TcpListener};
use std::path::PathBuf;

use anyhow::{Context, Result};
use hoppy_core::protocol::{DISCOVERY_PORT, TRANSFER_PORT};
use hoppy_core::{DeviceInfo, Discovery, DiscoveryConfig};

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
pub fn start_discovery(
    me: &DeviceInfo,
    transfer_port: u16,
    seeds: &[SocketAddr],
) -> Result<Discovery> {
    let mut cfg = DiscoveryConfig::new(me.clone(), transfer_port);
    cfg.seeds = seeds.to_vec();
    if transfer_port == 0 {
        cfg.bind_port = 0;
    }
    Discovery::start(cfg).context("starting discovery (is the UDP port blocked?)")
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

#[cfg(test)]
mod tests {
    use super::*;

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
