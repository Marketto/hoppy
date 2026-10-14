//! `hoppy` desktop client (PoC). Windows integration: `hoppy install` adds
//! "Send to → Hoppy", which runs `hoppy send --pause <files>`.

mod config;
mod platform;

use std::io::{self, BufRead, Write};
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use hoppy_core::protocol::TRANSFER_PORT;
use hoppy_core::transfer::serve;
use hoppy_core::{
    DeviceInfo, Discovery, DiscoveryConfig, Error, Offer, Peer, Progress, ReceiveHandler,
    SendOutcome, send_files,
};

#[derive(Parser)]
#[command(name = "hoppy", version, about = "Send files to nearby devices")]
struct Cli {
    /// Extra peer/discovery address (IP or IP:PORT) for networks without multicast.
    #[arg(long, global = true)]
    seed: Vec<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Send files, picking the destination from nearby devices.
    Send {
        files: Vec<PathBuf>,
        /// Destination: device name, id, or IP[:PORT]. Omit to pick interactively.
        #[arg(long)]
        to: Option<String>,
        /// Seconds to look for devices before showing the list.
        #[arg(long, default_value_t = 2)]
        wait: u64,
        /// Wait for Enter before exiting (used by the "Send to" shortcut).
        #[arg(long)]
        pause: bool,
    },
    /// Receive files and stay visible to nearby devices.
    Receive {
        /// Where to save files (default: Downloads/Hoppy).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Accept every transfer without asking (trusted networks only).
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value_t = TRANSFER_PORT)]
        port: u16,
    },
    /// List nearby devices.
    Peers {
        #[arg(long, default_value_t = 3)]
        wait: u64,
    },
    /// Show or change this device's name.
    Name { new_name: Option<String> },
    /// Windows: add "Send to → Hoppy" and start the receiver at login.
    Install,
    /// Windows: remove the shortcuts created by `install`.
    Uninstall,
}

fn main() {
    let cli = Cli::parse();
    let pause = matches!(cli.command, Command::Send { pause: true, .. });
    let result = run(cli);
    if let Err(e) = &result {
        eprintln!("✗ {e:#}");
    }
    if pause {
        print!("\nPress Enter to close…");
        let _ = io::stdout().flush();
        let _ = io::stdin().lock().read_line(&mut String::new());
    }
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}

fn run(cli: Cli) -> Result<()> {
    let identity = config::load_or_create()?;
    let me = config::device_info(&identity);
    let seeds = parse_seeds(&cli.seed)?;
    match cli.command {
        Command::Send {
            files, to, wait, ..
        } => cmd_send(&me, &seeds, files, to, wait),
        Command::Receive { dir, yes, port } => cmd_receive(&me, &seeds, dir, yes, port),
        Command::Peers { wait } => {
            let discovery = start_discovery(&me, 0, &seeds)?;
            let peers = discovery.wait_for_peers(usize::MAX, Duration::from_secs(wait));
            if peers.is_empty() {
                println!("No devices found. Is Hoppy receiving on the other device?");
            }
            for p in peers {
                println!(
                    "{:<24} {:<8} {}  {}",
                    p.device.name, p.device.platform, p.addr, p.device.id
                );
            }
            Ok(())
        }
        Command::Name { new_name } => {
            let mut identity = identity;
            if let Some(name) = new_name {
                identity.name = name.trim().chars().take(64).collect();
                config::save(&identity)?;
            }
            println!("{}", identity.name);
            Ok(())
        }
        Command::Install => platform::install(),
        Command::Uninstall => platform::uninstall(),
    }
}

fn parse_seeds(raw: &[String]) -> Result<Vec<SocketAddr>> {
    raw.iter()
        .map(|s| {
            s.parse::<SocketAddr>()
                .or_else(|_| {
                    s.parse::<IpAddr>()
                        .map(|ip| SocketAddr::new(ip, hoppy_core::protocol::DISCOVERY_PORT))
                })
                .with_context(|| format!("invalid --seed {s:?}"))
        })
        .collect()
}

/// `transfer_port == 0` → send-only node on an ephemeral UDP port (spec §2 rule 6).
fn start_discovery(me: &DeviceInfo, transfer_port: u16, seeds: &[SocketAddr]) -> Result<Discovery> {
    let mut cfg = DiscoveryConfig::new(me.clone(), transfer_port);
    cfg.seeds = seeds.to_vec();
    if transfer_port == 0 {
        cfg.bind_port = 0;
    }
    Discovery::start(cfg).context("starting discovery (is the UDP port blocked?)")
}

fn cmd_send(
    me: &DeviceInfo,
    seeds: &[SocketAddr],
    files: Vec<PathBuf>,
    to: Option<String>,
    wait: u64,
) -> Result<()> {
    if files.is_empty() {
        bail!("nothing to send: pass one or more files");
    }
    for f in &files {
        if !f.is_file() {
            bail!(
                "{} is not a file (folders arrive in a later version)",
                f.display()
            );
        }
    }
    let total: u64 = files
        .iter()
        .filter_map(|f| f.metadata().ok())
        .map(|m| m.len())
        .sum();
    println!("🐰 Hoppy — {} file(s), {}", files.len(), human(total));

    let target = match to.as_deref().map(parse_direct) {
        Some(Some(addr)) => Peer {
            device: DeviceInfo {
                id: String::new(),
                name: addr.to_string(),
                platform: "?".into(),
            },
            addr,
        },
        other => {
            let discovery = start_discovery(me, 0, seeds)?;
            let wanted = match other {
                Some(None) => to.clone(),
                _ => None,
            };
            pick_peer(&discovery, wanted.as_deref(), Duration::from_secs(wait))?
        }
    };

    println!(
        "→ {} ({})  waiting for them to accept…",
        target.device.name, target.addr
    );
    let mut bar = ProgressLine::default();
    let outcome = send_files(target.addr, me, &files, &mut |p| bar.update(&p))?;
    bar.finish();
    match outcome {
        SendOutcome::Delivered => println!("✓ Delivered to {}", target.device.name),
        SendOutcome::Declined(reason) => {
            bail!(
                "{} declined ({})",
                target.device.name,
                reason.unwrap_or_else(|| "no reason".into())
            )
        }
    }
    Ok(())
}

