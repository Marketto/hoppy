//! Helpers shared by the Linux (`crate::linux`) and macOS (`crate::macos`) desktop
//! backends: `PATH` lookup, the curl/wget HTTPS client of the update check, and the
//! installer of downloaded updates (`selfupdate`).

pub mod http;
pub mod selfupdate;

use std::path::{Path, PathBuf};

/// First executable called `name` on `PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_sh() {
        assert!(which("sh").is_some());
        assert!(which("surely-not-a-command-hoppy").is_none());
    }
}
