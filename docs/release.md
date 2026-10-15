# Releases and the download portal

## What gets published

| Platform | Status | Release asset (stable name) | Built by |
| --- | --- | --- | --- |
| Windows 10/11 x64 | **live** | `Hoppy-Setup-x64.exe` (Inno Setup, per-user, adds Send To), `Hoppy-windows-x64.zip` (portable) | `release.yml` → `windows` |
| Android 10+ (phone + tablet, one universal APK) | **live** | `Hoppy-android.apk` | `release.yml` → `android` |
| macOS 12+ — Intel Macs from 2015 and Apple silicon | **live** | `Hoppy-macos-universal.dmg` (`Hoppy.app` universal x86_64 + arm64 via `lipo`, ad-hoc signed; Developer ID + notarized once the `APPLE_*` secrets exist) | `release.yml` → `macos` (`apps/macos/build-app.sh`) |
| Ubuntu 22.04+ x64 | **live** | `Hoppy-linux-amd64.deb` (`cargo-deb`, built on ubuntu-22.04: glibc 2.35) | `release.yml` → `linux` |
| iPhone / iPad, iOS/iPadOS 15+ | **live** | `Hoppy-ios-unsigned.ipa` (`Payload/Hoppy.app` + `PlugIns/HoppyShare.appex`, **unsigned**: installed with Sideloadly and a free Apple ID, 7-day signature, Developer Mode; ADR 0009) | `release.yml` → `ios` (`apps/ios/build-ipa.sh`) |

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
`Hoppy-Setup-x64.exe` line, then runs it with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART
/UPDATE=1` (`hoppy.iss` relaunches the tray when it sees `/UPDATE=1`, also if setup fails). A
release without both assets is not offered. **Never rename `Hoppy-Setup-x64.exe` or
`SHA256SUMS.txt`**, never drop the installer from the sums, and keep tags `v` + plain `X.Y.Z`.

The **Linux tray** (docs/platforms/linux.md "Updates") reads the same `releases/latest`
and offers a version only when `Hoppy-linux-amd64.deb` and `SHA256SUMS.txt` are attached.
Installed from the .deb (`/usr/bin/hoppyw`), it downloads `Hoppy-linux-amd64.deb` from
`releases/download/<tag>/`, refuses it unless its SHA-256 matches its line in that release's
`SHA256SUMS.txt`, installs it with `pkexec apt-get install -y` (password through polkit) and
restarts; other copies link to the portal, whose Ubuntu button points at
`releases/latest/download/Hoppy-linux-amd64.deb` (`site/app.js`). **Never rename it.**
The **Mac menu-bar app** (docs/platforms/macos-ios.md "Updates") does the same with
`Hoppy-macos-universal.dmg` when it runs as `/Applications/Hoppy.app` (or
`~/Applications`) in a folder the user can write: download from `releases/download/<tag>/`,
SHA-256 against `SHA256SUMS.txt`, then the `Hoppy.app` **at the root of the .dmg** replaces
the installed one if `codesign --verify` passes and its `CFBundleIdentifier` is
`app.hoppy.desktop` and its `CFBundleShortVersionString` is the tag's `X.Y.Z` — so keep
`build-app.sh` writing the version and the bundle id, and the app at the image's root.
Elsewhere: notification + *Download Hoppy X…* → portal. **Never rename it.**
The **iPhone/iPad app** (`apps/ios/Hoppy/UpdateCheck.swift`) reads the same `releases/latest`
at most once a day and announces a version only when `Hoppy-ios-unsigned.ipa` is attached; it
never downloads (a sideloaded app is replaced by SideStore or by sideloading the new .ipa; the
portal's iOS button points at `releases/latest/download/Hoppy-ios-unsigned.ipa`). The portal
also publishes **`sidestore.json`**, an AltStore/SideStore source written by `tools/site.py`
from the latest release (`downloadURL` = `releases/download/<tag>/Hoppy-ios-unsigned.ipa`,
`version` = the tag without `v` or `-suffix`, equal to the app's `CFBundleShortVersionString`).
`release.yml` dispatches `pages.yml` after publishing so the source lists the new version
(releases created with `GITHUB_TOKEN` trigger no `release: published` run). **Never rename it.**

**Trust model of updates.** `SHA256SUMS.txt` comes from the same GitHub Release as the
installer, so it proves *integrity* (no corrupted or truncated download), not *authenticity*:
anyone who can publish a Release on this repository (a stolen token, a compromised account or
workflow) can ship a matching installer and sums. Authenticity rests on GitHub and on the
security of the repository and its CI. Consequences:

- **Windows:** the installer is **not signed** today. Planned: Authenticode-sign
  `Hoppy-Setup-x64.exe` (and both exes) in `release.yml` with a key from CI secrets, and have
  the tray verify the signature and publisher (`WinVerifyTrust`) before launching it.
- **Android:** updates install only if signed with the **same key** as the installed app.
  The keystore and its `ANDROID_*` CI secrets must never go missing or be rotated casually:
  losing them means no installed app can ever update again (users would have to uninstall).
  Keep an offline backup outside GitHub.

## Cutting a release

```
git tag v0.2.0 && git push origin v0.2.0        # or Actions → Release → Run workflow
```
`release.yml` stamps the version into Cargo (Windows `hoppy.exe` + `hoppyw.exe`, installer) and Android
(`versionName`, `versionCode` = run number), builds every enabled platform, then creates the
GitHub Release. Use **Run workflow** with *publish* unticked to build packages without
publishing (they appear as workflow artifacts).

## Unlocking a platform

Every platform is built on every release (the `ENABLE_LINUX`, `ENABLE_MACOS` and
`ENABLE_IOS` variables are no longer read). The portal shows a download as soon as the
latest release contains the platform's asset (`site/app.js` `PLATFORMS`); until then it says
"Coming soon". A new platform: add its job to `release.yml`, its asset to `PLATFORMS`, its
row above.

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
- **macOS**: today `Hoppy.app` is signed **ad hoc** (`codesign -s -`, no account needed; it
  runs on Apple silicon) and not notarized, so the first launch needs System Settings →
  Privacy & Security → **Open Anyway** (on macOS 12–14 also right-click → Open; macOS 15
  removed that bypass). The release job prints a warning saying so. Upgrade path, with an
  Apple Developer Program membership — **only secrets, no code change**:
  1. Create a *Developer ID Application* certificate, export it with its key as `.p12`:
     `base64 -i DeveloperID.p12 | pbcopy` → secret `APPLE_CERT_P12_BASE64`; its password →
     `APPLE_CERT_PASSWORD`. Optional `APPLE_SIGN_IDENTITY` (exact name, e.g.
     `Developer ID Application: Name (TEAMID)`) when the .p12 holds several identities.
     `release.yml` imports it into a temporary keychain (deleted at the end) and
     `apps/macos/build-app.sh` signs both executables, the app and the .dmg with the
     hardened runtime and a secure timestamp.
  2. For notarization add `APPLE_ID`, `APPLE_TEAM_ID` and `APPLE_APP_PASSWORD` (an
     app-specific password from account.apple.com): the job runs `xcrun notarytool submit
     --wait`, fails unless the status is *Accepted*, staples the ticket to the .dmg and
     checks it with `spctl`.
  3. Afterwards: drop the Gatekeeper steps from the .dmg's `Primo avvio - First launch.txt`,
     README and the portal hint (`dmgHint` in `site/app.js`); consider an in-app updater like
     Windows (verify the Developer ID team before replacing the app).
- **iOS**: the .ipa is **unsigned** on purpose (`CODE_SIGNING_ALLOWED=NO`, no entitlements):
  the user signs it with a free Apple ID through Sideloadly (docs/platforms/ios.md). The
  signature lasts 7 days; Developer Mode and "trust this Apple ID" are needed on the device.
  With an Apple Developer Program membership later: TestFlight/App Store, the multicast
  entitlement and an App Group become possible (ADR 0009 "Consequences").

## Download portal

- Source: `site/` (`index.html`, `styles.css`, `app.js`); built by `tools/site.py`
  (fills the repo/URL, generates `qr.svg`, converts the bundled fonts to WOFF2).
- Preview locally: `python3 tools/site.py serve` → http://localhost:8000
- Deployed by `.github/workflows/pages.yml` on pushes to `main` that touch the site, on
  releases published by hand and when `release.yml` dispatches it after publishing (needs
  `actions: write` in that job). One-time setup: `Settings → Pages → Source: GitHub Actions`.
- `sidestore.json` (iPhone/iPad SideStore/AltStore source) is generated at build time from
  `releases/latest`; CI builds with `--require-release` (an unreachable API fails the deploy
  rather than publishing a source without versions).
- Accessibility: `tools/site_audit.py` (axe-core, WCAG 2.2 AA, light/dark × desktop/phone, plus
  text over the checkerboard against both tiles) runs before every deploy and blocks it on failure.
- Behaviour: detects the OS (user-agent client hints + UA), shows one big download button
  for it, lists all platforms with requirements or "Coming soon", shows a QR code that
  opens the page on a phone (hidden on phones/tablets), Italian or English from the
  browser language with a toggle, Purikura look or Night look in dark mode, animations off
  with "reduce motion". Availability comes from the latest release's assets via the GitHub
  API, with Windows + Android assumed when the API is unreachable.
