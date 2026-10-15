//! Windows Defender Firewall rules for Hoppy (`hoppy firewall`, the tray hint, the installer).
//!
//! Hoppy is installed per user without admin rights, so the only automatic way through the
//! firewall is the Windows prompt on the first receive. If that prompt was dismissed (which
//! creates *block* rules) or the network is Public, phones find the PC but cannot connect.
//!
//! - `apply`/`remove` run `netsh` (they need an elevated process: `hoppy firewall --fix`, the
//!   tray and the installer start `hoppyw firewall --apply` through UAC). Rules, both inbound
//!   allow, tied to the program path, profiles Private + Domain (never Public, v0 is
//!   unencrypted): `Hoppy (hoppyw)` and `Hoppy (hoppy)`, each TCP 47471 + UDP 47470. Applying
//!   first deletes every inbound rule for the same program paths (ours from a previous run,
//!   and the prompt's block rules, which would win over an allow rule), so it is idempotent.
//!   Deletion always filters by program path, never by name alone: another Windows user of
//!   the same PC has rules with the same names for their own per-user install.
//! - `check` reads the effective rules + network categories through PowerShell (no admin
//!   needed); `Status::verdict` turns that into a hint, `check_code` into the exit code of
//!   `hoppyw firewall --check` (used by the installer). Pure parts are unit-tested on Linux.
//!
//! Values reach PowerShell through environment variables only; netsh gets plain arguments.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};
use hoppy_core::protocol::{DISCOVERY_PORT, TRANSFER_PORT};

use crate::platform;

/// (rule name, program file name). Names are a contract with `apps/windows/hoppy.iss`.
pub const RULES: [(&str, &str); 2] = [
    ("Hoppy (hoppyw)", "hoppyw.exe"),
    ("Hoppy (hoppy)", "hoppy.exe"),
];
/// Never Public: v0 is unencrypted (ADR 0003), so cafés/hotels stay closed.
pub const PROFILES: &str = "private,domain";
const DESCRIPTION: &str = "Hoppy: receive files from nearby devices";

fn kv(key: &str, value: impl AsRef<OsStr>) -> OsString {
    let mut s = OsString::from(key);
    s.push("=");
    s.push(value);
    s
}

fn netsh_rule(verb: &str, rest: Vec<OsString>) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["advfirewall", "firewall", verb, "rule"]
        .into_iter()
        .map(OsString::from)
        .collect();
    args.extend(rest);
    args
}

/// netsh argument lists that delete every inbound rule for `programs`: ours, and e.g. the
/// block rules left by a dismissed Windows prompt. "No rules match" is a normal outcome.
pub fn remove_commands(programs: &[PathBuf]) -> Vec<Vec<OsString>> {
    programs
        .iter()
        .map(|exe| {
            netsh_rule(
                "delete",
                vec![kv("name", "all"), kv("dir", "in"), kv("program", exe)],
            )
        })
        .collect()
}

/// netsh argument lists adding the allow rules for every program of `RULES` found in `dir`.
pub fn add_commands(dir: &Path, exists: impl Fn(&Path) -> bool) -> Vec<Vec<OsString>> {
    let mut cmds = Vec::new();
    for (name, file) in RULES {
        let exe = dir.join(file);
        if !exists(&exe) {
            continue;
        }
        for (protocol, port) in [("TCP", TRANSFER_PORT), ("UDP", DISCOVERY_PORT)] {
            cmds.push(netsh_rule(
                "add",
                vec![
                    kv("name", name),
                    kv("dir", "in"),
                    kv("action", "allow"),
                    kv("program", &exe),
                    kv("enable", "yes"),
                    kv("profile", PROFILES),
                    kv("protocol", protocol),
                    kv("localport", port.to_string()),
                    kv("description", DESCRIPTION),
                ],
            ));
        }
    }
    cmds
}

/// Programs of `RULES` present in `dir`.
pub fn programs_in(dir: &Path) -> Vec<PathBuf> {
    RULES
        .iter()
        .map(|(_, file)| dir.join(file))
        .filter(|p| p.is_file())
        .collect()
}

fn netsh() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(|r| PathBuf::from(r).join(r"System32\netsh.exe"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("netsh"))
}

fn run_netsh(args: &[OsString]) -> Result<bool> {
    let mut cmd = Command::new(netsh());
    cmd.args(args);
    platform::no_window(&mut cmd);
    Ok(cmd.output()?.status.success())
}

/// Replaces the rules for the programs in `dir`. Needs an elevated process.
pub fn apply(dir: &Path) -> Result<()> {
    let programs = programs_in(dir);
    if programs.is_empty() {
        bail!("no hoppy.exe / hoppyw.exe in {}", dir.display());
    }
    for args in remove_commands(&programs) {
        let _ = run_netsh(&args)?;
    }
    for args in add_commands(dir, Path::is_file) {
        if !run_netsh(&args)? {
            bail!("netsh could not add the firewall rules (not running as administrator?)");
        }
    }
    Ok(())
}

