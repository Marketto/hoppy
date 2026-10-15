//! "Open at login" and the per-user setup of the Mac app.
//!
//! - macOS 13+: `SMAppService.mainApp` (System Settings → General → Login Items lists
//!   "Hoppy"). If the system refuses it (e.g. it does not accept the ad-hoc signature), the
//!   LaunchAgent below is used instead.
//! - macOS 12: `~/Library/LaunchAgents/app.hoppy.desktop.login.plist` (`open -a Hoppy.app`),
//!   carrying `super::MARKER`; a file without it is never touched.
//!
//! Only for a copy running from an app bundle that stays where it is: not from the mounted
//! .dmg and not App-Translocated (Gatekeeper's temporary copy of an app opened from
//! Downloads), where a login item would point at a path that disappears.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

use super::{
    Location, bundle_location, bundle_of, is_our_file, launch_agent_path, launch_agent_plist,
    write_marked_file,
};
use crate::config;
use crate::update::CURRENT_VERSION;

/// Value of `config::Identity::desktop_integration` after `hoppy uninstall`.
pub const OFF: &str = "off";

/// Running on macOS `major` or later.
pub(super) fn at_least(major: isize) -> bool {
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: major,
        minorVersion: 0,
        patchVersion: 0,
    })
}

/// SMAppService exists (macOS 13+; the class is missing on 12, so never touch it there).
fn modern() -> bool {
    at_least(13)
}

/// The running app bundle, if any.
fn bundle() -> Option<PathBuf> {
    std::env::current_exe().ok().as_deref().and_then(bundle_of)
}

fn agent_path() -> Result<PathBuf> {
    Ok(launch_agent_path(
        &dirs::home_dir().context("no home folder")?,
    ))
}

fn agent_is_ours() -> bool {
    agent_path().is_ok_and(|p| is_our_file(&p))
}

/// `Some(on)` when running from Hoppy.app, `None` otherwise (no menu entry then).
pub fn enabled() -> Option<bool> {
    bundle()?;
    if modern() {
        // SAFETY: SMAppService exists on macOS 13+.
        let status = unsafe { SMAppService::mainAppService().status() };
        if status == SMAppServiceStatus::Enabled || status == SMAppServiceStatus::RequiresApproval {
            return Some(true);
        }
    }
    Some(agent_is_ours())
}

/// Turns "Open at login" on or off.
pub fn set(on: bool) -> Result<()> {
    let bundle = bundle().context("Hoppy is not running from Hoppy.app")?;
    if on {
        match bundle_location(&bundle) {
            Location::Installed => {}
            Location::Translocated | Location::DiskImage => bail!(
                "{}",
                if super::ui::ui_is_italian() {
                    "Sposta prima Hoppy nella cartella Applicazioni, poi riaprilo."
                } else {
                    "Move Hoppy to the Applications folder first, then open it again."
                }
            ),
        }
    }
    if modern() {
        // SAFETY: SMAppService exists on macOS 13+.
        let service = unsafe { SMAppService::mainAppService() };
        let result = if on {
            // SAFETY: as above.
            unsafe { service.registerAndReturnError() }
        } else {
            // SAFETY: as above.
            unsafe { service.unregisterAndReturnError() }
        };
        match result {
            Ok(()) if on => {
                remove_agent()?; // a LaunchAgent from macOS 12 would start a second copy
                // SAFETY: as above.
                if unsafe { service.status() } == SMAppServiceStatus::RequiresApproval {
                    // SAFETY: as above.
                    unsafe { SMAppService::openSystemSettingsLoginItems() };
                }
                return Ok(());
            }
            Ok(()) => {}
            Err(e) if on => {
                crate::diag::log(format!(
                    "login: SMAppService refused (error {}), using a LaunchAgent",
                    e.code()
                ));
            }
            // Not registered (only the LaunchAgent, or nothing): fine.
            Err(_) => {}
        }
    }
    if on {
        write_agent(&bundle)
    } else {
        remove_agent()
    }
}

fn write_agent(bundle: &std::path::Path) -> Result<()> {
    write_marked_file(&agent_path()?, &launch_agent_plist(bundle))
}

fn remove_agent() -> Result<()> {
    let path = agent_path()?;
    if agent_is_ours() {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// Asks macOS to re-read the Finder services of installed apps (after an install/update).
fn refresh_services() {
    objc2_app_kit::NSUpdateDynamicServices();
}

/// `hoppy install`: "Open at login" on, Finder services refreshed.
pub fn install() -> Result<()> {
    set(true)?;
    refresh_services();
    config::set_desktop_integration(CURRENT_VERSION)?;
    println!("✓ Hoppy opens at login");
    println!("✓ Finder: right-click → Services → \"Send with Hoppy\"");
    Ok(())
}

/// `hoppy uninstall`: "Open at login" off; the tray then never turns it on by itself.
pub fn uninstall() -> Result<()> {
    set(false)?;
    config::set_desktop_integration(OFF)?;
    println!("✓ Hoppy no longer opens at login");
    Ok(())
}

/// Tray start, once per version (release builds): refresh the Finder services; on the very
/// first run also turn "Open at login" on (like the Windows startup shortcut and Linux
/// autostart). Failures are logged, not fatal.
pub fn first_run() -> Result<config::Identity> {
    let identity = config::load_or_create()?;
    if identity.desktop_integration == CURRENT_VERSION || identity.desktop_integration == OFF {
        return Ok(identity);
    }
    refresh_services();
    if identity.desktop_integration.is_empty() {
        match bundle().map(|b| bundle_location(&b)) {
            Some(Location::Installed) => {
                if let Err(e) = set(true) {
                    crate::diag::log(format!("login: not turned on: {e:#}"));
                }
            }
            // Try again on a later start, once Hoppy has been moved to Applications.
            Some(_) => return Ok(identity),
            None => {}
        }
    }
    config::set_desktop_integration(CURRENT_VERSION)
}