/// `IP` or `IP:PORT` given with `--to`.
fn parse_direct(s: &str) -> Option<SocketAddr> {
    s.parse::<SocketAddr>().ok().or_else(|| {
        s.parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, TRANSFER_PORT))
    })
}

fn pick_peer(discovery: &Discovery, wanted: Option<&str>, wait: Duration) -> Result<Peer> {
    if let Some(wanted) = wanted {
        let w = wanted.to_lowercase();
        let found = discovery
            .wait_for_peers(usize::MAX, wait)
            .into_iter()
            .find(|p| p.device.id == wanted || p.device.name.to_lowercase() == w);
        return found.with_context(|| format!("no nearby device called {wanted:?}"));
    }
    print!("Looking for nearby devices…");
    let _ = io::stdout().flush();
    let mut peers = discovery.wait_for_peers(usize::MAX, wait);
    loop {
        println!();
        if peers.is_empty() {
            println!("No devices found. Open Hoppy on the other device (same Wi-Fi).");
        } else {
            for (i, p) in peers.iter().enumerate() {
                println!("  {}) {}  · {}", i + 1, p.device.name, p.device.platform);
            }
        }
        print!("Choose a number, Enter to search again, an IP address, or q to quit: ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            bail!("cancelled");
        }
        let line = line.trim();
        if line.eq_ignore_ascii_case("q") {
            bail!("cancelled");
        }
        if let Ok(n) = line.parse::<usize>()
            && let Some(p) = peers.get(n.wrapping_sub(1))
        {
            return Ok(p.clone());
        }
        if let Some(addr) = parse_direct(line) {
            return Ok(Peer {
                device: DeviceInfo {
                    id: String::new(),
                    name: addr.to_string(),
                    platform: "?".into(),
                },
                addr,
            });
        }
        discovery.refresh();
        peers = discovery.wait_for_peers(usize::MAX, Duration::from_millis(1500));
    }
}

fn cmd_receive(
    me: &DeviceInfo,
    seeds: &[SocketAddr],
    dir: Option<PathBuf>,
    yes: bool,
    port: u16,
) -> Result<()> {
    let dir = match dir {
        Some(d) => d,
        None => dirs::download_dir()
            .or_else(dirs::home_dir)
            .context("no Downloads folder")?
            .join("Hoppy"),
    };
    let listener = TcpListener::bind(("0.0.0.0", port))
        .or_else(|_| TcpListener::bind(("0.0.0.0", 0)))
        .context("cannot open a TCP port for receiving")?;
    let port = listener.local_addr()?.port();
    let _discovery = start_discovery(me, port, seeds)?;
    println!("🐰 Hoppy is receiving as \"{}\" on port {port}", me.name);
    println!("   Files go to {}", dir.display());
    if yes {
        println!("   ⚠ Auto-accept is ON: anyone on this network can send you files.");
    }
    serve(listener, dir, Arc::new(CliReceiver { auto_accept: yes }))
}

struct CliReceiver {
    auto_accept: bool,
}

impl ReceiveHandler for CliReceiver {
    fn decide(&self, offer: &Offer, from: SocketAddr) -> bool {
        let names: Vec<&str> = offer
            .files
            .iter()
            .take(5)
            .map(|f| f.name.as_str())
            .collect();
        let more = offer.files.len().saturating_sub(names.len());
        let mut body = format!(
            "{} ({}, {}) wants to send {} file(s), {}:\n• {}",
            offer.sender.name,
            offer.sender.platform,
            from.ip(),
            offer.files.len(),
            human(offer.total_size()),
            names.join("\n• ")
        );
        if more > 0 {
            body.push_str(&format!("\n… and {more} more"));
        }
        if self.auto_accept {
            println!("\n{body}\n→ accepted automatically");
            return true;
        }
        platform::ask_accept("Hoppy — incoming files", &body)
    }

    fn finished(&self, from: SocketAddr, result: &Result<Vec<PathBuf>, Error>) {
        match result {
            Ok(paths) => {
                println!("✓ Received {} file(s) from {}", paths.len(), from.ip());
                for p in paths {
                    println!("   {}", p.display());
                }
            }
            Err(e) => println!("✗ Transfer from {} failed: {e}", from.ip()),
        }
    }
}

#[derive(Default)]
struct ProgressLine {
    last_pct: Option<u64>,
}

impl ProgressLine {
    fn update(&mut self, p: &Progress) {
        if let Progress::Bytes { done, total } = p {
            let pct = if *total == 0 { 100 } else { done * 100 / total };
            if self.last_pct != Some(pct) {
                self.last_pct = Some(pct);
                let filled = (pct / 5) as usize;
                print!(
                    "\r[{}{}] {pct:>3}%  {}",
                    "█".repeat(filled),
                    "·".repeat(20 - filled),
                    human(*done)
                );
                let _ = io::stdout().flush();
            }
        }
    }
    fn finish(&self) {
        if self.last_pct.is_some() {
            println!();
        }
    }
}

fn human(bytes: u64) -> String {
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
}
