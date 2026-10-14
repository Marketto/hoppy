# Windows

## PoC experience
- Install: run `apps/windows/install.ps1` next to `hoppy.exe` (or `hoppy install`).
  It adds **Send to → Hoppy** and starts `hoppy receive` minimized at login.
- Send: right click files → *Send to* → *Hoppy* (Windows 11: *Show more options* first) →
  console lists nearby devices → type the number.
- Receive: a Yes/No dialog appears for each incoming transfer; files go to
  `Downloads\Hoppy`. `hoppy receive --yes` skips the question (trusted networks only).
- Firewall: the first `receive` triggers the Windows prompt; allow **Private networks**.
  If missed: `netsh advfirewall firewall add rule name="Hoppy" dir=in action=allow program="<path>\hoppy.exe" profile=private` (admin).

## Known limits
- Windows 11 hides "Send to" under "Show more options". A top-level entry needs an
  `IExplorerCommand` shell extension registered by a signed MSIX package (roadmap).
- Console UI is temporary; the planned picker window uses the theme palette.
- Network profile "Public" blocks discovery on many PCs: set the Wi-Fi to Private.
