//! `hoppy` desktop CLI (PoC). On Windows, `hoppy install` adds "Send to → Hoppy", the
//! "Send with Hoppy" right-click verb and a login item, pointing at the `hoppyw` tray/GUI
//! binary when it sits next to this one (fallback: `hoppy send --pause` / `hoppy receive`,
//! no verb). Shared code: `src/lib.rs`.

use std::io::{self, BufRead, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use hoppy::{
    DialogGuard, bind_receiver, config, default_receive_dir, diag, display_file_name, display_name,
    display_platform, firewall, human, parse_direct, parse_seeds, platform, start_discovery,
};
use hoppy_core::protocol::TRANSFER_PORT;
use hoppy_core::transfer::serve;
use hoppy_core::{
    DeviceInfo, Discovery, Error, Offer, Peer, Progress, ReceiveHandler, SendOutcome, send_files,
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

#[derive(Clone, Copy, ValueEnum)]
enum OnOff {
    On,
    Off,
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
    /// Show or change whether the Windows "Send to → Hoppy" window plays music.
    /// (`hoppy send` is always silent.)
    Music { state: Option<OnOff> },
    /// Windows: add "Send to → Hoppy", the "Send with Hoppy" right-click entry and start
    /// the receiver at login.
    Install,
    /// Windows: remove what `install` added (and the classic menu, if Hoppy turned it on).
    Uninstall,
    /// Windows 11: show or change whether right-click opens the classic menu, where
    /// "Send with Hoppy" is visible at once (per user; applies after Explorer restarts).
    ClassicMenu {
        state: Option<OnOff>,
        /// Restart Explorer now so the change is visible (closes File Explorer windows).
        #[arg(long)]
        restart_explorer: bool,
    },
    /// Print where the `hoppyw` diagnostic log is, and its last lines.
    Log {
        #[arg(long, default_value_t = 40)]
        lines: usize,
    },
    /// Windows: can other devices reach this PC through the firewall? `--fix` adds the
    /// Hoppy rules (asks for administrator permission).
    Firewall {
        /// Add the inbound rules for Hoppy (Private + Domain networks; UAC prompt).
        #[arg(long)]
        fix: bool,
        /// Add the rules in this process (it must already run as administrator).
        #[arg(long, hide = true, conflicts_with_all = ["fix", "remove"])]
        apply: bool,
        /// Remove the Hoppy rules (run from an administrator terminal).
        #[arg(long, conflicts_with = "fix")]
        remove: bool,
    },
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
    // Before load_or_create: the elevated copy may run as another (admin) user, whose
    // profile must not get a device.json.
    if let Command::Firewall { fix, apply, remove } = cli.command {
        return cmd_firewall(fix, apply, remove);
    }
    // No device.json needed for these.
    match cli.command {
        Command::Log { lines } => return cmd_log(lines),
        Command::ClassicMenu {
            state,
            restart_explorer,
        } => return cmd_classic_menu(state, restart_explorer),
        _ => {}
    }
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
        Command::Music { state } => {
            let identity = match state {
                Some(state) => config::set_music(matches!(state, OnOff::On))?,
                None => identity,
            };
            println!("{}", if identity.music { "on" } else { "off" });
            Ok(())
        }
        Command::Install => platform::install(),
        Command::Uninstall => platform::uninstall(),
        Command::Firewall { .. } | Command::Log { .. } | Command::ClassicMenu { .. } => {
            unreachable!("handled above")
        }
    }
}

fn cmd_log(lines: usize) -> Result<()> {
    let path = diag::log_path().context("no local data folder on this system")?;
    println!("{}", path.display());
    match diag::tail(&path, lines) {
        Ok(text) => println!("{text}"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            println!("(empty: hoppyw has not written anything yet)");
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    }
    Ok(())
}

fn cmd_classic_menu(state: Option<OnOff>, restart_explorer: bool) -> Result<()> {
    if let Some(state) = state {
        let changed = platform::set_classic_menu(matches!(state, OnOff::On))?;
        if changed && !restart_explorer {
            println!("Takes effect after File Explorer restarts (or at the next sign-in).");
        }
    }
    match platform::classic_menu() {
        Some(on) => println!("{}", if on { "on" } else { "off" }),
        None => println!("not applicable (Windows 11 only)"),
    }
    if restart_explorer {
        platform::restart_explorer()?;
    }
    Ok(())
}

fn cmd_firewall(fix: bool, apply: bool, remove: bool) -> Result<()> {
    if !cfg!(windows) {
        bail!("`hoppy firewall` is Windows-only; elsewhere open TCP 47471 and UDP 47470 by hand");
    }
    let exe = std::env::current_exe()?;
    let dir = exe.parent().context("no program folder")?.to_path_buf();
    if apply {
        firewall::apply(&dir)?;
        println!("✓ Firewall rules added for {}", dir.display());
        return Ok(());
    }
    if remove {
        firewall::remove(&dir)?;
        println!("✓ Firewall rules removed");
        return Ok(());
    }
    if fix {
        println!("Asking for administrator permission…");
        let child = platform::win::run_elevated(None, &exe, "firewall --apply")?;
        match child.wait(120_000) {
            Some(0) => println!("✓ Firewall rules added"),
            Some(code) => bail!(
                "adding the rules failed (exit code {code}); try `hoppy firewall --apply` \
                 from an administrator terminal"
            ),
            None => bail!("adding the rules did not finish"),
        }
    }
    let programs = firewall::programs_in(&dir);
    let status = firewall::check(&programs)
        .context("could not read the firewall state (PowerShell NetSecurity cmdlets)")?;
    let networks: Vec<&str> = status
        .networks
        .iter()
        .map(|n| match n {
            firewall::Network::Domain => "Domain",
            firewall::Network::Private => "Private",
            firewall::Network::Public => "Public",
        })
        .collect();
    println!(
        "Connected networks: {}",
        if networks.is_empty() {
            "none".to_owned()
        } else {
            networks.join(", ")
        }
    );
    for (i, program) in programs.iter().enumerate() {
        let list = |allow: bool| {
            let mut p = firewall::Profiles::default();
            for r in status
                .rules
                .iter()
                .filter(|r| r.program == i && r.allow == allow)
            {
                p.domain |= r.profiles.domain;
                p.private |= r.profiles.private;
                p.public |= r.profiles.public;
            }
            let names: Vec<&str> = [
                (p.domain, "Domain"),
                (p.private, "Private"),
                (p.public, "Public"),
            ]
            .into_iter()
            .filter_map(|(on, name)| on.then_some(name))
            .collect();
            if names.is_empty() {
                "-".to_owned()
            } else {
                names.join(", ")
            }
        };
        let verdict = match status.verdict(i) {
            firewall::Verdict::Ok => "✓ reachable".to_owned(),
            firewall::Verdict::PublicNetwork => {
                "✗ the network is Public: set it to Private in Windows Settings".to_owned()
            }
            firewall::Verdict::NeedsRules => "✗ blocked: run `hoppy firewall --fix` (on \
                corporate PCs a policy may ignore local rules: ask IT)"
                .to_owned(),
        };
        println!(
            "{}\n  allowed on: {}\n  blocked on: {}\n  {verdict}",
            program.display(),
            list(true),
            list(false)
        );
    }
    Ok(())
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
        None => default_receive_dir()?,
    };
    let listener = bind_receiver(port)?;
    let port = listener.local_addr()?.port();
    let _discovery = start_discovery(me, port, seeds)?;
    println!("🐰 Hoppy is receiving as \"{}\" on port {port}", me.name);
    println!("   Files go to {}", dir.display());
    if yes {
        println!("   ⚠ Auto-accept is ON: anyone on this network can send you files.");
    }
    let handler = CliReceiver {
        auto_accept: yes,
        asking: AtomicBool::new(false),
    };
    serve(listener, dir, Arc::new(handler))
}

struct CliReceiver {
    auto_accept: bool,
    /// One prompt at a time; offers arriving meanwhile are declined.
    asking: AtomicBool,
}

impl ReceiveHandler for CliReceiver {
    fn decide(&self, offer: &Offer, from: SocketAddr) -> bool {
        let names: Vec<String> = offer
            .files
            .iter()
            .take(5)
            .map(|f| display_file_name(&f.name))
            .collect();
        let more = offer.files.len().saturating_sub(names.len());
        let mut body = format!(
            "{} ({}, {}) wants to send {} file(s), {}:\n• {}",
            display_name(&offer.sender.name),
            display_platform(&offer.sender.platform),
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
        let Some(_asking) = DialogGuard::try_take(&self.asking) else {
            println!("\n{body}\n→ declined: another offer is waiting for an answer");
            return false;
        };
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
