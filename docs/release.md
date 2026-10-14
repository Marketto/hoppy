# Releases and the download portal

## What gets published

| Platform | Status | Release asset (stable name) | Built by |
| --- | --- | --- | --- |
| Windows 10/11 x64 | **live** | `Hoppy-Setup-x64.exe` (Inno Setup, per-user, adds Send To), `Hoppy-windows-x64.zip` (portable) | `release.yml` → `windows` |
| Android 10+ (phone + tablet, one universal APK) | **live** | `Hoppy-android.apk` | `release.yml` → `android` |
| macOS 12+ — Intel Macs from 2015 and Apple silicon | gated (`ENABLE_MACOS`) | `Hoppy-macos-universal.dmg` (x86_64 + arm64 via `lipo`) | `release.yml` → `macos` |
| Ubuntu 22.04+ x64 | gated (`ENABLE_LINUX`) | `hoppy_amd64.deb` (`cargo-deb`) | `release.yml` → `linux` |
| iPhone / iPad from 2021 | gated (`ENABLE_IOS`) | TestFlight / App Store link (no app yet) | `release.yml` → `ios` (placeholder) |

Every release also carries `SHA256SUMS.txt` (`sha256sum` output: `<hex>  <name>` per line).
Asset names never contain the version, so
`https://github.com/<owner>/<repo>/releases/latest/download/<asset>` always points at the
newest build — the portal relies on this, and so does the **Android in-app updater**
(`update/Updater.kt`, docs/platforms/android.md "Updates"): it reads `tag_name` from the
`releases/latest` API (must be semver, e.g. `v0.2.0`), downloads `Hoppy-android.apk` and
refuses it unless its SHA-256 matches the `Hoppy-android.apk` line of `SHA256SUMS.txt`.
Renaming either asset, publishing an APK not listed in the sums, or changing the signing
key breaks updates for every installed app. Pre-releases and drafts are not offered
(`releases/latest` skips them).

The **Windows tray updater** (`crates/hoppy-cli/src/update.rs`, docs/platforms/windows.md
"Updates") relies on the same contract: it reads `tag_name` (`vX.Y.Z`) from
`releases/latest`, downloads `Hoppy-Setup-x64.exe` and `SHA256SUMS.txt` from
`releases/download/<tag>/`, refuses the installer unless its SHA-256 matches the
`Hoppy-Setup-x64.exe` line, then runs it with `/SILENT /SUPPRESSMSGBOXES /NORESTART
/UPDATE=1` (`hoppy.iss` relaunches the tray when it sees `/UPDATE=1`). A release without both
assets is not offered. **Never rename `Hoppy-Setup-x64.exe` or `SHA256SUMS.txt`**, never drop
the installer from the sums, and keep tags `v` + plain `X.Y.Z`.

## Cutting a release

```
git tag v0.2.0 && git push origin v0.2.0        # or Actions → Release → Run workflow
```
`release.yml` stamps the version into Cargo (Windows `hoppy.exe` + `hoppyw.exe`, installer) and Android
(`versionName`, `versionCode` = run number), builds every enabled platform, then creates the
GitHub Release. Use **Run workflow** with *publish* unticked to build packages without
publishing (they appear as workflow artifacts).

## Unlocking a platform

1. Make the platform's build real (macOS: signed `.app` in the `.dmg`; iOS: Xcode project).
2. Set the repository variable (`Settings → Secrets and variables → Actions → Variables`):
   `ENABLE_MACOS`, `ENABLE_LINUX` or `ENABLE_IOS` = `true`.
3. Cut a release. The portal shows a download as soon as the latest release contains the
   platform's asset; until then it says "Coming soon". For iOS, add the store link in
   `site/app.js` (`PLATFORMS`, `ios`).

## Signing (secrets only — never committed)

- **Android** (needed so each release can update the previous one in place). Create once,
  keep the `.jks` and passwords safe (losing them means users must uninstall to update):
  ```
  keytool -genkeypair -v -keystore hoppy-release.jks -alias hoppy -keyalg RSA -keysize 4096 -validity 10000
  base64 -w0 hoppy-release.jks   # → secret ANDROID_KEYSTORE_BASE64
  ```
  Secrets: `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`,
  `ANDROID_KEY_PASSWORD`. Without them the APK is signed with a throwaway debug key (a
  workflow warning says so). The workflow ignores whitespace in the base64 secret (handy
  when pasting from a phone), fails early with a clear error if the keystore can't be opened,
  and prints the public certificate SHA-256 as a notice so you can check the right key signed.
- **Windows**: the installer is unsigned, so SmartScreen warns ("More info → Run anyway").
  A code-signing certificate (or Azure Trusted Signing) removes the warning; add a signing
  step after `ISCC.exe` when one exists.
- **macOS / iOS**: need an Apple Developer Program membership (Developer ID signing +
  notarization for macOS; TestFlight/App Store for iPhone/iPad).

## Download portal

- Source: `site/` (`index.html`, `styles.css`, `app.js`); built by `tools/site.py`
  (fills the repo/URL, generates `qr.svg`, converts the bundled fonts to WOFF2).
- Preview locally: `python3 tools/site.py serve` → http://localhost:8000
- Deployed by `.github/workflows/pages.yml` on pushes to `main` that touch the site and on
  every published release. One-time setup: `Settings → Pages → Source: GitHub Actions`.
- Accessibility: `tools/site_audit.py` (axe-core, WCAG 2.2 AA, light/dark × desktop/phone, plus
  text over the checkerboard against both tiles) runs before every deploy and blocks it on failure.
- Behaviour: detects the OS (user-agent client hints + UA), shows one big download button
  for it, lists all platforms with requirements or "Coming soon", shows a QR code that
  opens the page on a phone (hidden on phones/tablets), Italian or English from the
  browser language with a toggle, Purikura look or Night look in dark mode, animations off
  with "reduce motion". Availability comes from the latest release's assets via the GitHub
  API, with Windows + Android assumed when the API is unreachable.