/// Deletes the inbound rules for the programs of `RULES` in `dir`. Needs elevation.
pub fn remove(dir: &Path) -> Result<()> {
    let programs: Vec<PathBuf> = RULES.iter().map(|(_, file)| dir.join(file)).collect();
    for args in remove_commands(&programs) {
        let _ = run_netsh(&args)?;
    }
    Ok(())
}

/// Lists the enabled inbound rules (effective store) whose program is one of
/// `$env:HOPPY_EXES` ('|'-separated, a character Windows paths cannot contain) and the
/// category of each connected network. Ends with `done`, or `error|…`.
pub const STATUS_SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
try {
  $exes = $env:HOPPY_EXES.Split('|')
  $filters = @(Get-NetFirewallApplicationFilter -PolicyStore ActiveStore)
  for ($i = 0; $i -lt $exes.Count; $i++) {
    foreach ($f in $filters) {
      if ([Environment]::ExpandEnvironmentVariables([string]$f.Program) -eq $exes[$i]) {
        foreach ($r in @($f | Get-NetFirewallRule)) {
          if ([string]$r.Direction -eq 'Inbound' -and [string]$r.Enabled -eq 'True') {
            'rule|' + $i + '|' + $r.Action + '|' + $r.Profile
          }
        }
      }
    }
  }
  foreach ($p in @(Get-NetConnectionProfile -ErrorAction SilentlyContinue)) {
    'net|' + $p.NetworkCategory
  }
  'done'
} catch { 'error|' + $_ }
"#;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Profiles {
    pub domain: bool,
    pub private: bool,
    pub public: bool,
}

impl Profiles {
    /// `Profile` of a firewall rule: "Any", "Domain, Private", …
    pub fn parse(s: &str) -> Profiles {
        let mut p = Profiles::default();
        for part in s.split(',').map(str::trim) {
            match part {
                "Any" => {
                    return Profiles {
                        domain: true,
                        private: true,
                        public: true,
                    };
                }
                "Domain" => p.domain = true,
                "Private" => p.private = true,
                "Public" => p.public = true,
                _ => {}
            }
        }
        p
    }

    fn covers(self, net: Network) -> bool {
        match net {
            Network::Domain => self.domain,
            Network::Private => self.private,
            Network::Public => self.public,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    Domain,
    Private,
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Index into the programs passed to `check`.
    pub program: usize,
    pub allow: bool,
    pub profiles: Profiles,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub rules: Vec<Rule>,
    /// Categories of the connected networks.
    pub networks: Vec<Network>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Reachable on at least one connected network (or nothing is connected).
    Ok,
    /// Only Public networks, where Hoppy is not allowed: set the network to Private.
    PublicNetwork,
    /// A Private/Domain network where the program is blocked or not allowed: add the rules.
    NeedsRules,
}

impl Status {
    /// `None` unless the script reached `done`: a failed check never produces a hint.
    pub fn parse(out: &str) -> Option<Status> {
        let mut status = Status::default();
        let mut done = false;
        for line in out.lines().map(str::trim) {
            let parts: Vec<&str> = line.split('|').collect();
            match parts.as_slice() {
                ["rule", program, action, profile] => {
                    let Ok(program) = program.parse() else {
                        continue;
                    };
                    status.rules.push(Rule {
                        program,
                        allow: *action == "Allow",
                        profiles: Profiles::parse(profile),
                    });
                }
                ["net", category] => status.networks.push(match *category {
                    "DomainAuthenticated" => Network::Domain,
                    "Private" => Network::Private,
                    _ => Network::Public,
                }),
                ["done"] => done = true,
                _ => {}
            }
        }
        done.then_some(status)
    }

    /// Is `program` (an index given to `check`) allowed in on networks of category `net`?
    /// Block rules win over allow rules, like in Windows.
    pub fn reachable(&self, program: usize, net: Network) -> bool {
        let rules = || {
            self.rules
                .iter()
                .filter(move |r| r.program == program && r.profiles.covers(net))
        };
        rules().any(|r| r.allow) && !rules().any(|r| !r.allow)
    }

    /// Can other devices reach `program` on the connected networks?
    pub fn verdict(&self, program: usize) -> Verdict {
        if self.networks.is_empty() {
            return Verdict::Ok;
        }
        if self.networks.iter().any(|&n| self.reachable(program, n)) {
            Verdict::Ok
        } else if self.networks.iter().all(|&n| n == Network::Public) {
            Verdict::PublicNetwork
        } else {
            Verdict::NeedsRules
        }
    }
}

/// Exit code of `hoppyw firewall --check` for `check(&[hoppyw.exe, hoppy.exe…])`, read by
/// the installer: 0 = hoppyw is allowed on Private networks (nothing to do), 1 = no rule
/// for these programs, 2 = some rules but not allowed (e.g. the prompt's block rules),
/// 3 = the state could not be read.
pub fn check_code(status: Option<&Status>) -> i32 {
    match status {
        None => 3,
        Some(s) if s.reachable(0, Network::Private) => 0,
        Some(s) if s.rules.is_empty() => 1,
        Some(_) => 2,
    }
}

/// Reads the firewall state for `programs` (no admin rights needed). `None` on any error.
pub fn check(programs: &[PathBuf]) -> Option<Status> {
    if !cfg!(windows) {
        return None; // Windows Firewall only (no PowerShell elsewhere)
    }
    let mut joined = OsString::new();
    for (i, p) in programs.iter().enumerate() {
        if i > 0 {
            joined.push("|");
        }
        joined.push(p);
    }
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        STATUS_SCRIPT,
    ])
    .env("HOPPY_EXES", joined);
    platform::no_window(&mut cmd);
    let out = cmd.output().ok()?;
    Status::parse(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn add_commands_cover_both_programs_and_protocols() {
        let dir = Path::new("/opt/Hoppy Mario");
        let cmds = add_commands(dir, |_| true);
        assert_eq!(cmds.len(), 4);
        let first = strs(&cmds[0]);
        assert_eq!(&first[..4], ["advfirewall", "firewall", "add", "rule"]);
        assert!(first.contains(&"name=Hoppy (hoppyw)".to_owned()));
        assert!(first.contains(&format!("program={}", dir.join("hoppyw.exe").display())));
        assert!(first.contains(&"profile=private,domain".to_owned()));
        assert!(first.contains(&"protocol=TCP".to_owned()));
        assert!(first.contains(&"localport=47471".to_owned()));
        assert!(strs(&cmds[1]).contains(&"localport=47470".to_owned()));
        assert!(strs(&cmds[2]).contains(&"name=Hoppy (hoppy)".to_owned()));
        // Missing programs get no rule.
        let only_gui = add_commands(dir, |p| p.ends_with("hoppyw.exe"));
        assert_eq!(only_gui.len(), 2);
    }

    #[test]
    fn remove_commands_filter_by_program() {
        let exe = PathBuf::from("/x/hoppyw.exe");
        let cmds = remove_commands(std::slice::from_ref(&exe));
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            strs(&cmds[0])[4..],
            ["name=all", "dir=in", "program=/x/hoppyw.exe"]
        );
    }

