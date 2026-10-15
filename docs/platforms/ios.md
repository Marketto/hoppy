# iPhone and iPad

Hoppy for iPhone/iPad (ADR 0009): a SwiftUI app plus a Share extension over the Rust core
(`crates/hoppy-ffi` → `HoppyFFI.xcframework`). Target: **iOS/iPadOS 15 and later**, iPhone and
iPad (tested device: iPad Air 2022, iPadOS 18). Distributed as an **unsigned
`Hoppy-ios-unsigned.ipa`**, installed with **Sideloadly** and a free Apple ID: no Apple
Developer account, no App Store, no TestFlight.

## Install with Sideloadly (from a Windows PC)

1. On the PC: install **iTunes** (the apple.com version, not the Microsoft Store one, as
   Sideloadly asks) and **Sideloadly** (sideloadly.io). Download `Hoppy-ios-unsigned.ipa` from
   the Release page (or the `hoppy-ios` CI artifact).
2. Connect the iPad with a cable, unlock it and tap **Trust** («Autorizza») on "Trust this
   computer?".
3. In Sideloadly: drop the .ipa on the window, pick the iPad, type your **Apple ID** e-mail,
   press **Start**, enter the Apple ID password (and the 2-factor code) when asked. A free
   Apple ID is enough. Optional: under *Advanced options* tick *Automatic re-sign / Wi-Fi* if
   you want Sideloadly to refresh the app over Wi-Fi.
4. On the iPad: **Settings → General → VPN & Device Management** («VPN e gestione
   dispositivi») → your Apple ID → **Trust**.
5. **Settings → Privacy & Security → Developer Mode** («Modalità sviluppatore») → on → restart
   → confirm **Turn On** after the restart. (The switch appears only after a sideloaded app was
   installed once.)
6. Open Hoppy: allow **Local Network** («Rete locale») when asked — without it no device is
   found (fix later in Settings → Privacy & Security → Local Network → Hoppy).

