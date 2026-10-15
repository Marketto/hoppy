//! `hoppy` desktop CLI (PoC). On Windows, `hoppy install` adds "Send to → Hoppy", the
//! "Send with Hoppy" right-click verb and a login item, pointing at the `hoppyw` tray/GUI
//! binary when it sits next to this one (fallback: `hoppy send --pause` / `hoppy receive`,
//! no verb). On Linux it adds the right-click entries (Nautilus, Nemo, Dolphin), a
//! launcher and the tray at login for this user (needs `hoppyw` next to `hoppy`).
//! Shared code: `src/lib.rs`.

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
    display_platform, firewall, human, parse_direct, parse_seeds, platform, receive_forever,
    start_discovery, start_discovery_on,
};
use hoppy_core::protocol::{DISCOVERY_PORT, TRANSFER_PORT};
use hoppy_core::{
    DeviceInfo, Discovery, Error, Local, Offer, Peer, PeerKey, Progress, QrPayload, ReceiveHandler,
    SendOutcome, SessionInfo, TrustState, confirm_qr, send_to_peer, send_via_callback,
};

#[derive(Parser)]
#[command(name = "hoppy", version, about = "Send files to nearby devices")]
struct Cli {
    /// Extra peer/discovery address (IP or IP:PORT) for networks without multicast.
    #[arg(long, global = true)]
    seed: Vec<String>,
    /// Never sweep the local subnet with unicast queries (also: HOPPY_NO_SWEEP=1). Without
    /// the sweep an iPhone/iPad is not found by `send`/`peers` (docs/protocol.md §2.1).
    #[arg(long, global = true)]
    no_sweep: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Subcommand)]
