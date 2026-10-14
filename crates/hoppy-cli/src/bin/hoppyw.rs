//! `hoppyw.exe`: Windows GUI-subsystem binary (no console window ever).
//! `hoppyw` = tray receiver; `hoppyw send FILE…` = the "Send to → Hoppy" target.
//! All logic lives in `hoppy::gui`; the CLI stays in `hoppy` (src/main.rs).
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

fn main() {
    if cfg!(windows) {
        std::process::exit(hoppy::gui::run(std::env::args_os().skip(1).collect()));
    }
    eprintln!("hoppyw is the Windows tray app; on this system use the `hoppy` CLI.");
    std::process::exit(1);
}