**7 days.** A free signature expires after 7 days: Hoppy then does not open ("no longer
available"). Repeat step 3 (same Apple ID, files in Documents are kept). A free Apple ID can
have at most 3 sideloaded apps (each app extension counts toward the App ID limit of 10 per
week). From a Mac, Sideloadly works the same way.

## How it works

| Piece | Where | Notes |
| --- | --- | --- |
| Rust core over C | `crates/hoppy-ffi/src/lib.rs`, `crates/hoppy-ffi/include/hoppy.h` | One opaque node: discovery (unicast sweep), optional receiver, sends, accept question. JSON config/events; event list in the crate docs. Tested on Linux (`crates/hoppy-ffi/src/tests.rs`). |
| Swift wrapper | `apps/ios/Shared/HoppyCore.swift` | `HoppyNode` (start/stop, peers, send, decide, setReceiving), `HoppyEvent` decoded from JSON; no protocol logic. |
| App | `apps/ios/Hoppy/` | `HoppyApp.swift` (scene phase → start/stop), `AppModel.swift` (state), `ContentView.swift` (the one screen, pickers, Quick Look, share). |
| Share extension | `apps/ios/ShareExtension/` | `ShareViewController.swift`: stages the shared items, own send-only node, device list, progress, closes itself on delivery. |
| Shared UI bits | `apps/ios/Shared/` | `Theme.swift` (Purikura/Night, checked by `tools/contrast.py`), `Identity.swift` (device id/name, seeds, outbox), strings `en.lproj`/`it.lproj`, `Mascots.xcassets` (generated). |
| Project | `apps/ios/project.yml` | XcodeGen: targets `Hoppy`, `HoppyShare`, `HoppyTests` (host-less simulator tests). |
| Build scripts | `apps/ios/build-xcframework.sh`, `apps/ios/build-ipa.sh` | xcframework (checks iOS 15 `minos`, exported symbols); unsigned device build → .ipa (checks the extension is inside). |
| Icons (generated) | `python3 tools/gen_icons.py --ios` | `apps/ios/Hoppy/Assets.xcassets/AppIcon.appiconset` (opaque 1024 px) and `apps/ios/Shared/Mascots.xcassets`. Never hand-edit. |
| Jingle | `assets/music/desktop_done.wav` (generated) | Referenced by `project.yml`, played after a delivery (ambient: the silent switch mutes it; menu toggle «♪ Suono a fine invio»). |

- **Discovery (spec §2.1).** iOS apps without the multicast entitlement cannot send or hear
  multicast/broadcast. The node sends unicast `query` datagrams to every host of the Wi-Fi's
  /24 (paced, ~0.5 s, every 10 s and on "search again"), to recently seen peer IPs and to IPs
  typed in *Aggiungi per indirizzo IP*. Every Hoppy receiver answers a query with a unicast
  `announce` and lists the sender (spec §2 rule 3): the iPad sees them, they see the iPad.
  While receiving, it queries its known peers every 5 s and says `bye` to them when it stops.
  Desktop "Send with Hoppy" processes also sweep (they listen on an ephemeral port), so the
  iPad appears in their device menu (opt out: `HOPPY_NO_SWEEP=1` / `hoppy --no-sweep`);
  Android hears the iPad's sweeps on its shared node. Only the interface carrying the
  default route is swept when there is one (never cellular, VPN or virtual adapters).
- **Receiving.** Only while Hoppy is open (iOS has no background listening for this kind of
  app). The node starts when the app becomes active and stops (`bye`) when it goes to the
  background; a running transfer gets iOS's ~30 s to finish, and the screen does not lock
  during a transfer. An offer shows the alert **File in arrivo / Incoming files** with the
  sender, count, size and up to 5 names: **Rifiuta/Decline** is the bold cancel action,
  **Accetta/Accept** the other; no answer in 110 s = declined; a second offer meanwhile is
  declined. Files are written as `.hoppy-part` and published after the SHA-256 matches
  (core; leftover `.hoppy-part` files of a transfer iOS cut off are deleted at the next
  start), into the app's **Documents**: Files → On My iPad → Hoppy. The card "Ricevuti N file
  da …" offers **Apri** (Quick Look), **Condividi**, **Mostra in File**.
- **Sending.** *Invia file* → *Scegli file* (Files picker, copies) or *Foto e video* (Photos
  picker, originals: HEIC/HEVC), then the **Invia a** sheet; or tap a device first, then pick.
  *Open in Hoppy* from other apps opens the same sheet. Files are cloned into a temporary
  outbox (APFS clone: no extra space), streamed by Rust in 256 KiB chunks, removed after.
  Status: «Aspetto che X accetti…», «Invio a X · n%», «Consegnato a X ✓» (+ jingle and haptic),
  «X ha rifiutato.», the callback fallback text (spec §6) and «Non riesco a raggiungere…».
- **Share extension.** Photos/Files/any app → Share → **Hoppy**: the extension prepares the
  files, lists nearby devices (its own sweep, ephemeral port), sends, shows the progress and
  closes ~1 s after «Consegnato». It cannot read the app's settings (no App Group with a
  free signature): it appears to others as the device model ("iPad"), with its own id.
- **Device name.** iOS 16+ only tells apps the model ("iPad"); rename in *… → Nome
  dispositivo* (or tap «Visibile come …»).
- **Local Network permission.** Asked the first time Hoppy sends to the LAN. Denied = empty
  device list; the app shows the Settings path under the list.

## Limits (product facts)

- Receives only while open; senders get «Non riesco a raggiungere» otherwise.
- Free signature: 7 days, Developer Mode, trust step; no TestFlight/App Store.
- Networks larger than /24 (offices): only the iPad's /24 is swept; add other devices by IP.
  Client isolation (guest Wi-Fi) blocks everything, as on other platforms.
- The Share extension's name is the device model; folders are not sent (v0 has no folders).
- v0 is unencrypted (ADR 0003): trusted networks only.

## Build and CI

There is no local iOS build in this repository's dev loop (no Xcode on Linux; the owner's
2015 Mac cannot run a current Xcode). CI job **`iOS · Rust core + unsigned Hoppy.app +
simulator tests`** (`.github/workflows/ci.yml`, macos-latest):

