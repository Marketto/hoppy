//! `hoppyw`: the desktop app (Windows: GUI-subsystem exe, no console window ever; Linux:
//! panel icon + notifications + zenity/kdialog dialogs; macOS: the menu-bar app inside
//! Hoppy.app).
//! `hoppyw` = tray receiver; `hoppyw send FILE…` = the "Send with Hoppy" target;
//! `hoppyw open [FILE…]` = the Linux launcher (files: send them, none: the receiver).
//! All logic lives in `hoppy::gui`; the CLI stays in `hoppy` (src/main.rs).
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // CI (macos job): exercises the AppKit backend without a person, then exits.
    #[cfg(target_os = "macos")]
    if args.len() == 1 && args[0] == "--smoke-test" {
        std::process::exit(hoppy::macos::ui::smoke_test());
    }
    if cfg!(any(windows, target_os = "linux", target_os = "macos")) {
        std::process::exit(hoppy::gui::run(args));
    }
    eprintln!("hoppyw is the desktop app for Windows, Linux and macOS; here use the `hoppy` CLI.");
    std::process::exit(1);
}
