# Windows

## PoC experience
- Install: `Hoppy-Setup-x64.exe` (per user, no admin), or `apps/windows/install.ps1` next to
  `hoppy.exe` + `hoppyw.exe` from the portable zip, or `hoppy install`. It adds
  **Send to → Hoppy** and starts the Hoppy icon in the notification area at login.
- Two binaries: `hoppyw.exe` (GUI, never opens a console) is what users see; `hoppy.exe` is
  the CLI for scripts and tests. Without `hoppyw.exe`, `hoppy install` falls back to the
  console flow (`hoppy send --pause`, `hoppy receive` minimized).
- Receive: the tray icon (tooltip "Hoppy · *device name*") means Hoppy is ready. A Yes/No
  dialog appears for each incoming transfer (default button **No**, so a stray Enter does
  not accept); while it is open, further offers are declined. It shows file names as they
  will be saved (§4 sanitizer) and strips control/bidi characters from names. Files go to
  `Downloads\Hoppy`; a notification says what arrived and opens the folder when clicked.
  `hoppy receive --yes` (CLI) skips the question (trusted networks only).
- Tray menu (left or right click): *Send files…*, *Open received folder*, *♪ Music while
  sending* (check mark = on), *Quit*.
- Installer upgrades/uninstalls stop only the `hoppy.exe`/`hoppyw.exe` running from the
  install folder (PowerShell, path-filtered), not same-named programs elsewhere.
- Send: right click files → *Send to* → *Hoppy* (Windows 11: *Show more options* first, or
  Shift+right-click) → a menu at the mouse lists nearby devices → click one. Progress is in
  the tray icon tooltip; a notification confirms delivery (or the refusal).
- Music (only on the sender): the *Send to* flow plays a short 16-bit loop while you pick
  a device, a faster one once the other side accepts, and a jingle when delivered. Turn it
  off from the tray menu (*♪ Music while sending*) or with `hoppy music off`
  (stored as `"music"` in `%APPDATA%\hoppy\device.json`). The tray receiver, `hoppy send`
  and `hoppy receive` never play music. The installer wizard plays music too (checkbox
  *♪ Music* bottom-left; silent installs stay silent).
- Strings follow the Windows display language: Italian, otherwise English.
- Firewall: the first receive triggers the Windows prompt; allow **Private networks**. The
  rule is per executable, so upgrading from 0.1.0 (`hoppy.exe receive`) asks again for
  `hoppyw.exe`. If missed (admin):
  `netsh advfirewall firewall add rule name="Hoppy" dir=in action=allow program="<path>\hoppyw.exe" profile=private`.

## Known limits
- Windows 11 hides "Send to" under "Show more options". A top-level entry needs an
  `IExplorerCommand` shell extension registered by a signed MSIX package (roadmap).
- The Windows 11 **Share** sheet ("Condividi") only lists packaged apps with a share
  target, so Hoppy needs an MSIX package to appear there (roadmap).
- Accept dialog is a plain `MessageBoxW`; a themed picker/accept window is planned.
- Network profile "Public" blocks discovery on many PCs: set the Wi-Fi to Private.