1. `cargo clippy -p hoppy-ffi --target aarch64-apple-ios` (annotations: `tools/ci-annotate.sh`);
2. `bash apps/ios/build-xcframework.sh release` (3 targets, `lipo` for the simulator, iOS 15
   `minos` check, `_hoppy_node_start` exported);
3. `brew install xcodegen && xcodegen generate`;
4. `xcodebuild test` on the newest available iPhone simulator (`apps/ios/Tests/HoppyCoreTests.swift`:
   version, start/stop, bad config, loopback send accept + decline + bye) — builds the app and
   extension for the simulator too;
5. `bash apps/ios/build-ipa.sh` → `Hoppy-ios-unsigned.ipa` artifact `hoppy-ios`.

xcodebuild failures become annotations through `tools/ci_annotate_xcode.py` (compiler
`file:line: error:` lines, failing XCTest cases, linker blocks, log tail). On a Mac with
Xcode 15+: `rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios`,
`brew install xcodegen`, `bash apps/ios/build-xcframework.sh debug`, `cd apps/ios && xcodegen
generate && open Hoppy.xcodeproj`.

## Manual test plan (iPad Air 2022, iPadOS 18)

Same Wi-Fi: the iPad, a Windows PC with Hoppy, an Android phone (and a Mac/Linux if at hand).

1. **Install**: Sideloadly install succeeds; trust + Developer Mode steps as above; the icon is
   the bunny on the pink–lavender gradient; Hoppy opens; the Local Network prompt shows the
   Italian text on an Italian iPad; after *Consenti*, devices appear within ~10 s.
2. **Look**: mascot on top, «Pronto a ricevere» on, «Visibile come «iPad»», «Vicino a te»,
   *Invia file*; dark mode → Night palette; portrait and landscape, Split View and Slide Over
   keep the layout readable; Dynamic Type at the largest size wraps text; VoiceOver reads the
   mascot, switch, rows ("PC-Ufficio, Windows, button") and buttons.
3. **Rename** to "iPad di Marco": the PC/phone list shows the new name within ~10 s.
4. **Discovery**: the PC tray menu and the phone app list "iPad di Marco"; the iPad lists the
   PC and the phone. Turn «Pronto a ricevere» off → the iPad disappears from the PC/phone
   within ~1 s (bye); on → back within ~5 s. Send to the iPad from the PC's *Invia con Hoppy*
   (send-only process: tests the desktop sweep) and from Android's share sheet.
5. **Receive**: PC → iPad 2 photos: alert «File in arrivo» with names and size; Rifiuta → PC
   says declined, nothing saved; again, Accetta → progress «Ricevo · n%», card «Ricevuti 2
   file da …»; Apri (Quick Look), Condividi, Mostra in File (Files → Sul mio iPad → Hoppy).
   No answer for 110 s → alert closes, PC says declined. A 2 GB video: completes, no memory
   warning, screen stays on.
6. **Send from the app**: *Invia file → Foto e video* (HEIC + a 4K video) → *Invia a* → PC:
   PC asks, accepts, files arrive with their original names; jingle (silent switch off) and
   haptic; «Consegnato a … ✓». *Scegli file* from iCloud Drive / On My iPad; tap a device
   first then pick: goes straight to it. PC declines → «… ha rifiutato.» PC tray quit →
   «Non riesco a raggiungere…».
7. **Share extension**: Photos → select 3 → Share → Hoppy (scroll the app row / *More* the
   first time): list appears, tap PC, progress, «Consegnato», sheet closes. From Files with a
   1 GB file: no crash (memory limit). Cancel closes without sending.
8. **Open in Hoppy**: Files → long-press a PDF → Share → *Hoppy* in the app row or *Open in*
   → the *Invia a* sheet appears with that file.
9. **Background**: during a receive, press Home: the transfer completes within ~30 s or is
   reported failed on the PC; reopen Hoppy: the node restarts, devices reappear. Leave the
   app in background for a minute: the PC no longer lists the iPad (bye).
10. **Firewalled PC** (Windows firewall blocking Hoppy): iPad → PC uses the callback («… non
    risponde subito, provo un'altra strada…») and delivers.
11. **Expiry** (day 8): Hoppy no longer opens; reinstall with Sideloadly; received files in
    Documents are still there.
