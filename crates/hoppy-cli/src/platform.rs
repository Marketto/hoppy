//! OS integration. Windows: "Send to" + startup shortcuts and a native accept dialog.
//! Other desktops: stdin prompt; shell integration is documented in docs/platforms/.

use anyhow::Result;

/// Asks the user whether to accept an incoming transfer.
pub fn ask_accept(title: &str, body: &str) -> bool {
    #[cfg(windows)]
    {
        windows::message_box_yes_no(title, body)
    }
    #[cfg(not(windows))]
    {
        use std::io::{self, BufRead, Write};
        println!("\n{title}\n{body}");
        print!("Accept? [y/N] ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        let _ = io::stdin().lock().read_line(&mut line);
        matches!(
            line.trim().to_lowercase().as_str(),
            "y" | "yes" | "s" | "si" | "sì"
        )
    }
}

pub fn install() -> Result<()> {
    #[cfg(windows)]
    {
        windows::install()
    }
    #[cfg(not(windows))]
    {
        anyhow::bail!("`hoppy install` is Windows-only for now; see docs/platforms/linux.md")
    }
}

pub fn uninstall() -> Result<()> {
    #[cfg(windows)]
    {
        windows::uninstall()
    }
    #[cfg(not(windows))]
    {
        anyhow::bail!("`hoppy uninstall` is Windows-only for now")
    }
}

#[cfg(windows)]
mod windows {
    use std::path::PathBuf;
    use std::process::Command;

    use anyhow::{Context, Result, bail};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_ICONQUESTION, MB_SETFOREGROUND, MB_SYSTEMMODAL, MB_YESNO, MessageBoxW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn message_box_yes_no(title: &str, body: &str) -> bool {
        let (title, body) = (wide(title), wide(body));
        let style = MB_YESNO | MB_ICONQUESTION | MB_SYSTEMMODAL | MB_SETFOREGROUND;
        // SAFETY: both strings are NUL-terminated UTF-16 buffers alive for the call.
        unsafe { MessageBoxW(std::ptr::null_mut(), body.as_ptr(), title.as_ptr(), style) == IDYES }
    }

    fn appdata_dir(sub: &str) -> Result<PathBuf> {
        let base = std::env::var_os("APPDATA").context("APPDATA is not set")?;
        Ok(PathBuf::from(base).join(sub))
    }

    fn send_to_link() -> Result<PathBuf> {
        Ok(appdata_dir(r"Microsoft\Windows\SendTo")?.join("Hoppy.lnk"))
    }

    fn startup_link() -> Result<PathBuf> {
        Ok(
            appdata_dir(r"Microsoft\Windows\Start Menu\Programs\Startup")?
                .join("Hoppy receiver.lnk"),
        )
    }

    /// Creates a .lnk through WScript.Shell. Values travel in env vars to avoid quoting bugs.
    fn create_shortcut(link: &std::path::Path, args: &str, minimized: bool) -> Result<()> {
        let exe = std::env::current_exe()?;
        let script = "$s=(New-Object -ComObject WScript.Shell).CreateShortcut($env:HOPPY_LNK);\
            $s.TargetPath=$env:HOPPY_EXE;$s.Arguments=$env:HOPPY_ARGS;$s.IconLocation=$env:HOPPY_EXE;\
            $s.WindowStyle=[int]$env:HOPPY_STYLE;$s.Description='Hoppy';$s.Save()";
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("HOPPY_LNK", link)
            .env("HOPPY_EXE", &exe)
            .env("HOPPY_ARGS", args)
            .env("HOPPY_STYLE", if minimized { "7" } else { "1" })
            .status()
            .context("running powershell")?;
        if !status.success() {
            bail!("could not create {}", link.display());
        }
        Ok(())
    }

    pub fn install() -> Result<()> {
        let send_to = send_to_link()?;
        create_shortcut(&send_to, "send --pause", false)?;
        println!("✓ Added \"Send to → Hoppy\" ({})", send_to.display());
        let startup = startup_link()?;
        create_shortcut(&startup, "receive", true)?;
        println!(
            "✓ Receiver starts minimized at login ({})",
            startup.display()
        );
        println!("Start it now with: hoppy receive");
        println!(
            "Windows may ask once to allow Hoppy through the firewall: choose Private networks."
        );
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        for link in [send_to_link()?, startup_link()?] {
            if link.exists() {
                std::fs::remove_file(&link)?;
                println!("✓ Removed {}", link.display());
            }
        }
        Ok(())
    }
}