    #[test]
    fn parses_profiles() {
        assert_eq!(
            Profiles::parse("Domain, Private"),
            Profiles {
                domain: true,
                private: true,
                public: false
            }
        );
        assert_eq!(
            Profiles::parse("Any"),
            Profiles::parse("Domain, Private, Public")
        );
        assert!(!Profiles::parse("Public").private);
    }

    #[test]
    fn status_needs_done() {
        assert_eq!(Status::parse("rule|0|Allow|Any\nnet|Private\n"), None);
        assert_eq!(Status::parse("error|Access denied"), None);
        let s = Status::parse("rule|0|Allow|Any\r\nnet|Private\r\ndone\r\n").unwrap();
        assert_eq!(s.networks, [Network::Private]);
        assert_eq!(s.verdict(0), Verdict::Ok);
    }

    #[test]
    fn check_codes() {
        let code = |out: &str| check_code(Status::parse(out).as_ref());
        assert_eq!(code("error|x"), 3);
        assert_eq!(code("net|Public\ndone"), 1);
        assert_eq!(code("rule|0|Allow|Domain, Private\nnet|Public\ndone"), 0);
        assert_eq!(code("rule|0|Block|Private\nrule|0|Allow|Private\ndone"), 2);
        assert_eq!(code("rule|1|Allow|Private\ndone"), 2);
    }

    #[test]
    fn verdicts() {
        let v = |out: &str| Status::parse(out).unwrap().verdict(0);
        // Nothing connected: nothing to say.
        assert_eq!(v("done"), Verdict::Ok);
        // No rule at all on a private network.
        assert_eq!(v("net|Private\ndone"), Verdict::NeedsRules);
        // Rule for the other program only.
        assert_eq!(
            v("rule|1|Allow|Any\nnet|Private\ndone"),
            Verdict::NeedsRules
        );
        // Our rules on a domain (corporate) network.
        assert_eq!(
            v("rule|0|Allow|Domain, Private\nnet|DomainAuthenticated\ndone"),
            Verdict::Ok
        );
        // The dismissed prompt's block rule wins.
        assert_eq!(
            v("rule|0|Allow|Private\nrule|0|Block|Private\nnet|Private\ndone"),
            Verdict::NeedsRules
        );
        // Public Wi-Fi with our (Private/Domain) rules: change the network category.
        assert_eq!(
            v("rule|0|Allow|Domain, Private\nnet|Public\ndone"),
            Verdict::PublicNetwork
        );
        // …unless the user allowed Public in the Windows prompt.
        assert_eq!(v("rule|0|Allow|Public\nnet|Public\ndone"), Verdict::Ok);
        // Two adapters, one fine.
        assert_eq!(
            v("rule|0|Allow|Private\nnet|Public\nnet|Private\ndone"),
            Verdict::Ok
        );
    }
}
