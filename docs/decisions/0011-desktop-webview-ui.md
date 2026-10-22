# 0011. Desktop send window: one themed page in the system webview (wry + tao)
Status: Proposed (becomes Accepted after the macOS/Windows CI jobs are green and the manual
checks in docs/platforms/{windows,linux,macos-ios}.md "Send window" passed on real machines)
Date: 2026-10-22

## Context
On the desktops, "Send with Hoppy" showed the nearby devices in a native popup menu
(Windows), a plain `NSPanel` with buttons (macOS) or a zenity list (Linux); "no devices
found" was a greyed menu line, verification a separate dialog, progress only in a tray
tooltip. The product owner: *"a very ugly, very bare little screen… I'd prefer something
like the mobile one, nice, in line with the app's graphics, always with the selected
theme."* The Android send screen has the mascot, device cards, a progress bar, the inline
4-emoji panel and the five themes (ADR 0005).

Options:
1. Draw it natively three times (Win32 GDI, AppKit, GTK): three codebases for animated
   backgrounds, custom fonts and five palettes; Windows GDI cannot do any of it cheaply.
2. A Rust GUI toolkit (egui, iced, slint): one codebase, but a GPU/renderer stack in the
   binary, our own text shaping for Japanese decor and emoji, weaker screen-reader support.
3. The **system webview** through `wry` (Tauri's webview crate) in a `tao` window: WebView2
   on Windows (preinstalled on Windows 11 and on most Windows 10), WKWebView on macOS 12+,
   WebKitGTK 4.1 on Linux (Ubuntu 22.04+). HTML/CSS does gradients, animations,
   `prefers-reduced-motion`, web fonts, colour emoji and accessibility (UIA / AX / ATK) for
   free; one page for three systems.

## Decision
Option 3, only for `hoppyw send` (and the tray's *Send files…*, which spawns it). The tray
receiver, its menus and the accept dialogs stay native.

- **One page** (`crates/hoppy-cli/src/sendui/web/`: `index.html`, `app.css`, `app.js`, WOFF2
  font subsets) embedded with `include_str!`/`include_bytes!` and served by a custom protocol
  (`hoppy://localhost/`, `http://hoppy.localhost/` on Windows). The mascots are the
  generated `assets/brand/mascot-*.svg`, inlined so CSS animates ears, eyes and glint.
- **Rust owns the logic and every string**: `sendui::Flow` (pure, unit-tested on Linux) is
  the state machine (searching → devices | nobody found → sending with the inline
  verification panel → delivered | declined | failed | key changed); `Flow::view` builds a
  `View` with the texts already filled from the `gui.rs` IT/EN table; the page only draws it
  (`hoppy.render(view)` via `evaluate_script`) with `textContent`, never HTML from data.
- **Themes**: device.json `theme` (tray submenu *Theme*/*Tema* on all three desktops);
  empty = follow the system with Android's rule (high contrast → Manga, dark → Night, else
  Purikura; `crate::theme`, signals from `platform::system_look`). Palettes and fonts are
  docs/design/themes.md's; `tools/contrast.py` checks the CSS variables of every theme.
- **Fallback**: `sendui::window::run` returns `Unavailable` *before anything is shown* when
  there is no WebView2 runtime (`wry::webview_version`), no display, the toolkit or webview
  fails to start, or the page does not report ready within 10 s (the window stays hidden
  until then). `gui::send_mode` then runs the previous native flow (`send_native`)
  unchanged, so sending never breaks.

## Security
- Local content only: no remote URL is ever loaded; the custom protocol serves a fixed table
  of embedded files (anything else 404, non-GET 405) with a strict CSP (`default-src
  'none'`, `script-src 'self'`, `style-src 'self'`, `font-src 'self'`, no `connect-src`, no
  `unsafe-*`), `nosniff`, `no-store`; the same CSP is in a `<meta>` (tested equal).
- Navigation handler allows only our origin; `window.open` denied; devtools off (feature not
  compiled), zoom hotkeys, clipboard, autoplay and swipe navigation off; incognito
  (WebKit/WKWebView store nothing); WebView2's profile lives in `%LOCALAPPDATA%\Hoppy\WebView2`
  instead of next to the exe. File drops and the context menu are swallowed by the page.
- IPC: messages are taken only from our own page URL, at most 256 bytes, parsed by
  `sendui::parse_action` into a closed enum (exact field set, `pick` index bound-checked
  against the shown list, actions that do not fit the current screen ignored); invalid ones
  are logged by size only. The page never receives a path or an address: only cleaned
  display names (`display_*`), the first file's cleaned name, sizes and our strings.
- Unchanged protocol rules: "Forget device" is never the default, "They match" only marks
  the receiver verified (ADR 0010), the window can always be closed (= cancel).

## Consequences
- Linux: `hoppyw` links WebKitGTK 4.1 + GTK 3 (`libwebkit2gtk-4.1-0` in the .deb Depends;
  CI and release install `libwebkit2gtk-4.1-dev libgtk-3-dev`); the `hoppy` CLI does not
  (checked with `readelf`). `hoppyw` grows by about 0.3 MB of fonts; the release binary is
  ~5 MB on Linux.
- macOS: tao creates the `NSApplication` in the `hoppyw send` process (Accessory policy, no
  Dock icon); `ui::activate_app` brings it in front like the old panel. The AppKit smoke test
  and the window smoke test run as separate processes.
- Tests: `sendui` unit tests (transitions, IPC validation, JSON, assets, CSP);
  `hoppyw --smoke-test-window` opens the real window and renders every screen × theme (CI:
  Linux under Xvfb, macOS); `tools/sendui_audit.py` renders the same page in Chromium with
  axe-core (`dev.py a11y`). Windows is built and linted by CI; its window needs a manual run.
- Native popups/panel/zenity list remain as the fallback and must keep compiling.
