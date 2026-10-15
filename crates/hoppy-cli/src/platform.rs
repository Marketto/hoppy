//! OS integration. Windows: "Send to" + startup shortcuts, the "Send with Hoppy" Explorer
//! verb (+ optional classic Windows 11 context menu), native dialogs and the
//! notification-area (tray) icon used by `hoppyw`. Linux: the same UI on freedesktop
//! pieces (`crate::linux`), per-user right-click entries and autostart. macOS: the same UI
//! on AppKit (`crate::macos`): menu-bar icon, Finder service, login item.
//!
//! All Win32 FFI lives in `win` below behind `#[cfg(windows)]`; only the windows-latest CI
//! job compiles it. `ui` is what `gui.rs` uses: `win` on Windows, `crate::linux::ui` or
//! `crate::macos::ui` (same safe API) elsewhere, so `gui.rs` is compiled, linted and partly
//! run on Linux too.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;

#[cfg(windows)]
pub use win as ui;

/// The non-Windows desktop backend: `install`, `uninstall`, `first_run_integration`,
/// `ui_is_italian` and `ui` with the same names on Linux and macOS.
#[cfg(not(windows))]
#[cfg(not(target_os = "macos"))]
use crate::linux as desktop;
#[cfg(target_os = "macos")]
use crate::macos as desktop;

#[cfg(not(windows))]
pub use desktop::ui;

/// Asks the user whether to accept an incoming transfer (the `hoppy` CLI: a dialog on
/// Windows, a terminal prompt elsewhere).
pub fn ask_accept(title: &str, body: &str) -> bool {
    #[cfg(windows)]
    {
        win::message_box_yes_no(title, body)
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

/// UI language for `hoppyw` strings: the Windows display language (Linux: the message
/// locale; macOS: the first preferred language).
pub fn ui_is_italian() -> bool {
    #[cfg(windows)]
    {
        win::ui_language_is_italian()
    }
    #[cfg(not(windows))]
    {
        desktop::ui_is_italian()
    }
}

/// Windows: Send To + verb + startup shortcut. Linux: right-click entries (Nautilus, Nemo,
/// Dolphin), launcher and autostart for this user (`linux::integration`). macOS: login
/// item + Services refresh (the Finder service itself is declared in Info.plist).
pub fn install() -> Result<()> {
    #[cfg(windows)]
    {
        win::install()
    }
    #[cfg(not(windows))]
    {
        desktop::install()
    }
}

pub fn uninstall() -> Result<()> {
    #[cfg(windows)]
    {
        win::uninstall()
    }
    #[cfg(not(windows))]
    {
        desktop::uninstall()
    }
}

/// Tray start: the per-user desktop integration, once per version (Linux, macOS; nothing
/// to do on Windows, where the installer does it). Returns the settings as saved.
pub fn first_run_integration() -> Result<crate::config::Identity> {
    #[cfg(windows)]
    {
        crate::config::load_or_create()
    }
    #[cfg(not(windows))]
    {
        desktop::first_run_integration()
    }
}

/// "Open at login" (macOS login item): `Some(on)`, or `None` where the tray menu has no
/// such switch (Windows and Linux use a startup shortcut / autostart file instead, and a
/// macOS copy not running from an app bundle).
pub fn login_item() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::login::enabled()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Turns "Open at login" on/off (macOS).
pub fn set_login_item(on: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::login::set(on)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = on;
        anyhow::bail!("\"Open at login\" is a macOS setting")
    }
}

/// This copy can install updates itself: Windows when installed by Hoppy-Setup
/// (`%LOCALAPPDATA%\Programs\Hoppy`), macOS for `/Applications/Hoppy.app` (or
/// `~/Applications`) in a folder this user can write, Linux when running from the .deb.
/// Others only get the portal link. May touch the disk: call it before the event loop.
pub fn can_self_update() -> bool {
    #[cfg(windows)]
    {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.to_string_lossy().into_owned()));
        let local = std::env::var_os("LOCALAPPDATA").map(|l| l.to_string_lossy().into_owned());
        matches!((exe_dir, local), (Some(dir), Some(local)) if crate::update::is_setup_install(&dir, &local))
    }
    #[cfg(not(windows))]
    {
        crate::unix::selfupdate::target().is_some()
    }
}

/// Worker thread: downloads `release`'s package for this platform into a fresh private
/// temporary folder and checks it against `SHA256SUMS.txt`. Windows: returns the installer
/// to run. macOS/Linux: also installs it (`installing` is called when that starts).
pub fn fetch_update(
    http: &dyn crate::update::Http,
    release: &crate::update::Release,
    progress: &mut dyn FnMut(u64),
    installing: &mut dyn FnMut(),
) -> Result<crate::update::UpdateReady> {
    let temp = std::env::temp_dir();
    #[cfg(windows)]
    {
        let _ = installing;
        crate::update::download_fresh(http, release, &temp, progress)
            .map(crate::update::UpdateReady::RunInstaller)
    }
    #[cfg(not(windows))]
    {
        crate::unix::selfupdate::fetch_and_install(http, release, &temp, progress, installing)
    }
}

/// macOS/Linux, after `fetch_update` installed a new version: starts it once this process
/// has exited (the caller exits right after).
pub fn relaunch_after_exit() -> Result<()> {
    #[cfg(windows)]
    {
        anyhow::bail!("the Windows installer restarts Hoppy itself")
    }
    #[cfg(not(windows))]
    {
        crate::unix::selfupdate::relaunch_after_exit()
    }
}

/// Tray start (worker): removes what an interrupted update left next to the Mac app
/// (Windows and Linux keep everything in the temporary folder, cleaned by
/// `update::remove_stale_update_dirs`).
pub fn remove_update_leftovers() {
    #[cfg(not(windows))]
    crate::unix::selfupdate::remove_app_leftovers();
}

/// Label of the Explorer verb (`HKCU\Software\Classes\*\shell\Hoppy`), in the UI language
/// at install time.
pub fn verb_label() -> &'static str {
    if ui_is_italian() {
        "Invia con Hoppy"
    } else {
        "Send with Hoppy"
    }
}

/// Command line of the "Send with Hoppy" verb. With `MultiSelectModel=Player` Explorer runs
/// it once per selected file; `hoppyw send` merges them (`gather_send_files`).
pub fn verb_command(gui: &Path) -> OsString {
    let mut cmd = OsString::from("\"");
    cmd.push(gui);
    cmd.push("\" send \"%1\"");
    cmd
}

/// `hoppyw send` collector: stop this long after the last file arrived…
pub const COLLECT_QUIET: Duration = Duration::from_millis(500);
/// …and never wait longer than this in total.
pub const COLLECT_MAX: Duration = Duration::from_secs(3);

/// How much longer the collector waits for other instances; `None` = done.
pub fn collect_wait(elapsed: Duration, since_last: Duration) -> Option<Duration> {
    if elapsed >= COLLECT_MAX || since_last >= COLLECT_QUIET {
        return None;
    }
    Some((COLLECT_QUIET - since_last).min(COLLECT_MAX - elapsed))
}

/// Items of a NUL-separated UTF-16 list (the WM_COPYDATA payload); empty items skipped.
pub fn split_wide_list(buf: &[u16]) -> Vec<&[u16]> {
    buf.split(|&c| c == 0).filter(|p| !p.is_empty()).collect()
}

/// Removes repeated paths, keeping the first occurrence (Explorer may pass a file twice).
pub fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    paths
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Most files one send may gather: the protocol's offer limit.
pub const COLLECT_MAX_FILES: usize = hoppy_core::protocol::MAX_FILES;
/// Most UTF-16 payload bytes the collector accepts in total, over all forwarders.
pub const COLLECT_MAX_BYTES: usize = 16 * 1024 * 1024;

/// UTF-16LE bytes → code units (the WM_COPYDATA copy has no u16 alignment); an odd
/// trailing byte is ignored.
pub fn decode_utf16_le(bytes: &[u8]) -> Vec<u16> {
    let (pairs, _) = bytes.as_chunks::<2>();
    pairs.iter().map(|&p| u16::from_le_bytes(p)).collect()
}

/// Files gathered by the `hoppyw send` collector, within `COLLECT_MAX_FILES` /
/// `COLLECT_MAX_BYTES`; past either limit nothing more is kept and `overflow` is set.
#[derive(Debug, Default)]
pub struct Collected {
    pub files: Vec<PathBuf>,
    pub bytes: usize,
    pub overflow: bool,
}

impl Collected {
    /// Adds one forwarded payload of `bytes` bytes holding `paths`.
    pub fn add(&mut self, bytes: usize, paths: Vec<PathBuf>) {
        let bytes = self.bytes.saturating_add(bytes);
        if self.overflow
            || bytes > COLLECT_MAX_BYTES
            || self.files.len() + paths.len() > COLLECT_MAX_FILES
        {
            self.overflow = true;
            return;
        }
        self.bytes = bytes;
        self.files.extend(paths);
    }
}

