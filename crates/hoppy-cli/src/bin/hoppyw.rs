//! `hoppyw`: the desktop app (Windows: GUI-subsystem exe, no console window ever; Linux:
//! panel icon + notifications + zenity/kdialog dialogs).
//! `hoppyw` = tray receiver; `hoppyw send FILE…` = the "Send with Hoppy" target;
//! `hoppyw open [FILE…]` = the Linux launcher (files: send them, none: the receiver).
//! All logic lives in `hoppy::gui`; the CLI stays in `hoppy` (src/main.rs).
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

fn main() {
    if cfg!(any(windows, target_os = "linux")) {
        std::process::exit(hoppy::gui::run(std::env::args_os().skip(1).collect()));
    }
    eprintln!("hoppyw is the Windows/Linux desktop app; on this system use the `hoppy` CLI.");
    std::process::exit(1);
}