enum TrustAction {
    /// This device's key fingerprint and the remembered devices (default).
    List,
    /// Forget a device (by name or id): the next contact asks again, as for a new device.
    /// Use it when a device was reinstalled and Hoppy warns that its key changed.
    Forget { device: String },
    /// Mark a device as verified: either paste the payload of the QR code it shows
    /// (`--qr`; the other device learns it too), or compare its fingerprint by eye.
    Verify {
        /// Name or id of a remembered device (not needed with --qr).
        device: Option<String>,
        /// Payload of the other device's QR code: `hoppy:verify:1:…`.
        #[arg(long)]
        qr: Option<String>,
        /// With --qr: the device's address (IP[:PORT]) instead of looking for it.
        #[arg(long)]
        to: Option<String>,
        /// Do not ask for confirmation (fingerprint compared already).
        #[arg(long)]
        yes: bool,
    },
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
        /// Debug: skip the direct connection and ask the receiver to connect back
        /// (callback, as used when its firewall blocks incoming connections).
        #[arg(long)]
        no_direct: bool,
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
        /// Discovery UDP port (tests only; other devices look on 47470).
        #[arg(long, default_value_t = DISCOVERY_PORT, hide = true)]
        udp_port: u16,
        /// Print this device's QR verification payload (spec §7.7) for a phone to scan;
        /// its one-time code is accepted while this receiver runs (10 minutes).
        #[arg(long)]
        show_qr: bool,
    },
    /// Devices this one remembers (encryption keys): list, forget, verify.
    Trust {
        #[command(subcommand)]
        action: Option<TrustAction>,
    },
    /// List nearby devices.
    Peers {
        #[arg(long, default_value_t = 3)]
        wait: u64,
    },
    /// Show or change this device's name.
    Name { new_name: Option<String> },
    /// Show or change whether "Send with Hoppy" (the desktop app) plays a short sound when
    /// the files are delivered (`hoppy send` is always silent).
    Music { state: Option<OnOff> },
    /// Add the "Send with Hoppy" right-click entry and start the receiver at login
    /// (Windows: also "Send to → Hoppy"; Linux: Nautilus, Nemo and Dolphin, for this user;
    /// macOS: "Open at login" for Hoppy.app, whose Finder service is always there).
    Install,
    /// Remove what `install` added (Windows: also the classic menu, if Hoppy turned it on;
    /// Linux, macOS: the tray then stops adding it back).
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
    if cli.no_sweep {
        hoppy::disable_sweep();
    }
    match cli.command {
        Command::Send {
            files,
            to,
            wait,
            no_direct,
            ..
        } => cmd_send(
            &config::local(&identity)?,
            &seeds,
            files,
            to,
            wait,
            no_direct,
        ),
        Command::Receive {
            dir,
            yes,
            port,
            udp_port,
            show_qr,
        } => cmd_receive(
            &config::local(&identity)?,
            &seeds,
            dir,
            yes,
            port,
            udp_port,
            show_qr,
        ),
        Command::Trust { action } => cmd_trust(&config::local(&identity)?, &seeds, action),
        Command::Peers { wait } => {
            let discovery = start_discovery(&me, 0, &seeds)?;
            let peers = discovery.wait_for_peers(usize::MAX, Duration::from_secs(wait));
            if peers.is_empty() {
                println!("No devices found. Is Hoppy receiving on the other device?");
            }
            for p in peers {
                let crypto = if p.crypto.unwrap_or(0) >= 1 {
                    "🔒"
                } else {
                    "⚠ v0"
                };
                println!(
                    "{:<24} {:<8} {:<5} {}  {}",
                    p.device.name, p.device.platform, crypto, p.addr, p.device.id
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
    if cfg!(target_os = "macos") {
        bail!(
            "`hoppy firewall` is Windows-only. On a Mac the firewall is off by default; if it is \
             on, allow Hoppy in System Settings → Network → Firewall → Options (incoming \
             connections), or answer \"Allow\" when macOS asks"
        );
    }
    if !cfg!(windows) {
        bail!(
            "`hoppy firewall` is Windows-only; elsewhere allow TCP 47471 and UDP 47470 by hand \
             (Ubuntu's ufw is off by default: `sudo ufw allow 47471/tcp` and `47470/udp` if on)"
        );
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
        let child = platform::ui::run_elevated(None, &exe, "firewall --apply")?;
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
    local: &Local,
    seeds: &[SocketAddr],
    files: Vec<PathBuf>,
    to: Option<String>,
    wait: u64,
    no_direct: bool,
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

    // Kept alive for the whole send: the callback fallback (spec §6) needs it.
    let mut discovery = None;
    let target = match to.as_deref().map(parse_direct) {
        Some(Some(addr)) => Peer::at(addr),
        other => {
            let d = discovery.insert(start_discovery(&local.device, 0, seeds)?);
            let wanted = match other {
                Some(None) => to.clone(),
                _ => None,
            };
            pick_peer(d, wanted.as_deref(), Duration::from_secs(wait))?
        }
    };
    // A bare IP has no device id, which a callback needs (`to`): direct only.
    let discovery = discovery.as_ref().filter(|_| !target.device.id.is_empty());
    if no_direct && discovery.is_none() {
        bail!("--no-direct needs a discovered device (name or list), not an IP address");
    }

    println!(
        "→ {} ({})  waiting for them to accept…",
        target.device.name, target.addr
    );
    let mut bar = ProgressLine::default();
    let mut progress = |p: Progress| {
        match &p {
            Progress::Fallback => println!(
                "  {} does not answer directly; asking it to connect back (up to 20 s)…",
                target.device.name
            ),
            Progress::Session(session) => println!("  {}", security_line(session)),
            _ => {}
        }
        bar.update(&p);
    };
    let result = match discovery {
        Some(d) if no_direct => send_via_callback(&target, local, d, &files, &mut progress),
        d => send_to_peer(&target, local, d, &files, &mut progress),
    };
    bar.finish();
    let outcome = match result {
        Err(Error::Unreachable(detail)) => bail!(
            "cannot reach {} ({detail}).\n  Check that Hoppy is open there, on the same \
             network. On a Windows PC, run `hoppy firewall --fix` there (or allow Hoppy in \
             its firewall); on a company PC ask IT to allow Hoppy.",
            target.device.name
        ),
        Err(Error::KeyChanged(name)) => bail!(
            "⚠ the security key of {name} has changed: nothing was sent.\n  If {name} was \
             reinstalled or reset, run `hoppy trust forget \"{name}\"` and send again (then \
             verify it). Otherwise someone on this network may be pretending to be {name}."
        ),
        other => other?,
    };
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

/// One line about a session's protection, for the sender's and the receiver's terminal.
fn security_line(session: &SessionInfo) -> String {
    let (Some(trust), Some(sas)) = (session.trust, session.sas.as_ref()) else {
        return "⚠ NOT encrypted: the other device runs an old Hoppy version".into();
    };
    let state = match trust {
        TrustState::New => "new device: compare the code on both screens",
        TrustState::Known => "known device, not verified: compare the code",
        TrustState::Verified => "✓ verified device",
        TrustState::KeyChanged => "⚠ KEY CHANGED",
    };
    format!(
        "🔒 encrypted · {state} · code {} ({}) · {}",
        sas.text,
        sas.digits,
        sas.words(false).join(" ")
    )
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
            return Ok(Peer::at(addr));
        }
        discovery.refresh();
        peers = discovery.wait_for_peers(usize::MAX, Duration::from_millis(1500));
    }
}

fn cmd_receive(
    local: &Local,
    seeds: &[SocketAddr],
    dir: Option<PathBuf>,
    yes: bool,
    port: u16,
    udp_port: u16,
    show_qr: bool,
) -> Result<()> {
    let dir = match dir {
        Some(d) => d,
        None => default_receive_dir()?,
    };
    let listener = bind_receiver(port)?;
    let port = listener.local_addr()?.port();
    let discovery = start_discovery_on(&local.device, port, seeds, udp_port)?;
    println!(
        "🐰 Hoppy is receiving as \"{}\" on port {port}",
        local.device.name
    );
    println!("   Files go to {}", dir.display());
    println!("   This device's key: {}", local.fingerprint());
    if yes {
        println!("   ⚠ Auto-accept is ON: anyone on this network can send you files.");
    }
    if show_qr {
        // TODO(phase 2): render it as a QR code (desktop GUI); the text works with
        // `hoppy trust verify --qr` on another computer.
        println!("   QR verification code: {}", local.begin_qr());
    }
    let handler = CliReceiver {
        auto_accept: yes,
        asking: AtomicBool::new(false),
    };
    receive_forever(listener, &discovery, dir, local.clone(), Arc::new(handler))
}

/// A remembered device by id, or by name (case-insensitive) when that is unambiguous.
fn find_trusted(local: &Local, wanted: &str) -> Result<PeerKey> {
    let store = local.trust.snapshot();
    let w = wanted.to_lowercase();
    let mut found: Vec<PeerKey> = store
        .peers
        .iter()
        .filter(|(id, e)| *id == wanted || e.name.to_lowercase() == w)
        .map(|(id, e)| PeerKey {
            id: id.clone(),
            name: e.name.clone(),
            platform: String::new(),
            fingerprint: e.fingerprint.clone(),
        })
        .collect();
    match found.len() {
        0 => bail!("no remembered device called {wanted:?} (see `hoppy trust list`)"),
        1 => Ok(found.remove(0)),
        _ => bail!("several devices are called {wanted:?}: use the id (see `hoppy trust list`)"),
    }
}

/// Groups a fingerprint in blocks of 4 for reading aloud / comparing by eye.
fn grouped(fp: &str) -> String {
    fp.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn cmd_trust(local: &Local, seeds: &[SocketAddr], action: Option<TrustAction>) -> Result<()> {
    match action.unwrap_or(TrustAction::List) {
        TrustAction::List => {
            println!("This device: {}", local.device.name);
            println!("  key {}", grouped(&local.fingerprint()));
            let store = local.trust.snapshot();
            if store.peers.is_empty() {
                println!("No remembered devices yet.");
            }
            for (id, e) in &store.peers {
                println!(
                    "{} {:<24} {}  {}",
                    if e.verified { "✓" } else { "·" },
                    e.name,
                    id,
                    if e.verified {
                        "verified"
                    } else {
                        "not verified"
                    }
                );
                println!("  key {}", grouped(&e.fingerprint));
            }
            Ok(())
        }
        TrustAction::Forget { device } => {
            let peer = find_trusted(local, &device)?;
            local.trust.forget(&peer.id)?;
            println!("Forgot {} ({}).", peer.name, peer.id);
            Ok(())
        }
        TrustAction::Verify {
            device,
            qr: Some(payload),
            to,
            ..
        } => {
            let (qr, peer_key) = match local.trust.check_qr(&payload, None) {
                Ok(v) => v,
                Err(Error::VerificationFailed(why)) => bail!("✗ NOT verified: {why}"),
                Err(e) => return Err(e.into()),
            };
            if let Some(d) = device
                && d != qr.id
                && !d.eq_ignore_ascii_case(&peer_key.name)
            {
                bail!("the code belongs to {}, not {d}", peer_key.name);
            }
            println!("✓ {} is verified on this device.", peer_key.name);
            confirm_on_peer(local, seeds, &qr, &peer_key, to.as_deref())
        }
        TrustAction::Verify {
            device,
            qr: None,
            yes,
            ..
        } => {
            let device = device.context("name the device to verify, or pass --qr")?;
            let peer = find_trusted(local, &device)?;
            if peer.fingerprint.is_empty() {
                bail!("no key remembered for {}", peer.name);
            }
            println!(
                "Check that `hoppy trust list` on {} shows this key for itself:\n  {}",
                peer.name,
                grouped(&peer.fingerprint)
            );
            if !yes {
                print!("Is it exactly the same? [y/N] ");
                let _ = io::stdout().flush();
                let mut line = String::new();
                io::stdin().lock().read_line(&mut line)?;
                if !line.trim().eq_ignore_ascii_case("y") {
                    bail!("not verified");
                }
            }
            local.trust.mark_verified(&peer)?;
            println!("✓ {} is verified.", peer.name);
            Ok(())
        }
    }
}

/// Tells the device that showed the QR code that we scanned it (spec §7.7).
fn confirm_on_peer(
    local: &Local,
    seeds: &[SocketAddr],
    qr: &QrPayload,
    who: &PeerKey,
    to: Option<&str>,
) -> Result<()> {
    let mut discovery = None;
    let peer = match to {
        Some(addr) => {
            let addr = parse_direct(addr).with_context(|| format!("invalid address {addr:?}"))?;
            Peer {
                device: DeviceInfo {
                    id: qr.id.clone(),
                    name: who.name.clone(),
                    platform: String::new(),
                },
                addr,
                crypto: Some(1),
            }
        }
        None => {
            let d = discovery.insert(start_discovery(&local.device, 0, seeds)?);
            d.wait_for_peers(usize::MAX, Duration::from_secs(3))
                .into_iter()
                .find(|p| p.device.id == qr.id)
                .with_context(|| {
                    format!(
                        "{} is not nearby right now, so it still shows it as not verified",
                        who.name
                    )
                })?
        }
    };
    confirm_qr(&peer, local, discovery.as_ref(), qr)
        .with_context(|| format!("{} could not confirm", who.name))?;
    println!("✓ {} now shows this device as verified too.", who.name);
    Ok(())
}

struct CliReceiver {
    auto_accept: bool,
    /// One prompt at a time; offers arriving meanwhile are declined.
    asking: AtomicBool,
}

impl ReceiveHandler for CliReceiver {
    fn decide(&self, offer: &Offer, from: SocketAddr, session: &SessionInfo) -> bool {
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
        body.push_str(&format!("\n{}", security_line(session)));
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

    fn blocked(&self, from: SocketAddr, offer: &Offer, _session: &SessionInfo, reason: &str) {
        let name = display_name(&offer.sender.name);
        if reason == "key_changed" {
            println!(
                "\n⚠ {name} ({}) tried to send files, but its security key has CHANGED: \
                 declined.\n  If {name} was reinstalled, run `hoppy trust forget \"{name}\"`; \
                 otherwise someone may be pretending to be it.",
                from.ip()
            );
        } else {
            println!(
                "\n⚠ {name} ({}) tried to send without encryption, but it is a device that \
                 encrypts: declined.",
                from.ip()
            );
        }
    }

    fn verified(&self, _from: SocketAddr, peer: &PeerKey) {
        println!(
            "\n✓ {} scanned this device's code: verified.",
            display_name(&peer.name)
        );
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
