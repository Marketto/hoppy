//! OS integration. Windows: "Send to" + startup shortcuts, native dialogs and the
//! notification-area (tray) icon used by `hoppyw`. Other desktops: stdin prompt; shell
//! integration is documented in docs/platforms/.
//!
//! All Win32 FFI lives in `win` below behind `#[cfg(windows)]`; only the windows-latest CI
//! job compiles it. Non-Windows builds get a stub `win` with the same safe API whose
//! `Tray::new` fails, so `gui.rs` is compiled and linted on every OS.

use std::path::PathBuf;

use anyhow::Result;

/// Asks the user whether to accept an incoming transfer.
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

/// UI language for `hoppyw` strings: the Windows display language (LANG elsewhere).
pub fn ui_is_italian() -> bool {
    #[cfg(windows)]
    {
        win::ui_language_is_italian()
    }
    #[cfg(not(windows))]
    {
        std::env::var("LANG").is_ok_and(|l| l.starts_with("it"))
    }
}

pub fn install() -> Result<()> {
    #[cfg(windows)]
    {
        win::install()
    }
    #[cfg(not(windows))]
    {
        anyhow::bail!("`hoppy install` is Windows-only for now; see docs/platforms/linux.md")
    }
}

pub fn uninstall() -> Result<()> {
    #[cfg(windows)]
    {
        win::uninstall()
    }
    #[cfg(not(windows))]
    {
        anyhow::bail!("`hoppy uninstall` is Windows-only for now")
    }
}

/// Something the tray window reported to the `hoppyw` event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Left/right click (or keyboard select) on the notification-area icon.
    Click,
    /// The user clicked the last balloon/toast.
    BalloonClick,
    /// A worker thread called `Waker::wake`: check the shared state.
    Wake,
    /// A second `hoppyw` launch asked the running one to show itself.
    Reopen,
    /// A timer set with `Tray::set_timer` fired.
    Timer(usize),
}

