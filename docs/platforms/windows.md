# Windows

## PoC experience
- Install: `Hoppy-Setup-x64.exe` (per user, no admin; only the optional firewall rules ask
  for administrator approval), or `apps/windows/install.ps1` next to
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
  sending* (check mark = on), *Quit*. When an update is available, a bold *Update to X.Y.Z…*
  comes first.
- Updates (tray only, `crates/hoppy-cli/src/update.rs`): 30 s after the tray starts and then
  once a day it asks `api.github.com/repos/Marketto/hoppy/releases/latest` (WinHTTP, system
  proxy, User-Agent `Hoppy/<version>`); network errors are silent. If the release is newer
  (semver; pre-releases are never offered) a notification says "Hoppy X.Y.Z is available —
  click to update". Clicking it (or the menu item) downloads `Hoppy-Setup-x64.exe` into a
  fresh private folder `%TEMP%\Hoppy-update-<random>\Hoppy-Setup-X.Y.Z.exe` (progress in the
  tooltip), hashing it while it is written as `*.part` and renaming it only if the SHA-256
  matches the release's `SHA256SUMS.txt` (mismatch or error: notification, folder removed,
  nothing installed), runs it at once with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART
  /UPDATE=1` (no window, so no Cancel) and quits; the installer restarts the tray, also when
  setup fails (`DeinitializeSetup`). `Hoppy-update-*` folders older than a day are removed
  when the tray starts. The installer is unsigned for now (docs/release.md "Trust model"). Only copies installed by the setup (`%LOCALAPPDATA%\Programs\Hoppy`) update
  themselves; portable copies elsewhere get a notification/menu item that opens the
  download portal. Dev builds (debug, or a `-dev` version) never check.
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
- Firewall: see [Firewall](#firewall) below.

## Firewall

Symptom when it goes wrong: the phone *finds* the PC (discovery replies are outbound) but
the transfer fails with a connect timeout to port 47471.

- **Rules** (`crates/hoppy-cli/src/firewall.rs`): inbound *allow*, tied to the program path,
  profiles **Private + Domain**, never Public (v0 is unencrypted, ADR 0003; Domain is
  included because PCs on a company network usually sit on the Domain profile):
  `Hoppy (hoppyw)` for `hoppyw.exe` and `Hoppy (hoppy)` for `hoppy.exe`, each TCP 47471
  (transfer) + UDP 47470 (discovery). `hoppy receive --port N` with another port is not
  covered. Adding them first deletes every inbound rule for the same program paths: our old
  ones, and the *block* rules Windows creates when its "allow access?" prompt is
  dismissed (a block rule beats an allow rule). Deletion always filters by program path, so
  another Windows user's install on the same PC (same rule names, other path) is untouched.
- **Installer**: task *Allow Hoppy through Windows Firewall* (checked by default). After the
  files are copied, `hoppyw firewall --check` (no admin) decides; if Hoppy is not already
  allowed, `hoppyw firewall --apply` runs hidden through UAC. Declining UAC just skips it.
  Tray updates (`/UPDATE=1`) skip the task (paths never change). Silent IT deployments can
  skip it with `/MERGETASKS="!firewall"`. Uninstall removes the rules (UAC) only if rules
  for the install folder exist.
- **Tray**: at start it checks in the background (PowerShell `Get-NetFirewallRule` on the
  effective store + `Get-NetConnectionProfile`, no admin, silent on errors). If no
  connected network lets `hoppyw.exe` in: on a Private/Domain network a balloon "Other
  devices may not be able to send you files" (click → UAC → rules) and the menu item
  *Allow through firewall…*; on a Public-only network "This network is set to Public" (click
  → Settings → Network status, set it to Private) and *Set the network to Private…*. The
  balloon appears once per Hoppy version (`firewall_hint` in `device.json`); the menu item
  stays while the problem does (re-checked when the menu opens, at most once a minute).
- **CLI**: `hoppy firewall` prints the connected network categories and, for each exe, the
  profiles where it is allowed/blocked; `hoppy firewall --fix` adds the rules (UAC);
  `hoppy firewall --remove` (administrator terminal) deletes them.
- **Company PCs**: a Group Policy can set "Apply local firewall rules: No"; local rules
  (ours and the prompt's) are then ignored. The tray reports "the rules were added but have
  no effect"; IT must allow `hoppyw.exe` (TCP 47471, UDP 47470) by policy. Third-party
  security suites with their own firewall are not detected.
- By hand (admin terminal):
  `netsh advfirewall firewall add rule name="Hoppy (hoppyw)" dir=in action=allow program="<path>\hoppyw.exe" profile=private,domain protocol=TCP localport=47471`
  (and the same with `protocol=UDP localport=47470`).

## Known limits
- Windows 11 hides "Send to" under "Show more options". A top-level entry needs an
  `IExplorerCommand` shell extension registered by a signed MSIX package (roadmap).
- The Windows 11 **Share** sheet ("Condividi") only lists packaged apps with a share
  target, so Hoppy needs an MSIX package to appear there (roadmap).
- Accept dialog is a plain `MessageBoxW`; a themed picker/accept window is planned.
- Network profile "Public" blocks Hoppy (our rules exclude it on purpose): set the Wi-Fi to
  Private; the tray says so.
