# Windows

## PoC experience
- Install: `Hoppy-Setup-x64.exe` (per user, no admin; only the optional firewall rules ask
  for administrator approval), or `apps/windows/install.ps1` next to
  `hoppy.exe` + `hoppyw.exe` from the portable zip, or `hoppy install`. It adds the
  right-click entry **Send with Hoppy** (see [Right-click send](#right-click-send)),
  **Send to → Hoppy**, and starts the Hoppy icon in the notification area at login.
- Two binaries: `hoppyw.exe` (GUI, never opens a console) is what users see; `hoppy.exe` is
  the CLI for scripts and tests. Without `hoppyw.exe`, `hoppy install` falls back to the
  console flow (`hoppy send --pause`, `hoppy receive` minimized).
- Receive: the tray icon (tooltip "Hoppy · *device name*") means Hoppy is ready. A Hoppy
  dialog appears for each incoming transfer: who sends what, how the connection is
  protected and, for a device not verified yet, the 4-emoji code to compare (see
  [Device verification](#device-verification)); buttons **Decline** (default, so a stray
  Enter or Esc does not accept), **Accept** and, with a code, **They match, accept**. No
  answer within 110 s = declined; while it is open, further offers are declined. It shows
  file names as they will be saved (§4 sanitizer) and strips control/bidi characters from
  names. Files go to `Downloads\Hoppy`; a notification says what arrived and opens the
  folder when clicked. `hoppy receive --yes` (CLI) skips the question (trusted networks
  only).
- Tray menu (left or right click; it opens at the point Windows reports for the click):
  *Send files…*, *Open received folder*, *♪ Sound when delivered* (check mark = on), on
  Windows 11 *"Send with Hoppy" directly on right-click (classic menu)* (check mark = on),
  *Trusted devices…*, *About & diagnostics…*, *Quit*. When an update is available, a bold
  *Update to X.Y.Z…* comes first.
- The tray's UI thread never does file, registry or network I/O: `device.json` (roaming
  AppData, often redirected to a network share on company PCs) is cached and re-read by a
  worker whenever the menu opens; opening folders, starting processes, UAC prompts and
  settings writes run on worker threads. See [Diagnostics](#diagnostics).
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
- Send: right-click one or more files → *Send with Hoppy* (or *Send to* → *Hoppy*, or drop
  them on the desktop icon) → a menu at the mouse lists nearby devices (✓ verified,
  🔒 encrypted, "⚠ not encrypted" for old apps) → click one. For a device not verified
  yet a small window shows the same code as the receiver's dialog, with **They match**.
  Progress is in the tray icon tooltip; a notification confirms delivery (or the refusal).
  Folders are refused with a message (not supported yet).
- Sound (only on the sender): the *Send to* flow is silent while it searches, shows the
  device menu and sends; it plays one short jingle when the files are delivered (instead
  of the system notification sound). Turn it off from the tray menu (*♪ Sound when
  delivered*) or with `hoppy music off` (stored as `"music"` in
  `%APPDATA%\hoppy\device.json`, name kept for compatibility). The tray receiver keeps only
  the system notification sound of its balloon; `hoppy send` and `hoppy receive` are silent. The installer wizard plays music too (checkbox
  *♪ Music* bottom-left; silent installs stay silent).
- Strings follow the Windows display language: Italian, otherwise English.
- Firewall: see [Firewall](#firewall) below.

## Right-click send

All per user (HKCU, no admin), no Store/MSIX:

- **Verb** (`hoppy install`, run by the installer on every install and update; removed by
  `hoppy uninstall`): `HKCU\Software\Classes\*\shell\Hoppy`, default value *Invia con
  Hoppy* / *Send with Hoppy* (display language at install time), `Icon` = `hoppyw.exe`,
  `MultiSelectModel` = `Player`, `command` = `"<install dir>\hoppyw.exe" send "%1"`. Only
  added when `hoppyw.exe` is present. Files only (`*`), not folders.
- **Many files**: Explorer starts one `hoppyw send FILE` per selected file. The first
  instance takes the named mutex `Local\HoppySendCollector`, opens a hidden window of class
  `HoppySendCollector` and waits 500 ms after the last file arrived (3 s at most); the
  others find the window, pass their paths with `WM_COPYDATA` (tag `HOPP`, NUL-separated
  UTF-16, ≤ 4 MB per message; `wParam` = a hidden `HoppySendForwarder` window of the
  sender), allow it to take the foreground and exit. The collector ignores messages whose
  `wParam` window does not belong to a process running its own executable
  (`QueryFullProcessImageNameW`, case-insensitive; this keeps out unrelated programs, not a
  same-user process that passes one of our windows). More than 10 000 files (the protocol
  limit) or 16 MB of paths in total = "Too many files selected". Late arrivals (after 3 s)
  start a second send. Same flow for Send to / drag & drop (one process with every file).
- **Windows 11 classic menu** (opt-in): the new context menu shows classic verbs only under
  *Show more options* (or Shift+right-click). An empty default value in
  `HKCU\Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}\InprocServer32`
  brings back the full classic menu for this user, so *Send with Hoppy* is one click away.
  Set by the installer task (checked by default, Windows 11 only, not on tray updates), the
  tray menu toggle, or `hoppy classic-menu on|off`. Explorer reads it at start: the
  installer and the tray ask (Yes/No, default No) before restarting Explorer (`taskkill /f
  /im explorer.exe`; started again if Windows does not do it within ~6 s); otherwise it
  applies at the next sign-in. When Hoppy created the key it sets
  `HKCU\Software\Hoppy\ClassicMenuCreated` = 1, and uninstall removes the key only then.
  **This changes the whole Windows 11 context menu for that user**, not just Hoppy's entry:
  every app's verbs appear in the old menu and the new one is gone. Opt out by unchecking
  the installer task, or later with the tray toggle / `hoppy classic-menu off`.
- **Desktop shortcut** *Hoppy* (installer task, checked by default) → `hoppyw.exe send`:
  drop files on it, or double-click to choose them in the Open dialog.
- *Send to → Hoppy* stays (`%APPDATA%\Microsoft\Windows\SendTo\Hoppy.lnk`).

## Diagnostics

`hoppyw` writes `%LOCALAPPDATA%\Hoppy\hoppy.log` (local, never roamed; one backup
`hoppy.log.1` once it reaches 256 KB). Lines carry a UTC timestamp, the process id and role
(`tray`, `send`). Never logged: file names, paths (anything path-like in an error text
becomes `<path>`) and contents; hoppy-core errors only by category (`I/O error (NotFound)`,
`remote error`, …), since their texts can carry received names. May be logged on
purpose, for network diagnosis: local and peer IP addresses and device names. Logged: start/exit, file
counts and sizes, tray callbacks (event,
`wParam`/`lParam`, delivery = posted/sent/notify; mouse moves skipped), events handled
≥ 200 ms after they arrived, menus shown (position, anchor or cursor, foreground) and their
result, UI handlers taking ≥ 200 ms, a watchdog line when one runs > 2 s, and worker errors
(update check/download, firewall check/fix, receive, send, settings, classic menu). Open it
from the tray (*About & diagnostics…* → Notepad) or print it with `hoppy log [--lines N]`.

Tray clicks answered minutes late (up to 0.2.1): Explorer delivers tray callbacks with
`SendNotifyMessage`. `GetMessageW` runs the window procedure for such *sent* messages
internally and keeps waiting for a *posted* one, so an event only queued by the window
procedure waited for some unrelated message; the menu then opened wherever the mouse was.
The window procedure now posts itself a message whenever it queues an event.

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
- **Without rules: callback.** If the direct connection to TCP 47471 fails within 3 s, the
  sender asks this PC over UDP to connect back to it (spec §6, ADR 0006), so a PC whose
  inbound is blocked (company PC, UAC declined, no admin) can still receive, as long as
  outbound TCP to the LAN is allowed and Windows' "allow unicast response to
  multicast/broadcast" window (~3 s after each of our announces) is on, which it is by
  default. The accept dialog is the same (one offer at a time across both paths). It
  fails if a GPO sets `DisableUnicastResponsesToMulticastBroadcast` (Group Policy
  "Allow unicast response: No"), if both PCs block inbound, or across subnets. Sending
  side: `hoppy send` / `hoppyw send` say "… is not answering, trying another way…" and
  wait up to 20 s; then "Can't reach …" with the firewall hint. `hoppy send --no-direct`
  forces the callback (debugging).
  Senders keep their discovery socket on an ephemeral port (unicast replies must reach
  them, and the tray must keep getting the unicast sent to 47470); only while a callback
  is pending they open a second, listen-only socket on UDP 47470 (address reuse, joined to
  the group) to hear the receiver's announces and reply at once. On Windows that socket
  cannot be bound to the group address, so for those ≤ 20 s a unicast datagram meant for
  the tray on the same PC could land in the sender instead (the remote side re-sends).
- By hand (admin terminal):
  `netsh advfirewall firewall add rule name="Hoppy (hoppyw)" dir=in action=allow program="<path>\hoppyw.exe" profile=private,domain protocol=TCP localport=47471`
  (and the same with `protocol=UDP localport=47470`).

## Device verification

Protocol v1 (docs/protocol.md §7, ADR 0010) encrypts every session with a v1 peer and
remembers device keys. The dialogs are a small window of Hoppy's own
(`platform::win::show_dialog`, `crates/hoppy-cli/src/platform.rs`), described by
`verify::DialogView` (shared with Linux and macOS):

- **Code**: the 4 emoji in *Segoe UI Emoji* (GDI draws them in one colour, so each has its
  name under it, Italian or English). That is the only form of the code: people compare the
  4 emoji by eye with the other screen (no digits, no QR code). Screen readers read the
  names as text.
- **Receiver** (tray): the accept dialog above. *They match, accept* = accept + mark the
  sender verified.
- **Sender** (`hoppyw send`): after the handshake with a New/Known device, a window with the
  same code, **Later** (default; Italian *Più tardi*) and **They match** (marks the receiver
  verified); it closes by itself when the transfer ends. The tray tooltip says "Waiting for X
  to accept… code 🐘🐢🐙🦋".
- **Key changed** (a remembered device answers with another key): the sender refuses before
  the offer, the receiver declines without asking (spec §7.5); both show a warning dialog
  with **Close** (default) and **Forget device** (the next contact is a new device).
- **Trusted devices…** (tray menu): remembered devices (✓ = verified, start of the key
  fingerprint to tell same-named devices apart); picking one asks "Forget X?".
- Warnings as notifications: a new device using the name of a known one (in the accept
  dialog too), sessions abandoned mid-handshake (someone may be guessing codes; at most one
  a minute), a damaged `trust.json` set aside, a trust change that could not be saved.

The dialog runs its own message loop on a worker thread (never the tray thread), Topmost,
centred on the work area, `IsDialogMessageW` for Tab / Enter (= default button) / Esc
(= closed = default answer); it polls its `DialogLive` (close) every 200 ms.

## Known limits
- Windows 11's new (compact) context menu only lists `IExplorerCommand` handlers from a
  signed MSIX package (roadmap); until then *Send with Hoppy* is there only with the
  classic-menu option, otherwise under "Show more options".
- The Windows 11 **Share** sheet ("Condividi") only lists packaged apps with a share
  target, so Hoppy needs an MSIX package to appear there (roadmap).
- Emoji in the dialogs are monochrome (GDI); the names under them keep look-alikes apart.
- Network profile "Public" blocks Hoppy (our rules exclude it on purpose): set the Wi-Fi to
  Private; the tray says so.