/// One entry of a popup menu. Ids must be non-zero (0 means "dismissed").
#[derive(Debug, Clone, Copy)]
pub enum MenuItem<'a> {
    Action(u32, &'a str),
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

    use anyhow::{Context, Result, bail};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT,
        POINT, WPARAM,
    };
    use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    use windows_sys::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIM_ADD,
        NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_BALLOONUSERCLICK, NIN_SELECT,
        NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW, ShellExecuteW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
        DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW, GetSystemMetrics,
        HICON, IDI_APPLICATION, IDYES, IMAGE_ICON, KillTimer, LR_DEFAULTCOLOR, LoadIconW,
        LoadImageW, MB_ICONERROR, MB_ICONQUESTION, MB_OK, MB_SETFOREGROUND, MB_SYSTEMMODAL,
        MB_TOPMOST, MB_YESNO, MF_DISABLED, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, PostMessageW,
        RegisterClassExW, RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON, SW_SHOWNORMAL,
        SetForegroundWindow, SetProcessDPIAware, SetTimer, TPM_BOTTOMALIGN, TPM_NONOTIFY,
        TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_CONTEXTMENU,
        WM_NULL, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
    };

    use super::{MenuItem, TrayEvent, copy_wide, parse_multiselect};

    const WM_TRAY: u32 = WM_APP + 1;
    const WM_WAKE: u32 = WM_APP + 2;
    const WM_REOPEN: u32 = WM_APP + 3;
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
        Event(TrayEvent),
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

    pub fn ui_language_is_italian() -> bool {
        // SAFETY: no arguments, no side effects.
        let lang = unsafe { GetUserDefaultUILanguage() };
        lang & 0x3ff == LANG_ITALIAN
    }

    pub fn message_box_yes_no(title: &str, body: &str) -> bool {
        let (title, body) = (wide(title), wide(body));
        let style = MB_YESNO | MB_ICONQUESTION | MB_SYSTEMMODAL | MB_SETFOREGROUND;
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
        let (verb, file) = (wide("open"), wide_path(dir));
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
        r as isize > 32
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
        // NOTIFYICON_VERSION_4: LOWORD(lParam) is the notification event.
        let raw = match msg {
            WM_TRAY => match (lparam as u32) & 0xffff {
                NIN_SELECT | NIN_KEYSELECT | WM_CONTEXTMENU => Some(Raw::Event(TrayEvent::Click)),
                NIN_BALLOONUSERCLICK => Some(Raw::Event(TrayEvent::BalloonClick)),
                _ => None,
            },
            WM_WAKE => Some(Raw::Event(TrayEvent::Wake)),
            WM_REOPEN => Some(Raw::Event(TrayEvent::Reopen)),
            WM_TIMER => Some(Raw::Event(TrayEvent::Timer(wparam))),
            m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => Some(Raw::Readd),
            _ => None,
        };
        match raw {
            Some(raw) => {
                QUEUE.with(|q| q.borrow_mut().push_back(raw));
                0
            }
            // SAFETY: forwards the unmodified arguments Windows passed to us.
            None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    /// Lets worker threads nudge the UI thread (`TrayEvent::Wake`).
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
            // SAFETY: null = this executable's module.
            let hinstance: HINSTANCE = unsafe { GetModuleHandleW(null()) };
            let (class, taskbar, title) =
                (wide(CLASS_NAME), wide("TaskbarCreated"), wide(window_title));
            // SAFETY: NUL-terminated buffer alive for the call.
            let msg = unsafe { RegisterWindowMessageW(taskbar.as_ptr()) };
            TASKBAR_CREATED.store(msg, Ordering::Relaxed);
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            // SAFETY: `wc` and the class name are alive for the call; registering twice in
            // one process fails harmlessly and the first registration is used.
            unsafe { RegisterClassExW(&wc) };
            // SAFETY: class registered above; strings alive for the call. Never shown; a
            // top-level (not message-only) window so it receives "TaskbarCreated".
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
                bail!("cannot create the notification-area window");
            }
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
            unsafe {
                Shell_NotifyIconW(NIM_ADD, &nid);
                Shell_NotifyIconW(NIM_SETVERSION, &nid);
            }
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
        pub fn balloon(&self, title: &str, text: &str, error: bool) {
            let mut nid = self.nid();
            nid.uFlags = NIF_INFO;
            copy_wide(&mut nid.szInfoTitle, title);
            copy_wide(&mut nid.szInfo, if text.is_empty() { " " } else { text });
            nid.dwInfoFlags = if error { NIIF_ERROR } else { NIIF_INFO };
            // SAFETY: `nid` is fully initialised with the right cbSize.
            unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
        }

        pub fn waker(&self) -> Waker {
            Waker(self.hwnd as usize)
        }

        pub fn set_timer(&self, id: usize, ms: u32) {
            // SAFETY: live window; no callback, so WM_TIMER is posted to `wndproc`.
            unsafe { SetTimer(self.hwnd, id, ms, None) };
        }

        pub fn kill_timer(&self, id: usize) {
            // SAFETY: live window; unknown ids just fail.
            unsafe { KillTimer(self.hwnd, id) };
        }

        /// Shows a popup menu at the mouse cursor and returns the chosen id
        /// (`None` = dismissed). `above` anchors the menu's bottom edge (taskbar menus).
        pub fn popup_menu(&self, items: &[MenuItem<'_>], above: bool) -> Option<u32> {
            // SAFETY: no arguments.
            let menu = unsafe { CreatePopupMenu() };
            if menu.is_null() {
                return None;
            }
            for item in items {
                let (flags, id, text) = match *item {
                    MenuItem::Action(id, t) => (MF_STRING, id as usize, Some(t)),
                    MenuItem::Label(t) => (MF_STRING | MF_DISABLED, 0, Some(t)),
                    MenuItem::Separator => (MF_SEPARATOR, 0, None),
                };
                // '&' marks a keyboard accelerator in menus; names must show it literally.
                let text = text.map(|t| wide(&t.replace('&', "&&")));
                let ptr = text.as_ref().map_or(null(), |t| t.as_ptr());
                // SAFETY: live menu; AppendMenuW copies the string.
                unsafe { AppendMenuW(menu, flags, id, ptr) };
            }
            let mut flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON;
            if above {
                flags |= TPM_BOTTOMALIGN;
            }
            let mut pt = POINT { x: 0, y: 0 };
            // SAFETY: valid out-pointer, live window and menu. SetForegroundWindow before and
            // WM_NULL after is the documented way to make a tray menu close on outside clicks.
            let cmd = unsafe {
                GetCursorPos(&mut pt);
                SetForegroundWindow(self.hwnd);
                let cmd = TrackPopupMenu(menu, flags, pt.x, pt.y, 0, self.hwnd, null());
                PostMessageW(self.hwnd, WM_NULL, 0, 0);
                DestroyMenu(menu);
                cmd
            };
            u32::try_from(cmd).ok().filter(|&c| c != 0)
        }

        /// Native "Open" dialog with multi-select; empty if cancelled.
        pub fn pick_files(&self, title: &str) -> Vec<PathBuf> {
            let mut buf = vec![0u16; 64 * 1024];
            let title = wide(title);
            let mut ofn = OPENFILENAMEW {
                lStructSize: size_of::<OPENFILENAMEW>() as u32,
                hwndOwner: self.hwnd,
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
            // SAFETY: `buf` and `title` outlive the modal call; sizes match the buffers.
            let ok = unsafe {
                SetForegroundWindow(self.hwnd);
                GetOpenFileNameW(&mut ofn)
            };
            if ok == 0 {
                return Vec::new();
            }
            parse_multiselect(&buf, |w| PathBuf::from(OsString::from_wide(w)))
        }

        /// Pumps messages until the window reports something; `None` on WM_QUIT.
        pub fn next_event(&self) -> Option<TrayEvent> {
            loop {
                while let Some(raw) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
                    match raw {
                        Raw::Event(e) => return Some(e),
                        Raw::Readd => self.add(),
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

/// Same API as the Windows module; `Tray::new` fails, so nothing else is reachable.
#[cfg(not(windows))]
pub mod win {
    use std::convert::Infallible;
    use std::path::{Path, PathBuf};

    use anyhow::{Result, bail};

    use super::{MenuItem, TrayEvent};

    pub struct InstanceGuard;

    pub fn single_instance(_name: &str) -> Option<InstanceGuard> {
        Some(InstanceGuard)
    }

    pub fn signal_existing(_window_title: &str) -> bool {
        false
    }

    pub fn message_box_error(title: &str, body: &str) {
        eprintln!("{title}: {body}");
    }

    pub fn open_folder(_dir: &Path) -> bool {
        false
    }

    #[derive(Clone)]
    pub struct Waker(Infallible);

    impl Waker {
        pub fn wake(&self) {
            match self.0 {}
        }
    }

    pub struct Tray(Infallible);

    impl Tray {
        pub fn new(_window_title: &str, _tip: &str) -> Result<Tray> {
            bail!("the Hoppy tray app is Windows-only; use the `hoppy` CLI here")
        }
        pub fn set_tip(&self, _tip: &str) {
            match self.0 {}
        }
        pub fn balloon(&self, _title: &str, _text: &str, _error: bool) {
            match self.0 {}
        }
        pub fn waker(&self) -> Waker {
            match self.0 {}
        }
        pub fn set_timer(&self, _id: usize, _ms: u32) {
            match self.0 {}
        }
        pub fn kill_timer(&self, _id: usize) {
            match self.0 {}
        }
        pub fn popup_menu(&self, _items: &[MenuItem<'_>], _above: bool) -> Option<u32> {
            match self.0 {}
        }
        pub fn pick_files(&self, _title: &str) -> Vec<PathBuf> {
            match self.0 {}
        }
        pub fn next_event(&self) -> Option<TrayEvent> {
            match self.0 {}
        }
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
}
