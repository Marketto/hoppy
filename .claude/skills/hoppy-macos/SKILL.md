---
name: hoppy-macos
description: Use for the Mac desktop experience - hoppyw on macOS (menu-bar icon, NSAlert accept question, device menu at the pointer, notifications, Finder service, drops on the app icon, Open at login), the Hoppy.app bundle and .dmg, ad-hoc signing and the Developer ID/notarization upgrade path, Gatekeeper first-launch steps.
---

# macOS desktop

UX, files, first launch, test plan: `docs/platforms/macos-ios.md`. Decision:
`docs/decisions/0008-macos-desktop.md`. Shared logic: `crates/hoppy-cli/src/gui.rs`.

## Where things are

| What | File |
| --- | --- |
| `platform::ui` on macOS (AppKit via objc2; same API as `platform::win`) | `crates/hoppy-cli/src/macos/ui.rs` |
| Open at login (SMAppService 13+, LaunchAgent fallback), `hoppy install/uninstall`, first run | `crates/hoppy-cli/src/macos/login.rs` |
| Pure helpers + Info.plist consistency test (run on Linux) | `crates/hoppy-cli/src/macos/mod.rs` |
| curl update transport (shared with Linux) | `crates/hoppy-cli/src/unix/http.rs` |
| Bundle metadata, Finder service, privacy texts | `apps/macos/Info.plist`, `apps/macos/it.lproj/`, `apps/macos/en.lproj/` |
| Build universal app + dmg / bundle given binaries | `apps/macos/build-app.sh` (`release` / `bundle`) |
| Icons (generated: `python3 tools/gen_icons.py --macos`) | `assets/brand/macos/` |
| CI (clippy arm64 + x86_64, tests, `hoppyw --smoke-test`, dmg) | `.github/workflows/ci.yml` job `macos` |
| Release (ad-hoc; Developer ID + notarization when `APPLE_*` secrets exist) | `.github/workflows/release.yml` job `macos` |

## Rules

- AppKit only in `macos/ui.rs` and `macos/login.rs` (`#[cfg(target_os = "macos")]`); logic
  that can be pure goes in `macos/mod.rs` with a test. `gui.rs` gets no `cfg`; a new `ui`
  function needs twins in `platform::win` and `crate::linux::ui`.
- AppKit objects only on the main thread (`MainThreadMarker`). Workers: `post`/`Waker` to
  reach the event loop, `dispatch2::run_on_main` for a dialog. Every call that runs its own
  event loop (menus, `runModal`) is followed by `after_modal()` (re-arms the wake-up event).
- Never auto-accept: Decline is the alert's first (default) button; timeout = decline.
- Values for `osascript` are script arguments after the `"hoppy"` sentinel, never script
  text (`notification_args`); no other shell/AppleScript strings are built from data.
- Only the receiver registers as services provider (a sender process would take the port).
- macOS 12 is the floor: anything newer (SMAppService 13+, `activate` 14+) behind a runtime
  check or the deprecated 12-compatible call with `#[allow(deprecated)]`; `build-app.sh`
  fails if any architecture's `minos` > 12.0.
- objc2 features are listed one by one in `crates/hoppy-cli/Cargo.toml`; add the header
  feature when a type/method is "not found" in CI.
- Asset name is a contract (`docs/release.md`): `Hoppy-macos-universal.dmg`. Bundle id
  `app.hoppy.desktop` (Info.plist ↔ `macos::BUNDLE_ID`, tested).
- Generated icons: never hand-edit `assets/brand/macos/*`.

## Verify

`python3 tools/dev.py check` on Linux (pure macOS tests included). Linux cannot compile the
AppKit code: push and read the `macOS · tests + universal Hoppy.app` job's annotations (CI
logs are not readable; failures are re-emitted by `tools/ci-annotate.sh`). The smoke test
runs in a debug build so objc2 verifies message signatures. A person on a real Mac runs the
manual test plan in `docs/platforms/macos-ios.md` (Intel 2015 on macOS 12 + Apple silicon).