/// Same executable, compared the way Windows does: case-insensitive, `\\?\` prefix ignored.
/// Paths that are not valid Unicode never match.
pub fn same_exe_path(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> Option<String> {
        let s = p.to_str()?;
        Some(s.strip_prefix(r"\\?\").unwrap_or(s).to_lowercase())
    }
    matches!((norm(a), norm(b)), (Some(a), Some(b)) if a == b)
}

/// Explorer starts one `hoppyw send FILE` per selected file. The first instance collects
/// the others' files (`COLLECT_QUIET`/`COLLECT_MAX`) and gets them all back; the others
/// forward theirs and get `None` (exit at once). Elsewhere: the files as given.
/// More than `COLLECT_MAX_FILES` files (or `COLLECT_MAX_BYTES` of paths) = `TooManyFiles`.
pub fn gather_send_files(files: Vec<PathBuf>) -> Result<Option<Vec<PathBuf>>, TooManyFiles> {
    #[cfg(windows)]
    {
        win::gather_send_files(files)
    }
    #[cfg(not(windows))]
    {
        if files.len() > COLLECT_MAX_FILES {
            return Err(TooManyFiles);
        }
        Ok(Some(files))
    }
}

/// Windows 11 classic context menu (per user): `Some(on)`, or `None` where the switch does
/// not apply (Windows 10, other systems).
pub fn classic_menu() -> Option<bool> {
    #[cfg(windows)]
    {
        win::classic_menu()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Turns the classic context menu on/off; `true` if something changed (Explorer must be
/// restarted, or the user sign in again, to see it).
pub fn set_classic_menu(on: bool) -> Result<bool> {
    #[cfg(windows)]
    {
        win::set_classic_menu(on)
    }
    #[cfg(not(windows))]
    {
        let _ = on;
        anyhow::bail!("the classic context menu is a Windows 11 setting")
    }
}

/// Restarts Explorer (taskbar + open folder windows) so a context-menu change applies.
pub fn restart_explorer() -> Result<()> {
    #[cfg(windows)]
    {
        win::restart_explorer()
    }
    #[cfg(not(windows))]
    {
        anyhow::bail!("restarting Explorer is Windows-only")
    }
}

/// Hides the console window a child process would otherwise open (`hoppyw` is a GUI
/// app, so `netsh`/`powershell` would flash a black window).
pub fn no_window(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// `run_elevated`: the user answered No to the UAC prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElevationDeclined;

impl std::fmt::Display for ElevationDeclined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("administrator permission was not granted")
    }
}

impl std::error::Error for ElevationDeclined {}

/// Something the tray window reported to the `hoppyw` event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Left/right click (or keyboard select) on the notification-area icon, with the anchor
    /// point Explorer reports (screen coordinates) for placing the menu.
    Click(Option<(i32, i32)>),
    /// The user clicked the last balloon/toast.
    BalloonClick,
    /// A worker thread called `Waker::wake`: check the shared state.
    Wake,
    /// A second `hoppyw` launch asked the running one to show itself.
    Reopen,
    /// A timer set with `Tray::set_timer` fired.
    Timer(usize),
    /// An item of the persistent tray menu (`Tray::set_menu`, Linux) or a notification
    /// button was picked.
    Menu(u32),
    /// The persistent tray menu is about to open (Linux): refresh what it shows.
    MenuOpening,
}

/// `Tray::pick_files`: the selection did not fit the dialog's buffer; `gather_send_files`:
/// more than `COLLECT_MAX_FILES` files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooManyFiles;

/// One entry of a popup menu. Ids must be non-zero (0 means "dismissed").
#[derive(Debug, Clone, Copy)]
pub enum MenuItem<'a> {
    Action(u32, &'a str),
    /// An action drawn with a check mark when `true`.
    Check(u32, &'a str, bool),
    /// An action drawn in bold (the menu's default item); at most one per menu.
    Bold(u32, &'a str),
    /// Not selectable; drawn in normal (not greyed) text for contrast.
    Label(&'a str),
    Separator,
}

/// Copies `s` into a fixed UTF-16 field (e.g. `szTip`), truncating on a character
/// boundary and always NUL-terminating.
pub fn copy_wide(dst: &mut [u16], s: &str) {
    let Some(max) = dst.len().checked_sub(1) else {
        return;
    };
    let mut n = 0;
    for ch in s.chars() {
        let mut tmp = [0u16; 2];
        let enc = ch.encode_utf16(&mut tmp);
        if n + enc.len() > max {
            break;
        }
        dst[n..n + enc.len()].copy_from_slice(enc);
        n += enc.len();
    }
    dst[n..].fill(0);
}

/// Parses a `GetOpenFileNameW` buffer with `OFN_ALLOWMULTISELECT | OFN_EXPLORER`:
/// one file → `full\path\0\0`; several → `dir\0name1\0name2\0\0`.
pub fn parse_multiselect(buf: &[u16], to_path: impl Fn(&[u16]) -> PathBuf) -> Vec<PathBuf> {
    let parts: Vec<&[u16]> = buf
        .split(|&c| c == 0)
        .take_while(|p| !p.is_empty())
        .collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [single] => vec![to_path(single)],
        [dir, names @ ..] => {
            let dir = to_path(dir);
            names.iter().map(|n| dir.join(to_path(n))).collect()
        }
    }
}

/// Receives each chunk of an HTTP body plus the Content-Length when known; an error
/// aborts the transfer.
pub type HttpSink<'a> = dyn FnMut(&[u8], Option<u64>) -> Result<()> + 'a;

/// Splits `https://host/path?query` into (`host`, `/path?query`) for WinHTTP. Only plain
/// https on the default port: no user info, no explicit port.
pub fn split_https_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = match rest.find(['/', '?', '#']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], &rest[i..]),
        Some(_) => return None,
        None => (rest, "/"),
    };
    let valid = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    valid.then_some((host, path))
}

#[cfg(windows)]
pub mod win {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result, bail};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS,
        GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WAIT_OBJECT_0, WPARAM,
    };
    use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
    use windows_sys::Win32::Graphics::Gdi::{
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_BTNFACE, CreateFontW, DEFAULT_CHARSET,
        DEFAULT_PITCH, DT_CALCRECT, DT_NOPREFIX, DT_SINGLELINE, DT_WORDBREAK, DeleteObject,
        DrawTextW, FW_NORMAL, GetDC, GetDeviceCaps, GetSysColorBrush, HFONT, LOGPIXELSY,
        OUT_DEFAULT_PRECIS, ReleaseDC, SelectObject,
    };
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    use windows_sys::Win32::Networking::WinHttp::{
        INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
        WINHTTP_QUERY_CONTENT_LENGTH, WINHTTP_QUERY_FLAG_NUMBER64, WINHTTP_QUERY_STATUS_CODE,
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
        WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    };
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD,
        REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    };
    use windows_sys::Win32::System::Threading::{
        CreateMutexW, GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, FNERR_BUFFERTOOSMALL, GetOpenFileNameW, OFN_ALLOWMULTISELECT,
        OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIIF_NOSOUND,
        NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_BALLOONUSERCLICK, NIN_SELECT,
        NOTIFYICON_VERSION_4, NOTIFYICONDATAW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC,
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, Shell_NotifyIconW, ShellExecuteExW,
        ShellExecuteW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, BN_CLICKED, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, DC_HASDEFID, DM_GETDEFID,
        HMENU, HWND_TOPMOST, IDC_ARROW, IDCANCEL, IsDialogMessageW, LoadCursorW, PostQuitMessage,
        SPI_GETWORKAREA, SendMessageW, SetWindowPos, ShowWindow, SystemParametersInfoW, WM_CLOSE,
        WM_COMMAND, WM_SETFONT, WS_CAPTION, WS_CHILD, WS_EX_APPWINDOW, WS_EX_DLGMODALFRAME,
        WS_EX_TOPMOST, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
        DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos,
        GetMessageW, GetSystemMetrics, GetWindowThreadProcessId, HICON, IDI_APPLICATION, IDYES,
        IMAGE_ICON, InSendMessageEx, KillTimer, LR_DEFAULTCOLOR, LoadIconW, LoadImageW,
        MB_DEFBUTTON2, MB_ICONERROR, MB_ICONQUESTION, MB_OK, MB_SETFOREGROUND, MB_SYSTEMMODAL,
        MB_TOPMOST, MB_YESNO, MF_CHECKED, MF_DISABLED, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW,
        MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, PostMessageW, QS_ALLINPUT,
        RegisterClassExW, RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON, SMTO_ABORTIFHUNG,
        SW_HIDE, SW_SHOWNORMAL, SendMessageTimeoutW, SetForegroundWindow, SetMenuDefaultItem,
        SetProcessDPIAware, SetTimer, TPM_BOTTOMALIGN, TPM_NONOTIFY, TPM_RETURNCMD,
        TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_CONTEXTMENU, WM_COPYDATA,
        WM_MOUSEMOVE, WM_NULL, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
    };

    use super::{
        COLLECT_MAX, COLLECT_MAX_BYTES, COLLECT_MAX_FILES, Collected, ElevationDeclined, HttpSink,
        MenuItem, TooManyFiles, TrayEvent, collect_wait, copy_wide, decode_utf16_le, dedup_paths,
        parse_multiselect, same_exe_path, split_https_url, split_wide_list, verb_command,
        verb_label,
    };
    use crate::diag;
    use crate::verify::{DialogLive, DialogView};

    // The only sound `hoppyw send` plays (on delivery). 'static, so the buffer outlives the
    // asynchronous SND_MEMORY playback.
    static DONE_JINGLE: &[u8] = include_bytes!("../../../assets/music/desktop_done.wav");

    /// Tray menus are built on each click and shown at the cursor (`popup_menu`).
    pub const NATIVE_POPUPS: bool = true;

    const WM_TRAY: u32 = WM_APP + 1;
    const WM_WAKE: u32 = WM_APP + 2;
    const WM_REOPEN: u32 = WM_APP + 3;
    /// Posted to ourselves whenever `wndproc` queues an event; see `wndproc`.
    const WM_KICK: u32 = WM_APP + 4;
    /// `NIN_SELECT | NINF_KEY` (not exported by windows-sys).
    const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
    const LANG_ITALIAN: u16 = 0x10;
    const TRAY_UID: u32 = 1;
    const CLASS_NAME: &str = "HoppyTrayWindow";
    /// Icon resource id embedded by build.rs (winresource `set_icon` uses id 1).
    const ICON_RESOURCE_ID: usize = 1;

    /// "TaskbarCreated" is broadcast when Explorer restarts; the icon must be re-added.
    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

    enum Raw {
        /// With the time `wndproc` saw it (logged when it waited too long).
        Event(TrayEvent, Instant),
        Readd,
    }

    thread_local! {
        /// Filled by `wndproc`, drained by `Tray::next_event` (never re-entrantly borrowed).
        static QUEUE: RefCell<VecDeque<Raw>> = const { RefCell::new(VecDeque::new()) };
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_path(p: &Path) -> Vec<u16> {
        p.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Starts `tune` asynchronously (loops: Idle, Transfer), replacing whatever this process
    /// was playing. Silent if the PC has no audio device (SND_NODEFAULT: no beep either).
    /// One-shot "delivered" jingle (assets/music/desktop_done.wav), asynchronous.
    pub fn play_done_jingle() {
        // SAFETY: with SND_MEMORY the "name" is a pointer to a complete WAV image; it is
        // 'static, so it stays valid for the whole asynchronous playback.
        unsafe {
            PlaySoundW(
                DONE_JINGLE.as_ptr().cast(),
                null_mut(),
                SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
            )
        };
    }

    pub fn stop_sound() {
        // SAFETY: a null sound stops this process's playback.
        unsafe { PlaySoundW(null(), null_mut(), 0) };
    }

    pub fn ui_language_is_italian() -> bool {
        // SAFETY: no arguments, no side effects.
        let lang = unsafe { GetUserDefaultUILanguage() };
        lang & 0x3ff == LANG_ITALIAN
    }

    /// Yes/No with **No** as the default button: the dialog pops up over whatever the user
    /// is typing into, and a stray Enter/Space must not accept files.
    pub fn message_box_yes_no(title: &str, body: &str) -> bool {
        let (title, body) = (wide(title), wide(body));
        let style = MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION | MB_SYSTEMMODAL | MB_SETFOREGROUND;
        // SAFETY: both strings are NUL-terminated UTF-16 buffers alive for the call.
        unsafe { MessageBoxW(null_mut(), body.as_ptr(), title.as_ptr(), style) == IDYES }
    }

    pub fn message_box_error(title: &str, body: &str) {
        let (title, body) = (wide(title), wide(body));
        let style = MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST;
        // SAFETY: both strings are NUL-terminated UTF-16 buffers alive for the call.
        unsafe { MessageBoxW(null_mut(), body.as_ptr(), title.as_ptr(), style) };
    }

    /// Opens a folder in Explorer.
    pub fn open_folder(dir: &Path) -> bool {
        shell_open(&wide_path(dir))
    }

    /// Opens a web page in the default browser.
    pub fn open_url(url: &str) -> bool {
        shell_open(&wide(url))
    }

    /// Blocking (a network folder can take seconds): `hoppyw` calls it from worker threads,
    /// which ShellExecute wants COM-initialised.
    fn shell_open(file: &[u16]) -> bool {
        let verb = wide("open");
        let flags = (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32;
        // SAFETY: plain call; S_OK/S_FALSE (>= 0) must be balanced by CoUninitialize.
        let com = unsafe { CoInitializeEx(null(), flags) } >= 0;
        // SAFETY: NUL-terminated buffers alive for the call; null params/dir are allowed.
        let r = unsafe {
            ShellExecuteW(
                null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            )
        };
        if com {
            // SAFETY: balances the successful CoInitializeEx above on this thread.
            unsafe { CoUninitialize() };
        }
        r as isize > 32
    }

    /// A process started through UAC; its handle is closed on drop.
    pub struct Elevated(usize);

    impl Elevated {
        /// Exit code, or `None` on timeout/error.
        pub fn wait(self, timeout_ms: u32) -> Option<u32> {
            let handle = self.0 as HANDLE;
            if handle.is_null() {
                return None;
            }
            let mut code = 0u32;
            // SAFETY: live process handle owned by `self`; valid out-pointer.
            unsafe {
                (WaitForSingleObject(handle, timeout_ms) == WAIT_OBJECT_0
                    && GetExitCodeProcess(handle, &mut code) != 0)
                    .then_some(code)
            }
        }
    }

    impl Drop for Elevated {
        fn drop(&mut self) {
            if self.0 != 0 {
                // SAFETY: handle returned by ShellExecuteExW, closed exactly once.
                unsafe { CloseHandle(self.0 as HANDLE) };
            }
        }
    }

    /// Runs `exe params` hidden as administrator (UAC prompt, owned by the tray window when
    /// given, which may belong to another thread). Blocks until the user has answered, so
    /// `hoppyw` calls it from a worker; `ElevationDeclined` if they said No.
    /// `params` must be a constant: it is a command line, not a script.
    pub fn run_elevated(owner: Option<&Waker>, exe: &Path, params: &str) -> Result<Elevated> {
        let (verb, file, params) = (wide("runas"), wide_path(exe), wide(params));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            hwnd: owner.map_or(null_mut(), |w| w.0 as HWND),
            lpVerb: verb.as_ptr(),
            lpFile: file.as_ptr(),
            lpParameters: params.as_ptr(),
            nShow: SW_HIDE,
            ..Default::default()
        };
        // SAFETY: `info` is initialised with the right cbSize; strings outlive the call.
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            // SAFETY: reads the error of ShellExecuteExW just above.
            let err = unsafe { GetLastError() };
            if err == ERROR_CANCELLED {
                return Err(ElevationDeclined.into());
            }
            bail!(
                "could not start {} as administrator (error {err})",
                exe.display()
            );
        }
        Ok(Elevated(info.hProcess as usize))
    }

    /// A WinHTTP handle, closed on drop.
    struct Internet(*mut core::ffi::c_void);

    impl Drop for Internet {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: handle returned by WinHTTP, closed exactly once.
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    fn net_error(what: &str) -> anyhow::Error {
        // SAFETY: reads this thread's last-error value, set by the failed WinHTTP call.
        anyhow::anyhow!("{what} failed (WinHTTP error {})", unsafe {
            GetLastError()
        })
    }

    fn query_u64(request: &Internet, info: u32) -> Option<u64> {
        let mut value: u64 = 0;
        let mut len = size_of::<u64>() as u32;
        // SAFETY: live request; the out-buffer is a u64 and `len` says so.
        let ok = unsafe {
            WinHttpQueryHeaders(
                request.0,
                info | WINHTTP_QUERY_FLAG_NUMBER64,
                null(),
                (&raw mut value).cast(),
                &mut len,
                null_mut(),
            )
        };
        (ok != 0).then_some(value)
    }

    /// HTTPS GET through WinHTTP (system proxy, OS TLS, https redirects followed). `sink`
    /// gets each chunk and the Content-Length when known; non-200 is an error. Blocking:
    /// call it from a worker thread.
    pub fn http_get(url: &str, user_agent: &str, sink: &mut HttpSink<'_>) -> Result<()> {
        let (host, path) = split_https_url(url).context("unsupported URL")?;
        let (agent, host, path, verb) = (wide(user_agent), wide(host), wide(path), wide("GET"));
        // SAFETY: NUL-terminated buffers alive for the call; null proxy = automatic.
        let session = Internet(unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                null(),
                null(),
                0,
            )
        });
        if session.0.is_null() {
            return Err(net_error("WinHttpOpen"));
        }
        // SAFETY: live session handle; timeouts in milliseconds.
        unsafe { WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 30_000) };
        // SAFETY: live session; host is NUL-terminated and alive for the call.
        let connect = Internet(unsafe {
            WinHttpConnect(session.0, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0)
        });
        if connect.0.is_null() {
            return Err(net_error("WinHttpConnect"));
        }
        // SAFETY: live connection; strings alive for the call; null = HTTP/1.1, no
        // referrer, no accept types.
        let request = Internet(unsafe {
            WinHttpOpenRequest(
                connect.0,
                verb.as_ptr(),
                path.as_ptr(),
                null(),
                null(),
                null(),
                WINHTTP_FLAG_SECURE,
            )
        });
        if request.0.is_null() {
            return Err(net_error("WinHttpOpenRequest"));
        }
        // SAFETY: live request; no extra headers and no body.
        let sent = unsafe {
            WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) != 0
                && WinHttpReceiveResponse(request.0, null_mut()) != 0
        };
        if !sent {
            return Err(net_error("HTTPS request"));
        }
        match query_u64(&request, WINHTTP_QUERY_STATUS_CODE) {
            Some(200) => {}
            status => bail!("HTTP status {}", status.unwrap_or(0)),
        }
        let total = query_u64(&request, WINHTTP_QUERY_CONTENT_LENGTH);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut n: u32 = 0;
            // SAFETY: live request; `buf` is writable for `buf.len()` bytes.
            let ok = unsafe {
                WinHttpReadData(request.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n)
            };
            if ok == 0 {
                return Err(net_error("WinHttpReadData"));
            }
            if n == 0 {
                return Ok(());
            }
            sink(&buf[..n as usize], total)?;
        }
    }

    /// Held for the life of the process; a second holder of the same name gets `None`.
    pub struct InstanceGuard(HANDLE);

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: handle returned by CreateMutexW, closed exactly once.
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    pub fn single_instance(name: &str) -> Option<InstanceGuard> {
        let name = wide(name);
        // SAFETY: null attributes = defaults; name is NUL-terminated and alive for the call.
        let handle = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
        // SAFETY: reads this thread's last-error value, set by CreateMutexW just above.
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let guard = InstanceGuard(handle);
        (!existed).then_some(guard)
    }

    /// Asks an already running tray (window titled `window_title`) to show itself.
    pub fn signal_existing(window_title: &str) -> bool {
        let (class, title) = (wide(CLASS_NAME), wide(window_title));
        // SAFETY: NUL-terminated buffers alive for the call.
        let hwnd = unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) };
        // SAFETY: PostMessageW tolerates any handle value; null was excluded.
        !hwnd.is_null() && unsafe { PostMessageW(hwnd, WM_REOPEN, 0, 0) } != 0
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let now = Instant::now();
        let event = |e| Some(Raw::Event(e, now));
        let raw = match msg {
            // NOTIFYICON_VERSION_4: LOWORD(lParam) is the notification event; for clicks,
            // NIN_(KEY)SELECT and WM_CONTEXTMENU, wParam holds the anchor point (x, y as
            // signed 16-bit values, screen coordinates) where the menu belongs.
            WM_TRAY => {
                let what = (lparam as u32) & 0xffff;
                if what != WM_MOUSEMOVE {
                    log_tray_message(what, wparam, lparam);
                }
                match what {
                    NIN_SELECT | NIN_KEYSELECT | WM_CONTEXTMENU => {
                        let x = i32::from((wparam & 0xffff) as u16 as i16);
                        let y = i32::from(((wparam >> 16) & 0xffff) as u16 as i16);
                        event(TrayEvent::Click(Some((x, y))))
                    }
                    NIN_BALLOONUSERCLICK => event(TrayEvent::BalloonClick),
                    _ => None,
                }
            }
            WM_WAKE => event(TrayEvent::Wake),
            WM_REOPEN => event(TrayEvent::Reopen),
            WM_TIMER => event(TrayEvent::Timer(wparam)),
            m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => Some(Raw::Readd),
            _ => None,
        };
        match raw {
            Some(raw) => {
                QUEUE.with(|q| q.borrow_mut().push_back(raw));
                // Explorer delivers tray callbacks (and "TaskbarCreated") as *sent* messages
                // (SendNotifyMessage). GetMessageW runs `wndproc` for those internally and
                // keeps waiting for a *posted* message, so `next_event` would only see this
                // event when some unrelated message arrives — seconds or minutes later, with
                // the menu then opening wherever the mouse happens to be. Posting a message
                // to ourselves makes GetMessageW return now. Harmless for posted messages.
                // SAFETY: our own live window; PostMessageW is thread-safe.
                unsafe { PostMessageW(hwnd, WM_KICK, 0, 0) };
                0
            }
            // SAFETY: forwards the unmodified arguments Windows passed to us.
            None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    /// Diagnostic log line for a tray callback: event, raw parameters, and how it was
    /// delivered (posted, sent, or SendNotifyMessage).
    fn log_tray_message(what: u32, wparam: WPARAM, lparam: LPARAM) {
        // SAFETY: no pointers; describes the message being processed on this thread.
        let how = unsafe { InSendMessageEx(null()) };
        let delivery = match how & 0x7 {
            0 => "posted",
            1 => "sent",
            2 => "notify",
            _ => "callback",
        };
        diag::log(format!(
            "tray: event=0x{what:04x} wParam=0x{wparam:x} lParam=0x{lparam:x} delivery={delivery}"
        ));
    }

    /// Lets worker threads nudge the UI thread (`TrayEvent::Wake`); also the owner window
    /// for `run_elevated`.
    #[derive(Clone)]
    pub struct Waker(usize);

    impl Waker {
        pub fn wake(&self) {
            // SAFETY: PostMessageW is thread-safe; a destroyed window just makes it fail.
            unsafe { PostMessageW(self.0 as HWND, WM_WAKE, 0, 0) };
        }
    }

    /// Hidden window + notification-area icon. UI-thread only (`!Send`).
    pub struct Tray {
        hwnd: HWND,
        icon: HICON,
        owned_icon: bool,
        tip: RefCell<String>,
    }

    impl Tray {
        /// `window_title` identifies the hidden window for `signal_existing`.
        pub fn new(window_title: &str, tip: &str) -> Result<Tray> {
            // SAFETY: no pointers; failures only mean "already set" / blurry scaling.
            unsafe {
                SetProcessDPIAware();
                CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
            }
            let taskbar = wide("TaskbarCreated");
            // SAFETY: NUL-terminated buffer alive for the call.
            let msg = unsafe { RegisterWindowMessageW(taskbar.as_ptr()) };
            TASKBAR_CREATED.store(msg, Ordering::Relaxed);
            // A top-level (not message-only) window so it receives "TaskbarCreated".
            let hwnd = hidden_window(CLASS_NAME, wndproc, window_title)
                .context("cannot create the notification-area window")?;
            // SAFETY: null = this executable's module.
            let hinstance: HINSTANCE = unsafe { GetModuleHandleW(null()) };
            let (icon, owned_icon) = load_icon(hinstance);
            let tray = Tray {
                hwnd,
                icon,
                owned_icon,
                tip: RefCell::new(tip.to_owned()),
            };
            tray.add();
            Ok(tray)
        }

        fn nid(&self) -> NOTIFYICONDATAW {
            NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: TRAY_UID,
                ..Default::default()
            }
        }

        fn add(&self) {
            let mut nid = self.nid();
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
            nid.uCallbackMessage = WM_TRAY;
            nid.hIcon = self.icon;
            copy_wide(&mut nid.szTip, &self.tip.borrow());
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            // SAFETY: `nid` is fully initialised with the right cbSize. Failure (Explorer
            // not running yet) is recovered by the "TaskbarCreated" re-add.
            let (added, versioned) = unsafe {
                (
                    Shell_NotifyIconW(NIM_ADD, &nid) != 0,
                    Shell_NotifyIconW(NIM_SETVERSION, &nid) != 0,
                )
            };
            diag::log(format!("tray: icon added={added} version4={versioned}"));
        }

        pub fn set_tip(&self, tip: &str) {
            if *self.tip.borrow() == tip {
                return;
            }
            *self.tip.borrow_mut() = tip.to_owned();
            let mut nid = self.nid();
            nid.uFlags = NIF_TIP | NIF_SHOWTIP;
            copy_wide(&mut nid.szTip, tip);
            // SAFETY: `nid` is fully initialised with the right cbSize.
            unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
        }

        /// Balloon (a toast on Windows 10/11). Clicking it yields `TrayEvent::BalloonClick`.
        /// `sound`: the system notification sound (off only when our own jingle plays).
        pub fn balloon(&self, title: &str, text: &str, error: bool, sound: bool) {
            let mut nid = self.nid();
            nid.uFlags = NIF_INFO;
            copy_wide(&mut nid.szInfoTitle, title);
            copy_wide(&mut nid.szInfo, if text.is_empty() { " " } else { text });
            nid.dwInfoFlags = if error { NIIF_ERROR } else { NIIF_INFO };
            if !sound {
                nid.dwInfoFlags |= NIIF_NOSOUND;
            }
            // SAFETY: `nid` is fully initialised with the right cbSize.
            unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
        }

        pub fn waker(&self) -> Waker {
            Waker(self.hwnd as usize)
        }

        /// Persistent menus are a Linux thing: Windows builds each menu on click.
        pub fn set_menu(&self, _items: &[MenuItem<'_>]) {}

        pub fn set_timer(&self, id: usize, ms: u32) {
            // SAFETY: live window; no callback, so WM_TIMER is posted to `wndproc`.
            unsafe { SetTimer(self.hwnd, id, ms, None) };
        }

        pub fn kill_timer(&self, id: usize) {
            // SAFETY: live window; unknown ids just fail.
            unsafe { KillTimer(self.hwnd, id) };
        }

        /// Shows a popup menu at `anchor` (the point a tray click reported) or else at the
        /// mouse cursor, and returns the chosen id (`None` = dismissed). `above` anchors the
        /// menu's bottom edge (taskbar menus).
        pub fn popup_menu(
            &self,
            items: &[MenuItem<'_>],
            above: bool,
            anchor: Option<(i32, i32)>,
        ) -> Option<u32> {
            // SAFETY: no arguments.
            let menu = unsafe { CreatePopupMenu() };
            if menu.is_null() {
                return None;
            }
            for item in items {
                let (flags, id, text) = match *item {
                    MenuItem::Action(id, t) => (MF_STRING, id as usize, Some(t)),
                    MenuItem::Check(id, t, on) => {
                        let check = if on { MF_CHECKED } else { 0 };
                        (MF_STRING | check, id as usize, Some(t))
                    }
                    MenuItem::Bold(id, t) => (MF_STRING, id as usize, Some(t)),
                    MenuItem::Label(t) => (MF_STRING | MF_DISABLED, 0, Some(t)),
                    MenuItem::Separator => (MF_SEPARATOR, 0, None),
                };
                // '&' marks a keyboard accelerator in menus; names must show it literally.
                let text = text.map(|t| wide(&t.replace('&', "&&")));
                let ptr = text.as_ref().map_or(null(), |t| t.as_ptr());
                // SAFETY: live menu; AppendMenuW copies the string.
                unsafe { AppendMenuW(menu, flags, id, ptr) };
                if let MenuItem::Bold(id, _) = *item {
                    // SAFETY: live menu; `id` was just appended (by command, not position).
                    unsafe { SetMenuDefaultItem(menu, id, 0) };
                }
            }
            let mut flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON;
            if above {
                flags |= TPM_BOTTOMALIGN;
            }
            let mut pt = POINT { x: 0, y: 0 };
            let source = match anchor {
                Some((x, y)) => {
                    pt = POINT { x, y };
                    "anchor"
                }
                None => {
                    // SAFETY: valid out-pointer.
                    unsafe { GetCursorPos(&mut pt) };
                    "cursor"
                }
            };
            // SAFETY: live window. A tray menu needs its window in the foreground or it does
            // not close on outside clicks; Explorer grants us that right with the click, so
            // this fails (logged) mainly when the click is handled late.
            let foreground = unsafe { SetForegroundWindow(self.hwnd) } != 0;
            diag::log(format!(
                "menu: shown, {} items at {},{} ({source}), foreground={foreground}",
                items.len(),
                pt.x,
                pt.y
            ));
            let shown = Instant::now();
            // SAFETY: live window and menu. WM_NULL after is the documented way to make a
            // tray menu close properly.
            let cmd = unsafe {
                let cmd = TrackPopupMenu(menu, flags, pt.x, pt.y, 0, self.hwnd, null());
                PostMessageW(self.hwnd, WM_NULL, 0, 0);
                DestroyMenu(menu);
                cmd
            };
            diag::log(format!(
                "menu: returned {cmd} after {} ms",
                shown.elapsed().as_millis()
            ));
            u32::try_from(cmd).ok().filter(|&c| c != 0)
        }

        /// Native "Open" dialog with multi-select; empty if cancelled.
        pub fn pick_files(&self, title: &str) -> Result<Vec<PathBuf>, TooManyFiles> {
            pick_files_owned_by(self.hwnd, title)
        }

        /// Pumps messages until the window reports something; `None` on WM_QUIT.
        pub fn next_event(&self) -> Option<TrayEvent> {
            loop {
                while let Some(raw) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
                    match raw {
                        Raw::Event(e, at) => {
                            let waited = at.elapsed();
                            if waited >= diag::SLOW_HANDLER {
                                diag::log(format!(
                                    "tray: {e:?} waited {} ms before being handled",
                                    waited.as_millis()
                                ));
                            }
                            return Some(e);
                        }
                        Raw::Readd => {
                            diag::log("tray: taskbar re-created, adding the icon again");
                            self.add();
                        }
                    }
                }
                let mut msg = MSG::default();
                // SAFETY: valid out-pointer; null hwnd = every message of this thread.
                if unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } <= 0 {
                    return None;
                }
                // SAFETY: `msg` was filled in by GetMessageW.
                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            let nid = self.nid();
            // SAFETY: removes our own icon, then destroys our own window/icon exactly once.
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &nid);
                DestroyWindow(self.hwnd);
                if self.owned_icon {
                    DestroyIcon(self.icon);
                }
            }
        }
    }

    /// Small icon from the exe's resources; stock application icon as fallback.
    fn load_icon(hinstance: HINSTANCE) -> (HICON, bool) {
        // SAFETY: plain metric queries.
        let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON)) };
        // MAKEINTRESOURCEW(id): integer resource ids travel in the pointer value.
        let name = ICON_RESOURCE_ID as *const u16;
        // SAFETY: integer resource ids are valid `name` values for LoadImageW.
        let icon = unsafe { LoadImageW(hinstance, name, IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR) };
        if !icon.is_null() {
            return (icon, true);
        }
        // SAFETY: shared stock icon; must not be destroyed (owned = false).
        (unsafe { LoadIconW(null_mut(), IDI_APPLICATION) }, false)
    }

    type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

    /// A never-shown top-level window of class `class` (registered on first use), findable
    /// with `FindWindowW(class, title)`.
    fn hidden_window(class: &str, proc_: WndProc, title: &str) -> Result<HWND> {
        // SAFETY: null = this executable's module.
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(null()) };
        let (class, title) = (wide(class), wide(title));
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_),
            hInstance: hinstance,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        // SAFETY: `wc` and the class name are alive for the call; registering twice in one
        // process fails harmlessly and the first registration is used.
        unsafe { RegisterClassExW(&wc) };
        // SAFETY: class registered above; strings alive for the call.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                hinstance,
                null(),
            )
        };
        if hwnd.is_null() {
            bail!("CreateWindowExW failed");
        }
        Ok(hwnd)
    }

    /// Native "Open" dialog with no tray icon yet (`hoppyw send` without files, e.g. the
    /// desktop shortcut double-clicked); empty if cancelled.
    pub fn pick_files_unowned(title: &str) -> Result<Vec<PathBuf>, TooManyFiles> {
        pick_files_owned_by(null_mut(), title)
    }

    fn pick_files_owned_by(owner: HWND, title: &str) -> Result<Vec<PathBuf>, TooManyFiles> {
        let mut buf = vec![0u16; 64 * 1024];
        let title = wide(title);
        let mut ofn = OPENFILENAMEW {
            lStructSize: size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: owner,
            lpstrFile: buf.as_mut_ptr(),
            nMaxFile: buf.len() as u32,
            lpstrTitle: title.as_ptr(),
            Flags: OFN_EXPLORER
                | OFN_ALLOWMULTISELECT
                | OFN_FILEMUSTEXIST
                | OFN_PATHMUSTEXIST
                | OFN_HIDEREADONLY,
            ..Default::default()
        };
        // SAFETY: `buf` and `title` outlive the modal call; sizes match the buffers; the
        // owner is our live window or null.
        let ok = unsafe {
            if !owner.is_null() {
                SetForegroundWindow(owner);
            }
            GetOpenFileNameW(&mut ofn)
        };
        if ok == 0 {
            // SAFETY: no arguments; reads the error of the dialog call just above.
            return match unsafe { CommDlgExtendedError() } {
                FNERR_BUFFERTOOSMALL => Err(TooManyFiles),
                _ => Ok(Vec::new()), // cancelled (0) or a dialog failure
            };
        }
        Ok(parse_multiselect(&buf, |w| {
            PathBuf::from(OsString::from_wide(w))
        }))
    }

    // ------------------------------------------------------------ verification dialog
    //
    // A small window of our own (MessageBox cannot show large emoji): static texts, the 4
    // emoji in "Segoe UI Emoji" (GDI draws them in one colour, so each has its name under
    // it), push buttons. It runs its own message loop on the calling thread (a worker: never
    // the tray thread), with IsDialogMessageW for Tab, Enter (= the first, default button)
    // and Esc (= closed = the default answer).

    const DIALOG_CLASS: &str = "HoppyDialog";
    const DIALOG_TIMER: usize = 1;
    /// Control ids of the buttons: `CTL_BUTTON + index`.
    const CTL_BUTTON: usize = 100;
    /// Not exported without the SystemServices feature.
    const SS_CENTER: u32 = 0x1;
    const SS_NOPREFIX: u32 = 0x80;

    thread_local! {
        /// The last button (or Enter/Esc) of this thread's dialog, set by `dialog_wndproc`.
        static DLG_COMMAND: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }

    unsafe extern "system" fn dialog_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            // Buttons, and IsDialogMessageW's Enter (default id) / Esc (IDCANCEL).
            WM_COMMAND if (wparam >> 16) & 0xffff == BN_CLICKED as usize => {
                DLG_COMMAND.with(|c| c.set(Some(wparam & 0xffff)));
                0
            }
            WM_CLOSE => {
                DLG_COMMAND.with(|c| c.set(Some(IDCANCEL as usize)));
                0
            }
            // Enter = the first button, never "Accept".
            DM_GETDEFID => ((DC_HASDEFID as LRESULT) << 16) | CTL_BUTTON as LRESULT,
            // SAFETY: forwards the unmodified arguments Windows passed to us.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    /// A GDI font, deleted on drop.
    struct Font(HFONT);

    impl Font {
        fn new(face: &str, height: i32, weight: u32) -> Font {
            let face = wide(face);
            // SAFETY: plain values; the face name is NUL-terminated and alive for the call.
            Font(unsafe {
                CreateFontW(
                    -height,
                    0,
                    0,
                    0,
                    weight as i32,
                    0,
                    0,
                    0,
                    u32::from(DEFAULT_CHARSET),
                    u32::from(OUT_DEFAULT_PRECIS),
                    u32::from(CLIP_DEFAULT_PRECIS),
                    u32::from(CLEARTYPE_QUALITY),
                    u32::from(DEFAULT_PITCH),
                    face.as_ptr(),
                )
            })
        }
    }

    impl Drop for Font {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: created by CreateFontW, deleted once, after the window is gone.
                unsafe { DeleteObject(self.0) };
            }
        }
    }

    /// Height of `text` wrapped to `width` pixels in `font` (+2: static controls wrap a
    /// hair differently from DT_CALCRECT).
    fn text_height(text: &str, font: &Font, width: i32) -> i32 {
        text_rect(text, font, width, DT_WORDBREAK).bottom + 2
    }

    fn text_width(text: &str, font: &Font) -> i32 {
        text_rect(text, font, 10_000, DT_SINGLELINE).right
    }

    fn text_rect(text: &str, font: &Font, width: i32, flags: u32) -> RECT {
        let w = wide(text);
        let mut r = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: 0,
        };
        // SAFETY: screen DC released below; the old font is selected back before release.
        unsafe {
            let hdc = GetDC(null_mut());
            let old = SelectObject(hdc, font.0);
            DrawTextW(
                hdc,
                w.as_ptr(),
                -1,
                &mut r,
                DT_CALCRECT | DT_NOPREFIX | flags,
            );
            SelectObject(hdc, old);
            ReleaseDC(null_mut(), hdc);
        }
        r
    }

    /// Creates a child control (`STATIC` / `BUTTON`) with `font`.
    #[allow(clippy::too_many_arguments)]
    fn child(
        parent: HWND,
        class: &str,
        text: &str,
        style: u32,
        id: usize,
        (x, y, w, h): (i32, i32, i32, i32),
        font: &Font,
    ) -> HWND {
        let (class, text) = (wide(class), wide(text));
        // SAFETY: null = this module; strings alive for the call; the parent is our window.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                text.as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                x,
                y,
                w,
                h,
                parent,
                id as HMENU,
                GetModuleHandleW(null()),
                null(),
            )
        };
        // SAFETY: a live control (or null, which SendMessageW tolerates); the font
        // outlives the window.
        unsafe { SendMessageW(hwnd, WM_SETFONT, font.0 as WPARAM, 0) };
        hwnd
    }

    /// The accept question: the verification dialog (Decline is the default button).
    pub fn ask_offer(view: &DialogView, live: &DialogLive) -> Option<u32> {
        show_dialog(view, live)
    }

    /// Shows `view` and waits for a button; `None` = closed (Esc, ×), timed out or closed
    /// through `live` (the caller uses the default). Worker threads only.
    pub fn show_dialog(view: &DialogView, live: &DialogLive) -> Option<u32> {
        // SAFETY: null = this executable's module.
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(null()) };
        let class = wide(DIALOG_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(dialog_wndproc),
            hInstance: hinstance,
            lpszClassName: class.as_ptr(),
            // SAFETY: shared system cursor/brush; neither is ever destroyed.
            hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
            // SAFETY: as above.
            hbrBackground: unsafe { GetSysColorBrush(COLOR_BTNFACE) },
            ..Default::default()
        };
        // SAFETY: `wc` and the class name are alive for the call; registering twice in one
        // process fails harmlessly and the first registration is used.
        unsafe { RegisterClassExW(&wc) };

        // Layout in pixels at the screen's DPI (the process is system-DPI aware).
        // SAFETY: screen DC, released right away.
        let dpi = unsafe {
            let hdc = GetDC(null_mut());
            let dpi = GetDeviceCaps(hdc, LOGPIXELSY as _);
            ReleaseDC(null_mut(), hdc);
            if dpi > 0 { dpi } else { 96 }
        };
        let px = |v: i32| v * dpi / 96;
        let text_font = Font::new("Segoe UI", px(15), FW_NORMAL);
        let small_font = Font::new("Segoe UI", px(12), FW_NORMAL);
        let emoji_font = Font::new("Segoe UI Emoji", px(40), FW_NORMAL);
        let (width, margin) = (px(480), px(18));
        let inner = width - 2 * margin;

        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
        let ex_style = WS_EX_TOPMOST | WS_EX_DLGMODALFRAME | WS_EX_APPWINDOW;
        let title = wide(&view.title);
        // Created hidden and empty, sized once the controls are laid out.
        // SAFETY: class registered above; strings alive for the call.
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                class.as_ptr(),
                title.as_ptr(),
                style,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                hinstance,
                null(),
            )
        };
        if hwnd.is_null() {
            diag::log("dialog: CreateWindowExW failed");
            return None;
        }
        let label = SS_NOPREFIX; // '&' in device names is not an accelerator
        let mut y = margin;
        let h = text_height(&view.text, &text_font, inner);
        child(
            hwnd,
            "STATIC",
            &view.text,
            label,
            0,
            (margin, y, inner, h),
            &text_font,
        );
        y += h + px(10);
        if !view.note.is_empty() {
            let h = text_height(&view.note, &text_font, inner);
            child(
                hwnd,
                "STATIC",
                &view.note,
                label,
                0,
                (margin, y, inner, h),
                &text_font,
            );
            y += h + px(10);
        }
        if let Some(code) = &view.code {
            let cell = inner / 4;
            let (emoji_h, names_h) = (px(56), px(34));
            for i in 0..4 {
                let x = margin + cell * i as i32;
                let center = label | SS_CENTER;
                child(
                    hwnd,
                    "STATIC",
                    code.emoji[i],
                    center,
                    0,
                    (x, y, cell, emoji_h),
                    &emoji_font,
                );
                child(
                    hwnd,
                    "STATIC",
                    code.names[i],
                    center,
                    0,
                    (x, y + emoji_h, cell, names_h),
                    &small_font,
                );
            }
            y += emoji_h + names_h + px(10);
        }
        // Buttons right-aligned in the view's order.
        let button_h = px(30);
        let button_w = |t: &str| (text_width(t, &text_font) + px(28)).max(px(90));
        let mut x = width - margin;
        let mut first_button = null_mut();
        for (i, (_, text)) in view.buttons.iter().enumerate().rev() {
            let w = button_w(text);
            x -= w;
            let kind = if i == 0 {
                BS_DEFPUSHBUTTON
            } else {
                BS_PUSHBUTTON
            } as u32;
            let b = child(
                hwnd,
                "BUTTON",
                text,
                WS_TABSTOP | kind,
                CTL_BUTTON + i,
                (x, y, w, button_h),
                &text_font,
            );
            if i == 0 {
                first_button = b;
            }
            x -= px(8);
        }
        let client_h = y + button_h + margin;

        // Size and centre on the work area, then show.
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: client_h,
        };
        let mut work = RECT::default();
        // SAFETY: valid out-pointers; our live window.
        unsafe {
            AdjustWindowRectEx(&mut frame, style, 0, ex_style);
            SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut work).cast(), 0);
            let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
            let left = work.left + ((work.right - work.left) - w).max(0) / 2;
            let top = work.top + ((work.bottom - work.top) - h).max(0) / 2;
            SetWindowPos(hwnd, HWND_TOPMOST, left, top, w, h, 0);
            ShowWindow(hwnd, SW_SHOWNORMAL);
            SetForegroundWindow(hwnd);
            SetFocus(first_button);
            SetTimer(
                hwnd,
                DIALOG_TIMER,
                crate::verify::LIVE_POLL.as_millis() as u32,
                None,
            );
        }

        let deadline = view.timeout.map(|t| Instant::now() + t);
        DLG_COMMAND.with(|c| c.set(None));
        let answer = loop {
            if let Some(id) = DLG_COMMAND.with(std::cell::Cell::take) {
                // Esc and the close box (IDCANCEL) are below CTL_BUTTON: closed.
                break id
                    .checked_sub(CTL_BUTTON)
                    .and_then(|i| view.buttons.get(i))
                    .map(|b| b.0);
            }
            if live.closed() || deadline.is_some_and(|d| Instant::now() >= d) {
                break None;
            }
            let mut msg = MSG::default();
            // SAFETY: valid out-pointer; null hwnd = every message of this thread (the
            // timer above makes this return at least every LIVE_POLL).
            let got = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
            if got == 0 {
                // WM_QUIT: hand it on to whoever runs the outer loop.
                // SAFETY: plain call.
                unsafe { PostQuitMessage(msg.wParam as i32) };
                break None;
            }
            if got < 0 {
                break None;
            }
            // SAFETY: `msg` was filled in by GetMessageW; our window is alive.
            unsafe {
                if IsDialogMessageW(hwnd, &msg) == 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        };
        // SAFETY: our own window and timer, destroyed once; the fonts drop after this.
        unsafe {
            KillTimer(hwnd, DIALOG_TIMER);
            DestroyWindow(hwnd);
        }
        DLG_COMMAND.with(|c| c.set(None));
        answer
    }

    // ------------------------------------------------------------ `hoppyw send` collector

    const COLLECT_CLASS: &str = "HoppySendCollector";
    const COLLECT_MUTEX: &str = r"Local\HoppySendCollector";
    /// Window class of the short-lived window a forwarder passes as WM_COPYDATA's wParam.
    const FORWARD_CLASS: &str = "HoppySendForwarder";
    /// WM_COPYDATA tag ("HOPP"); anything else is ignored.
    const COPYDATA_MAGIC: usize = 0x484F_5050;
    /// Larger payloads are ignored (a few thousand long paths fit).
    const COPYDATA_MAX: u32 = 4 * 1024 * 1024;

    thread_local! {
        /// Files received by `collect_wndproc` (within the limits), read by `collect`.
        static COLLECTED: RefCell<Collected> = RefCell::new(Collected::default());
        /// WM_COPYDATA messages refused because the sender is not our own executable.
        static REJECTED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    /// Full image path of the process owning `hwnd`.
    fn window_process_image(hwnd: HWND) -> Option<PathBuf> {
        let mut pid = 0u32;
        // SAFETY: any handle value is accepted; an invalid window leaves `pid` at 0.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == 0 {
            return None;
        }
        // SAFETY: plain call; a null handle is checked below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 32 * 1024];
        let mut len = buf.len() as u32;
        // SAFETY: `process` is open with query rights; `len` is the buffer size in u16s and
        // receives the length written (without NUL).
        let ok = unsafe {
            let ok =
                QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
            CloseHandle(process);
            ok
        };
        if ok == 0 {
            return None;
        }
        let path = buf.get(..len as usize)?;
        Some(PathBuf::from(OsString::from_wide(path)))
    }

    /// The forwarding window (wParam of WM_COPYDATA) belongs to a process running our own
    /// executable. wParam is chosen by the sender, so this only keeps out unrelated programs.
    fn sender_is_own_exe(sender: HWND) -> bool {
        let Ok(own) = std::env::current_exe() else {
            return false;
        };
        !sender.is_null() && window_process_image(sender).is_some_and(|p| same_exe_path(&p, &own))
    }

    unsafe extern "system" fn collect_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg != WM_COPYDATA {
            // SAFETY: forwards the unmodified arguments Windows passed to us.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
        let cds = lparam as *const COPYDATASTRUCT;
        if cds.is_null() {
            return 0;
        }
        // SAFETY: for WM_COPYDATA, lParam points to a COPYDATASTRUCT valid during the call.
        let cds = unsafe { &*cds };
        if cds.dwData != COPYDATA_MAGIC
            || cds.lpData.is_null()
            || cds.cbData > COPYDATA_MAX
            || cds.cbData % 2 != 0
        {
            return 0;
        }
        if !sender_is_own_exe(wparam as HWND) {
            REJECTED.with(|r| r.set(r.get() + 1));
            return 0;
        }
        // SAFETY: the system copied `cbData` bytes to `lpData` for the duration of the call.
        // Read as bytes: the copy carries no alignment guarantee for u16.
        let bytes =
            unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
        let units = decode_utf16_le(bytes);
        let paths: Vec<PathBuf> = split_wide_list(&units)
            .into_iter()
            .map(|w| PathBuf::from(OsString::from_wide(w)))
            .collect();
        // Taken even past the limits: the first instance reports "too many files" once.
        COLLECTED.with(|c| c.borrow_mut().add(bytes.len(), paths));
        1
    }

    /// See `super::gather_send_files`.
    pub fn gather_send_files(files: Vec<PathBuf>) -> Result<Option<Vec<PathBuf>>, TooManyFiles> {
        let start = Instant::now();
        loop {
            if let Some(guard) = single_instance(COLLECT_MUTEX) {
                return collect(files, guard).map(Some);
            }
            if forward(&files) {
                diag::log(format!(
                    "send: forwarded {} file(s) to the first instance",
                    files.len()
                ));
                return Ok(None);
            }
            // The collector is starting (no window yet) or closing (window gone, mutex not
            // yet released): retry, and send on our own if that takes too long.
            if start.elapsed() >= COLLECT_MAX {
                diag::log("send: no collector answered, sending alone");
                if files.len() > COLLECT_MAX_FILES {
                    return Err(TooManyFiles);
                }
                return Ok(Some(files));
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    unsafe extern "system" fn forward_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: forwards the unmodified arguments Windows passed to us.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// Hands `files` to the collector window; `true` if it took them.
    fn forward(files: &[PathBuf]) -> bool {
        let class = wide(COLLECT_CLASS);
        // SAFETY: NUL-terminated class name alive for the call; any title.
        let hwnd = unsafe { FindWindowW(class.as_ptr(), null()) };
        if hwnd.is_null() {
            return false;
        }
        let mut data: Vec<u16> = Vec::new();
        for f in files {
            data.extend(f.as_os_str().encode_wide());
            data.push(0);
        }
        let Ok(cb) = u32::try_from(data.len() * 2) else {
            return false;
        };
        // The collector checks that wParam's window belongs to our executable.
        let me = match hidden_window(FORWARD_CLASS, forward_wndproc, FORWARD_CLASS) {
            Ok(me) => me,
            Err(_) => return false,
        };
        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_MAGIC,
            cbData: cb,
            lpData: data.as_mut_ptr().cast(),
        };
        let mut pid = 0u32;
        let mut answer = 0usize;
        // SAFETY: live (or just closed: the calls then fail) window; `cds` and `data` are
        // alive during the synchronous send; `me` is our own window, destroyed once. The
        // collector will show the device menu, so pass it the foreground right Explorer
        // gave this process.
        let sent = unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid != 0 {
                AllowSetForegroundWindow(pid);
            }
            let sent = SendMessageTimeoutW(
                hwnd,
                WM_COPYDATA,
                me as WPARAM,
                (&raw const cds) as LPARAM,
                SMTO_ABORTIFHUNG,
                2000,
                &mut answer,
            );
            DestroyWindow(me);
            sent
        };
        sent != 0 && answer == 1
    }

    /// First instance: keep a findable window open while other instances keep arriving.
    fn collect(files: Vec<PathBuf>, guard: InstanceGuard) -> Result<Vec<PathBuf>, TooManyFiles> {
        let own = files.len();
        if own > COLLECT_MAX_FILES {
            return Err(TooManyFiles);
        }
        let hwnd = match hidden_window(COLLECT_CLASS, collect_wndproc, COLLECT_CLASS) {
            Ok(hwnd) => hwnd,
            Err(e) => {
                diag::log(format!("send: collector window failed: {e:#}"));
                return Ok(files);
            }
        };
        COLLECTED.with(|c| {
            *c.borrow_mut() = Collected {
                files,
                ..Default::default()
            }
        });
        let start = Instant::now();
        let mut last = start;
        let mut count = own;
        while let Some(wait) = collect_wait(start.elapsed(), last.elapsed()) {
            let ms = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX).max(1);
            let mut msg = MSG::default();
            // SAFETY: no handles; wakes on any message (sent ones included) or timeout.
            // PeekMessageW also runs pending *sent* messages (WM_COPYDATA) in our wndproc.
            unsafe {
                MsgWaitForMultipleObjects(0, null(), 0, ms, QS_ALLINPUT);
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            let (now, overflow) = COLLECTED.with(|c| {
                let c = c.borrow();
                (c.files.len(), c.overflow)
            });
            if now != count || overflow {
                count = now;
                last = Instant::now();
            }
        }
        // SAFETY: our own window, destroyed once; later forwarders find nothing and retry.
        unsafe { DestroyWindow(hwnd) };
        drop(guard);
        let got = COLLECTED.with(|c| std::mem::take(&mut *c.borrow_mut()));
        let rejected = REJECTED.with(|r| r.replace(0));
        if rejected > 0 {
            diag::log(format!(
                "send: ignored {rejected} message(s) not sent by this executable"
            ));
        }
        if got.overflow {
            diag::log(format!(
                "send: more than {COLLECT_MAX_FILES} files or {COLLECT_MAX_BYTES} bytes of paths"
            ));
            return Err(TooManyFiles);
        }
        let files = dedup_paths(got.files);
        diag::log(format!(
            "send: collected {} file(s) ({} from other instances) in {} ms",
            files.len(),
            files.len().saturating_sub(own),
            start.elapsed().as_millis()
        ));
        Ok(files)
    }

    // ------------------------------------------------------------ registry (HKCU only)

    /// An open registry key, closed on drop.
    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: key opened by RegCreateKeyExW/RegOpenKeyExW, closed exactly once.
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn reg_error(what: &str, path: &str, code: u32) -> anyhow::Error {
        anyhow::anyhow!("{what} HKCU\\{path} failed (error {code})")
    }

    fn create_key(path: &str) -> Result<Key> {
        let sub = wide(path);
        let mut key: HKEY = null_mut();
        // SAFETY: NUL-terminated subkey alive for the call; valid out-pointer.
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                null(),
                &mut key,
                null_mut(),
            )
        };
        if code != ERROR_SUCCESS {
            return Err(reg_error("creating", path, code));
        }
        Ok(Key(key))
    }

    /// `name: None` = the key's default value.
    fn set_string(key: &Key, name: Option<&str>, value: &[u16]) -> Result<()> {
        let mut data: Vec<u16> = value.to_vec();
        data.push(0);
        let name = name.map(wide);
        // SAFETY: live key; `data` holds `data.len() * 2` bytes incl. the terminating NUL.
        let code = unsafe {
            RegSetValueExW(
                key.0,
                name.as_ref().map_or(null(), |n| n.as_ptr()),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if code != ERROR_SUCCESS {
            bail!("writing a registry value failed (error {code})");
        }
        Ok(())
    }

    fn set_dword(key: &Key, name: &str, value: u32) -> Result<()> {
        let name = wide(name);
        // SAFETY: live key; the data is 4 bytes.
        let code = unsafe {
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                REG_DWORD,
                (&raw const value).cast(),
                4,
            )
        };
        if code != ERROR_SUCCESS {
            bail!("writing a registry value failed (error {code})");
        }
        Ok(())
    }

    fn key_exists(path: &str) -> bool {
        let sub = wide(path);
        let mut key: HKEY = null_mut();
        // SAFETY: NUL-terminated subkey alive for the call; valid out-pointer.
        let code = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ, &mut key) };
        let found = code == ERROR_SUCCESS;
        if found {
            drop(Key(key));
        }
        found
    }

    fn get_dword(path: &str, name: &str) -> Option<u32> {
        let (sub, name) = (wide(path), wide(name));
        let mut value = 0u32;
        let mut len = 4u32;
        // SAFETY: NUL-terminated strings alive for the call; 4-byte out-buffer.
        let code = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                (&raw mut value).cast(),
                &mut len,
            )
        };
        (code == ERROR_SUCCESS).then_some(value)
    }

    /// Deletes `path` and everything below it; a missing key is fine.
    fn delete_tree(path: &str) -> Result<()> {
        let sub = wide(path);
        // SAFETY: NUL-terminated subkey alive for the call.
        let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, sub.as_ptr()) };
        if code != ERROR_SUCCESS && code != ERROR_FILE_NOT_FOUND {
            return Err(reg_error("deleting", path, code));
        }
        Ok(())
    }

    /// `CurrentBuildNumber` of Windows (0 if unknown); Windows 11 starts at 22000.
    fn windows_build() -> u32 {
        let sub = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        let name = wide("CurrentBuildNumber");
        let mut buf = [0u16; 32];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: NUL-terminated strings alive for the call; `len` is the buffer's size.
        let code = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                sub.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if code != ERROR_SUCCESS {
            return 0;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
            .trim()
            .parse()
            .unwrap_or(0)
    }

    /// Explorer verb for every file type ("*"), current user only.
    const VERB_KEY: &str = r"Software\Classes\*\shell\Hoppy";
    /// Windows 11's new context menu is this COM class; an empty per-user InprocServer32
    /// overrides it, and Explorer (after a restart) shows the classic menu.
    const CLASSIC_KEY: &str = r"Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}";
    const CLASSIC_SERVER: &str =
        r"Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}\InprocServer32";
    /// Our own per-user key; `ClassicMenuCreated`=1 when we created `CLASSIC_KEY`, so
    /// uninstall only removes what it added.
    const HOPPY_KEY: &str = r"Software\Hoppy";
    const CLASSIC_MARKER: &str = "ClassicMenuCreated";

    fn add_send_verb(gui: &Path) -> Result<()> {
        let key = create_key(VERB_KEY)?;
        let label: Vec<u16> = verb_label().encode_utf16().collect();
        let icon: Vec<u16> = gui.as_os_str().encode_wide().collect();
        set_string(&key, None, &label)?;
        set_string(&key, Some("Icon"), &icon)?;
        // One process per selected file; `gather_send_files` merges them.
        let model: Vec<u16> = "Player".encode_utf16().collect();
        set_string(&key, Some("MultiSelectModel"), &model)?;
        let command = create_key(&format!(r"{VERB_KEY}\command"))?;
        let cmd: Vec<u16> = verb_command(gui).encode_wide().collect();
        set_string(&command, None, &cmd)
    }

    pub fn classic_menu() -> Option<bool> {
        (windows_build() >= 22000).then(|| key_exists(CLASSIC_SERVER))
    }

    pub fn set_classic_menu(on: bool) -> Result<bool> {
        if key_exists(CLASSIC_SERVER) == on {
            return Ok(false);
        }
        if on {
            let ours = !key_exists(CLASSIC_KEY);
            let key = create_key(CLASSIC_SERVER)?;
            set_string(&key, None, &[])?;
            if ours {
                set_dword(&create_key(HOPPY_KEY)?, CLASSIC_MARKER, 1)?;
            }
        } else if get_dword(HOPPY_KEY, CLASSIC_MARKER) == Some(1) {
            delete_tree(CLASSIC_KEY)?;
            delete_tree(HOPPY_KEY)?;
        } else {
            // Turned on by something else, off on the user's explicit request: remove only
            // the override itself.
            delete_tree(CLASSIC_SERVER)?;
        }
        diag::log(format!("classic context menu set to {on}"));
        Ok(true)
    }

    /// Ends Explorer (taskkill) and makes sure it comes back: Windows normally restarts the
    /// shell by itself; if the taskbar is not back within a few seconds, start it.
    pub fn restart_explorer() -> Result<()> {
        let root =
            PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()));
        let mut kill = Command::new(root.join(r"System32\taskkill.exe"));
        kill.args(["/f", "/im", "explorer.exe"]);
        super::no_window(&mut kill);
        let status = kill.status().context("running taskkill")?;
        diag::log(format!(
            "explorer restart: taskkill exit {:?}",
            status.code()
        ));
        let class = wide("Shell_TrayWnd");
        std::thread::sleep(Duration::from_millis(1500));
        for _ in 0..20 {
            // SAFETY: NUL-terminated class name alive for the call.
            if !unsafe { FindWindowW(class.as_ptr(), null()) }.is_null() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Command::new(root.join("explorer.exe"))
            .spawn()
            .context("starting explorer.exe")?;
        Ok(())
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
    fn create_shortcut(link: &Path, exe: &Path, args: &str, minimized: bool) -> Result<()> {
        let script = "$s=(New-Object -ComObject WScript.Shell).CreateShortcut($env:HOPPY_LNK);\
            $s.TargetPath=$env:HOPPY_EXE;$s.Arguments=$env:HOPPY_ARGS;$s.IconLocation=$env:HOPPY_EXE;\
            $s.WindowStyle=[int]$env:HOPPY_STYLE;$s.Description='Hoppy';$s.Save()";
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("HOPPY_LNK", link)
            .env("HOPPY_EXE", exe)
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
        let cli = std::env::current_exe()?;
        let gui = cli.with_file_name("hoppyw.exe");
        let send_to = send_to_link()?;
        let startup = startup_link()?;
        if gui.is_file() {
            create_shortcut(&send_to, &gui, "send", false)?;
            println!("✓ Added \"Send to → Hoppy\" ({})", send_to.display());
            create_shortcut(&startup, &gui, "", false)?;
            println!(
                "✓ Hoppy starts in the notification area at login ({})",
                startup.display()
            );
            add_send_verb(&gui)?;
            println!(
                "✓ Added \"{}\" to the right-click menu of files (HKCU\\{VERB_KEY}; Windows \
                 11: under \"Show more options\" unless `hoppy classic-menu on`)",
                verb_label()
            );
            println!("Start it now: run hoppyw.exe (Start menu → Hoppy).");
        } else {
            // Portable copy without hoppyw.exe: console fallback.
            create_shortcut(&send_to, &cli, "send --pause", false)?;
            println!(
                "✓ Added \"Send to → Hoppy\" ({}, console mode: hoppyw.exe not found)",
                send_to.display()
            );
            create_shortcut(&startup, &cli, "receive", true)?;
            println!(
                "✓ Receiver starts minimized at login ({})",
                startup.display()
            );
            println!("Start it now with: hoppy receive");
        }
        println!(
            "Windows may ask once to allow Hoppy through the firewall: choose Private networks \
             (or run `hoppy firewall --fix`)."
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
        if key_exists(VERB_KEY) {
            delete_tree(VERB_KEY)?;
            println!("✓ Removed \"{}\" from the right-click menu", verb_label());
        }
        // Only if Hoppy turned it on (a tweak made by the user or IT stays).
        if get_dword(HOPPY_KEY, CLASSIC_MARKER) == Some(1) {
            delete_tree(CLASSIC_KEY)?;
            println!("✓ Restored the Windows 11 context menu (after the next sign-in)");
        }
        delete_tree(HOPPY_KEY)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn path(units: &[u16]) -> PathBuf {
        PathBuf::from(String::from_utf16_lossy(units))
    }

    #[test]
    fn copy_wide_truncates_and_terminates() {
        let mut buf = [0xffffu16; 6];
        copy_wide(&mut buf, "Hoppy!!");
        assert_eq!(&buf[..5], w("Hoppy").as_slice());
        assert_eq!(buf[5], 0);
        // A surrogate pair is never split.
        let mut buf = [0xffffu16; 4];
        copy_wide(&mut buf, "ab🐰");
        assert_eq!(buf, [b'a' as u16, b'b' as u16, 0, 0]);
        let mut empty: [u16; 0] = [];
        copy_wide(&mut empty, "x");
    }

    #[test]
    fn multiselect_single_and_many() {
        let mut one = w(r"C:\Users\me\foto.jpg");
        one.extend([0, 0, 0]);
        assert_eq!(
            parse_multiselect(&one, path),
            vec![PathBuf::from(r"C:\Users\me\foto.jpg")]
        );

        let mut many = w(r"C:\dir");
        many.push(0);
        many.extend(w("a.txt"));
        many.push(0);
        many.extend(w("b 🐰.png"));
        many.extend([0, 0, 0, 0]);
        let dir = PathBuf::from(r"C:\dir");
        assert_eq!(
            parse_multiselect(&many, path),
            vec![dir.join("a.txt"), dir.join("b 🐰.png")]
        );
        assert!(parse_multiselect(&[0, 0], path).is_empty());
    }

    #[test]
    fn verb_command_quotes_the_exe_and_the_file() {
        let cmd = verb_command(Path::new(
            r"C:\Users\Zoë Rossi\AppData\Local\Programs\Hoppy\hoppyw.exe",
        ));
        assert_eq!(
            cmd,
            OsString::from(
                r#""C:\Users\Zoë Rossi\AppData\Local\Programs\Hoppy\hoppyw.exe" send "%1""#
            )
        );
    }

    #[test]
    fn collector_waits_while_files_keep_arriving() {
        let ms = Duration::from_millis;
        assert_eq!(collect_wait(ms(0), ms(0)), Some(COLLECT_QUIET));
        assert_eq!(collect_wait(ms(300), ms(100)), Some(ms(400)));
        assert_eq!(collect_wait(ms(600), ms(500)), None, "quiet long enough");
        // Files every 400 ms keep it open, but never past COLLECT_MAX.
        assert_eq!(collect_wait(ms(2800), ms(100)), Some(ms(200)));
        assert_eq!(collect_wait(COLLECT_MAX, ms(0)), None);
    }

    #[test]
    fn wide_lists_and_dedup() {
        let mut buf = w(r"C:\a.txt");
        buf.push(0);
        buf.push(0);
        buf.extend(w(r"C:\b 🐰.png"));
        buf.push(0);
        let items: Vec<PathBuf> = split_wide_list(&buf).into_iter().map(path).collect();
        assert_eq!(
            items,
            vec![PathBuf::from(r"C:\a.txt"), PathBuf::from(r"C:\b 🐰.png")]
        );
        assert!(split_wide_list(&[0, 0]).is_empty());
        let a = PathBuf::from("a");
        let b = PathBuf::from("b");
        assert_eq!(
            dedup_paths(vec![a.clone(), b.clone(), a.clone()]),
            vec![a, b]
        );
    }

    #[test]
    fn gather_is_a_no_op_off_windows() {
        if !cfg!(windows) {
            let files = vec![PathBuf::from("x")];
            assert_eq!(gather_send_files(files.clone()), Ok(Some(files)));
            let many = vec![PathBuf::from("x"); COLLECT_MAX_FILES + 1];
            assert_eq!(gather_send_files(many), Err(TooManyFiles));
            assert_eq!(classic_menu(), None);
        }
    }

    #[test]
    fn decodes_unaligned_utf16_le() {
        let bytes: Vec<u8> = w("a🐰").iter().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(decode_utf16_le(&bytes), w("a🐰"));
        assert_eq!(decode_utf16_le(&bytes[1..]).len(), (bytes.len() - 1) / 2);
        assert!(decode_utf16_le(&[7]).is_empty());
    }

    #[test]
    fn collected_stops_at_the_limits() {
        let mut c = Collected::default();
        c.add(10, vec![PathBuf::from("a"); COLLECT_MAX_FILES - 1]);
        c.add(10, vec![PathBuf::from("b")]);
        assert_eq!(
            (c.files.len(), c.bytes, c.overflow),
            (COLLECT_MAX_FILES, 20, false)
        );
        c.add(2, vec![PathBuf::from("c")]);
        assert!(c.overflow);
        c.add(0, Vec::new()); // nothing more once over
        assert_eq!(c.files.len(), COLLECT_MAX_FILES);

        let mut c = Collected::default();
        c.add(COLLECT_MAX_BYTES, vec![PathBuf::from("a")]);
        assert!(!c.overflow);
        c.add(1, vec![PathBuf::from("b")]);
        assert!(c.overflow);
        assert_eq!(c.files, vec![PathBuf::from("a")]);
    }

    #[test]
    fn compares_exe_paths_like_windows() {
        let same = |a: &str, b: &str| same_exe_path(Path::new(a), Path::new(b));
        assert!(same(
            r"C:\Users\U\AppData\Local\Programs\Hoppy\hoppyw.exe",
            r"c:\users\u\appdata\local\programs\hoppy\HOPPYW.EXE"
        ));
        assert!(same(r"\\?\C:\H\hoppyw.exe", r"C:\H\hoppyw.exe"));
        assert!(!same(r"C:\H\hoppyw.exe", r"C:\H\hoppy.exe"));
        assert!(!same(r"C:\Evil\hoppyw.exe", r"C:\H\hoppyw.exe"));
    }

    #[test]
    fn splits_https_urls() {
        assert_eq!(
            split_https_url("https://api.github.com/repos/Marketto/hoppy/releases/latest"),
            Some(("api.github.com", "/repos/Marketto/hoppy/releases/latest"))
        );
        assert_eq!(
            split_https_url("https://example.org"),
            Some(("example.org", "/"))
        );
        for bad in [
            "http://example.org/",
            "https:///x",
            "https://user:pw@host/", // user info (no "x@y.z": the secrets scan)
            "https://example.org:8443/",
            "https://example.org?x",
            "ftp://example.org/",
        ] {
            assert_eq!(split_https_url(bad), None, "{bad}");
        }
    }
}
