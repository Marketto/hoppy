---
name: hoppy-ios
description: Use for the iPhone/iPad app - SwiftUI app and Share extension in apps/ios, the Rust FFI crate (crates/hoppy-ffi, HoppyFFI.xcframework), the unicast discovery sweep, XcodeGen project, unsigned .ipa for Sideloadly, iOS CI job and its annotations.
---

# iPhone / iPad

UX, install (Sideloadly), limits, manual test plan: `docs/platforms/ios.md`. Decision:
`docs/decisions/0009-ios-unsigned-sideload.md`. Wire behaviour of the sweep: `docs/protocol.md` §2.1.

## Where things are

| What | File |
| --- | --- |
| C ABI over hoppy-core (node, events, JSON), Linux-tested | `crates/hoppy-ffi/src/lib.rs`, `crates/hoppy-ffi/src/tests.rs` |
| C header (hand-written; a test keeps it equal to the exports) | `crates/hoppy-ffi/include/hoppy.h` |
| Unicast sweep (client behaviour, no wire change) | `crates/hoppy-core/src/discovery.rs` (`Sweep`, `sweep_targets`, `set_transfer_port`) |
| Swift wrapper (`HoppyNode`, `HoppyEvent`, `HoppySecurity`/`HoppySas`/`HoppyPeerKey`) | `apps/ios/Shared/HoppyCore.swift` |
| App: scene lifecycle, state, the one screen, offer sheet | `apps/ios/Hoppy/HoppyApp.swift`, `apps/ios/Hoppy/AppModel.swift`, `apps/ios/Hoppy/ContentView.swift` |
| QR scanner (AVFoundation) and "Mostra il mio QR" (CoreImage), app only | `apps/ios/Hoppy/QRViews.swift` |
| Update notice (`releases/latest`, daily) | `apps/ios/Hoppy/UpdateCheck.swift`; SideStore source `sidestore.json` from `tools/site.py` |
| Share extension (self-contained sender, own identity) | `apps/ios/ShareExtension/ShareViewController.swift` |
| Verification UI (code, «Coincide», banners) shared by app + extension | `apps/ios/Shared/Verification.swift` |
| Palettes (checked by `tools/contrast.py`), identity (id bound to the v1 key, `Identity.startNode`)/seeds/outbox, strings | `apps/ios/Shared/Theme.swift`, `apps/ios/Shared/Identity.swift`, `apps/ios/Shared/en.lproj/`, `apps/ios/Shared/it.lproj/` |
| Plists | `apps/ios/Hoppy/Info.plist`, `apps/ios/ShareExtension/Info.plist` |
| XcodeGen project (no committed .xcodeproj) | `apps/ios/project.yml` |
| Builds | `apps/ios/build-xcframework.sh`, `apps/ios/build-ipa.sh` |
| Simulator tests (Swift FFI smoke) | `apps/ios/Tests/HoppyCoreTests.swift` |
| Generated icon + mascots | `python3 tools/gen_icons.py --ios` → `apps/ios/Hoppy/Assets.xcassets/AppIcon.appiconset`, `apps/ios/Shared/Mascots.xcassets` |
| Static checks on Linux | `tools/ios_check.py` (in `dev.py check`) |
| CI / release | `.github/workflows/ci.yml` job `ios`, `.github/workflows/release.yml` job `ios`; annotator `tools/ci_annotate_xcode.py` |

## Rules

- No protocol logic in Swift: everything on the wire goes through `hoppy-core` (invariant 1).
  New capability = a Rust function in `hoppy-ffi` + header line + Rust test + Swift wrapper.
- Free signature only (ADR 0009): no entitlements file, no App Groups, iCloud, push, multicast
  entitlement, background modes. `tools/ios_check.py` fails on such entitlements.
- The Share extension binds an **ephemeral** UDP port (`udp_port: 0`) and never receives; it
  must not use `UIApplication.shared` (extension-safe API only) and must stream (no `Data`
  of whole files). It has **no camera** (extensions cannot use it): no AVFoundation capture in
  `ShareExtension/` or `Shared/` (checked).
- Protocol v1 identity (ADR 0010, ADR 0009 addendum): every node starts through
  `Identity.startNode` (persistent `key_file`/`trust_file` in Application Support, excluded
  from backups; a new key means a new device id, never KeyChanged on peers) — never
  `HoppyNode(config:)` in the app or extension (checked). The extension is a **separate
  device** «<model> (Condividi)» with its own key: there is no free way to share the app's key.
- Verification follows the other platforms: code (4 emoji / «123» → digits), «Coincide»,
  «Verifica con QR» (app only), «Mostra il mio QR» (app only, receiving on while shown),
  «Più tardi»; KeyChanged → alert with «Dimentica dispositivo»; Decline stays the default.
- Update notice only (never a download); the asset name `Hoppy-ios-unsigned.ipa` is shared by
  `UpdateCheck.swift`, `tools/site.py` (SideStore source) and `release.yml` (checked).
- Never auto-accept: the alert's cancel action is Decline; the Rust side declines on
  timeout (110 s) and on stop.
- iOS 15 is the floor (`project.yml`, `build-xcframework.sh`; checked): no iOS 16+ SwiftUI API
  (`NavigationStack`, `.autocorrectionDisabled`, alert text fields…) without `#available`.
- Every user string in `Shared/{en,it}.lproj/Localizable.strings(dict)` (same keys and
  format specifiers; checked). Keep `site/howto.js` (iOS flow) identical to the real UI text.
- The release xcframework is built with the workspace's `[profile.ffi]` (release without
  `strip`: a stripped static library loses its C exports; `build-xcframework.sh` checks
  `_hoppy_node_start` with `nm -g` in every slice). Every `extern "C"` body runs in `guard`
  (no panic crosses the boundary); callbacks never call back into the node synchronously.
- Generated files (`AppIcon-1024.png`, `Mascots.xcassets`, `desktop_done.wav`): never hand-edit.
- Asset name is a contract (`docs/release.md`): `Hoppy-ios-unsigned.ipa`. Bundle ids
  `app.hoppy.ios`, `app.hoppy.ios.share`.

## Verify

`python3 tools/dev.py check` (Rust FFI + sweep tests, `ios_check.py`, contrast) and
`python3 tools/dev.py interop`. Linux cannot build Swift: push and read the annotations of the
CI job `iOS · Rust core + unsigned Hoppy.app + simulator tests` (logs are not readable; cargo
failures via `tools/ci-annotate.sh`, xcodebuild via `tools/ci_annotate_xcode.py`). A person
runs the manual test plan in `docs/platforms/ios.md` on the iPad.
