# 0008. macOS desktop: the same `hoppyw` on AppKit via objc2, ad-hoc signed
Status: Proposed (PoC; becomes Accepted after the macOS CI job is green and the manual test
plan in `docs/platforms/macos-ios.md` passed on an Intel 2015 Mac and an Apple silicon Mac)
Date: 2026-10-15

## Context
After Windows, Android and Ubuntu (ADR 0007), the Mac should get the same experience: a
receiver in the menu bar at login, "Send with Hoppy" from the Finder, a device menu, an
accept question, notifications, the jingle, an update notice. Constraints:
- Macs from 2015 (Intel) stop at macOS 12 Monterey; Apple silicon must run natively → one
  universal binary with deployment target 12.0.
- No Apple Developer account: no Developer ID, no notarization, no App Store. Gatekeeper
  blocks the first launch once (macOS 15 removed the right-click → Open bypass).
- CI can compile for macOS (macos-latest); the development machine (Linux) cannot.

Options for the UI: a Swift/SwiftUI app (needs Xcode in CI, a second code path for the
logic), `tray-icon`/`muda` crates (a menu-bar icon only; dialogs, services, login items still
needed), or AppKit directly from Rust through the objc2 crates behind the existing
`platform::ui` API.

## Decision
- `hoppyw` gains a macOS backend, `crate::macos::ui`, with the same API as `platform::win`
  and `crate::linux::ui`; `gui.rs` stays platform-neutral (only a new "Open at login" switch
  through `platform::login_item`, `None` elsewhere). `NATIVE_POPUPS = true`: the menu-bar
  icon pops a menu built at each click, like Windows.
- objc2 0.6 / objc2-app-kit, -foundation, -user-notifications, -service-management 0.3,
  block2, dispatch2, with only the header features in use. The main thread runs a manual
  `NSApplication` event loop inside `Tray::next_event`; worker threads post to a channel and
  wake it with an application-defined `NSEvent`; dialogs requested by workers run on the main
  thread via `dispatch2::run_on_main`.
- UX mapping: `NSStatusItem` + template image; device menu `NSMenu` at the pointer; accept
  question `NSAlert` with Decline as the default button (never auto-accept, 110 s timeout =
  decline); file picker `NSOpenPanel`; jingle `NSSound`; folders/URLs `/usr/bin/open`;
  updates: notify + portal link (no self-install of an unsigned app), via `/usr/bin/curl`
  (code shared with Linux in `crate::unix`).
- Notifications: `UNUserNotificationCenter` when running from the bundle (click → open the
  folder). Fallback to `osascript display notification` only when the API is unavailable
  (bare binary, or authorization error for the ad-hoc signature) — never when the user
  turned notifications off.
- Finder integration without Automator: an app-provided **Service** declared in Info.plist
  (`NSServices`, `sendFiles:userData:error:`, `public.data`, enabled by default via
  `NSRequiredContext`), plus `CFBundleDocumentTypes` (rank Alternate) for drops on the icon
  and "Open With". Only the long-running tray registers as services provider; it hands the
  files to a `hoppyw send` child, like the Windows tray does.
- Login item: `SMAppService.mainApp` on macOS 13+, falling back to a marked LaunchAgent plist
  (`open -a Hoppy.app`) on macOS 12 or when the system refuses the ad-hoc app; turned on at
  the very first run (like the Windows startup shortcut / Linux autostart), skipped while
  App-Translocated or on the .dmg.
- Packaging: `Hoppy.app` (`LSUIElement`, `LSMinimumSystemVersion` 12.0, id
  `app.hoppy.desktop`, `.icns` generated with Pillow), `hoppyw` as the main executable and the
  `hoppy` CLI next to it, signed ad hoc; `Hoppy-macos-universal.dmg` with an Applications
  link and first-launch instructions. `release.yml` builds it on every release (no longer
  gated by `ENABLE_MACOS`); Developer ID signing + notarization run automatically once the
  `APPLE_*` secrets exist.
- Verification without a Mac: pure helpers are unit-tested on Linux; the CI macos job runs
  clippy (arm64 and x86_64), the tests, and `hoppyw --smoke-test` (debug build, so objc2
  checks every message's types at runtime) bare and inside a bundle, then builds the dmg.

## Consequences
One code path for three desktops. No Xcode project, no Swift toolchain. Costs: Gatekeeper's
"Open Anyway" on first launch and after updates, privacy prompts (Downloads, Local Network,
firewall) may repeat after updates because grants are tied to the ad-hoc signature; the Finder
entry lives under Services, not top level; no Share-menu entry (both need an `.appex`, i.e.
Xcode + signing: roadmap). The `osascript` fallback shows notifications under Script Editor's
name. An Apple Developer account removes the first two costs without code changes.

## Amendment (2026-10-15): self-update and device verification
The product owner asked that the desktop apps fetch, verify and install new versions
themselves. The "no self-install" choice above assumed Gatekeeper would block a replaced
app; an update downloaded by Hoppy's own curl carries no quarantine flag, so it is not
blocked. Hoppy now replaces `/Applications/Hoppy.app` (or `~/Applications`) when that folder
is writable by the user: the release's .dmg (SHA-256 against `SHA256SUMS.txt`) is mounted
read-only, the app copied next to the installed one, checked (`codesign --verify`, bundle id,
version), swapped in with two renames and relaunched after the tray exits; elsewhere the
portal link stays. Privacy prompts may still repeat after an update (ad-hoc signature). The
protocol v1 verification UI (ADR 0010) is an `NSAlert` accessory view (emoji + names, a
"123" switch, the QR code as an image). Details: `docs/platforms/macos-ios.md`.
