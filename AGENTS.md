# AGENTS.md — Hoppy

Entry point for AI agents (Claude Code, Codex, Cursor, …). Read this file first,
then **only** the skill/doc the router points you to. Humans: see `README.md`.

Hoppy sends files between nearby devices (Windows, macOS, Linux, Android, iPhone)
as simply as possible: "Send to → Hoppy", tap a device, done. Current stage:
**PoC — Windows CLI + Android app, LAN only, protocol v0 (unencrypted)**.

## 1. Task router

Match the request to a row, load the skill, follow it. One request may hit several rows.

| If the task is about… | Load skill | Main files | Verify with |
| --- | --- | --- | --- |
| Wire format, discovery messages, limits, file-name rules | `.claude/skills/hoppy-protocol-change/SKILL.md` | `docs/protocol.md`, `docs/test-vectors.json` | `python3 tools/dev.py all` |
| Rust core (discovery, transfer, sanitizing) | `.claude/skills/hoppy-rust-core/SKILL.md` | `crates/hoppy-core/src/` | `python3 tools/dev.py check` |
| Desktop CLI, Windows "Send to", installer | `.claude/skills/hoppy-windows/SKILL.md` | `crates/hoppy-cli/src/`, `apps/windows/` | `python3 tools/dev.py check` |
| Android app (UI, service, share target, storage) | `.claude/skills/hoppy-android/SKILL.md` | `apps/android/` | `python3 tools/dev.py android-test` (+ CI build) |
| Themes, theme picker, animations, mascots, music, fonts, app icon | `.claude/skills/hoppy-themes/SKILL.md` | `apps/android/app/src/main/java/app/hoppy/ui/theme/`, `tools/gen_mascots.py`, `tools/gen_music.py`, `tools/fonts.py`, `docs/design/themes.md` | `python3 tools/dev.py mascots` / `music` (+ CI build) |
| Cross-implementation bugs ("works Rust→Rust, not with Android") | `.claude/skills/hoppy-interop-test/SKILL.md` | `tools/hoppy_ref.py`, `tools/interop.sh` | `python3 tools/dev.py interop` |
| New transport (BLE, Wi-Fi Direct/Aware, relay) or new platform | `.claude/skills/hoppy-new-transport/SKILL.md` | `docs/architecture.md`, `docs/roadmap.md` | depends; write an ADR |
| Releases, installers, download portal, enabling a platform | `.claude/skills/hoppy-release/SKILL.md` | `.github/workflows/release.yml`, `.github/workflows/pages.yml`, `site/`, `docs/release.md` | `python3 tools/site.py serve` + Release run with *publish* off |
| An architectural decision, or docs for agents | `.claude/skills/hoppy-docs-adr/SKILL.md` | `docs/decisions/`, this file | `python3 tools/dev.py docs` |
| Anything else / unsure | read `docs/architecture.md`, then pick a row | | `python3 tools/dev.py check` |

Specialised subagents live in `.claude/agents/` (`core-engineer`, `android-engineer`,
`windows-engineer`, `protocol-reviewer`). Use `protocol-reviewer` after any change that
touches bytes on the wire or files written to disk.

## 2. Repository map

```
crates/hoppy-core/     Rust library: protocol.rs, discovery.rs, transfer.rs, sanitize.rs
crates/hoppy-cli/      `hoppy` CLI (send / receive / peers / install) + `hoppyw` Windows tray/Send To GUI
apps/android/core/     Pure-JVM Kotlin: same protocol for Android (unit + interop tested)
apps/android/app/      Android UI shell: share target, receive service, themes
apps/windows/          Windows installer (Inno Setup) and install script
site/                  Download portal (GitHub Pages), built by tools/site.py
tools/dev.py           ONE command runner for everything (see `--help`)
tools/hoppy_ref.py     Python reference implementation + debugging CLI (stdlib only)
tools/interop.sh       Rust ↔ Python ↔ Kotlin wire tests
tools/gen_mascots.py   Mascot source of truth → SVG + Android vectors + animations
tools/gen_music.py     Theme music source of truth → Android OGG + desktop WAV
tools/fonts.py         Bundles subsetted OFL Japanese fonts into the Android app
docs/                  protocol, architecture, roadmap, decisions (ADRs), design, platforms
assets/brand/          Generated mascot SVGs + preview
assets/music/          Generated desktop music (WAV) + manifest
```

## 3. Invariants (never break these)

1. **Three implementations, one protocol.** Rust, Kotlin and Python must change together
   with `docs/protocol.md` and `docs/test-vectors.json`. A wire change without a vector and
   an interop run is not done.
2. **Never trust a received name.** All file names go through the §4 sanitizer; files are
   written to a temporary/pending location and published only after the SHA-256 matches.
3. **Never auto-accept by default.** Receiving asks the user unless they explicitly opt in.
4. **Android `:core` stays pure JVM** (no `android.*` imports) so it is testable off-device.
5. **No network work on the Android main thread**; `DiscoveryNode` already hops threads.
6. **Mascots, music and fonts are generated**: edit `tools/gen_mascots.py` /
   `tools/gen_music.py` / run `tools/fonts.py`, never edit the XML/SVG/OGG/WAV/TTF outputs.
7. **v0 is unencrypted** (ADR 0003). Do not ship outside trusted LANs; v1 adds encryption.
8. **Original art only.** Mascots must not resemble existing characters or brands; music
   must not quote or imitate existing tunes (game, anime, pop).
9. **Public repo: never commit sensitive data** — credentials, tokens, keystores, `.env`,
   personal e-mails, IPs or paths of real machines, names of anyone other than the author.
   The author is credited as **Marco Ricupero** (LICENSE, Cargo `authors`, installer
   publisher, .deb maintainer, portal footer); commit as
   `Marco Ricupero <Marketto@users.noreply.github.com>`, never with a personal e-mail. `python3 tools/dev.py secrets` (part of
   `check` and CI) must pass before every commit; signing keys come from CI secrets only.
10. **Accessible by default (WCAG 2.2 AA).** Text contrast ≥ 4.5:1 (large text 3:1) in every
    theme and in light/dark on the portal; pastels are decoration only. Enforced by
    `tools/contrast.py` (app, in `check`) and `tools/site_audit.py` (portal, gates deploy).

## 4. Commands

```
python3 tools/dev.py doctor        # what this machine can build
python3 tools/dev.py check         # fmt + clippy + Rust tests + Python selftest + docs lint + contrast + secrets scan
python3 tools/dev.py a11y          # axe-core audit of the portal (Playwright)
python3 tools/dev.py interop       # cross-language wire tests
python3 tools/dev.py all           # what CI runs on Linux
cargo run -p hoppy -- receive --yes --dir /tmp/in     # desktop receiver
cargo run -p hoppy -- send --to 127.0.0.1 FILE        # desktop sender
python3 tools/hoppy_ref.py peers                      # who is on the LAN?
```

The Android app needs the Android SDK; where it is missing, rely on CI
(`.github/workflows/ci.yml`) and on `python3 tools/dev.py interop` for the networking.

## 5. Conventions

- Code, comments, commit messages and agent docs: **English**. User-facing strings: English
  + Italian (`values-it`); the product owner is Italian.
- Rust: edition 2024, `cargo fmt`, clippy clean with `-D warnings`, blocking std I/O + threads
  (no async runtime in v0). Errors: `hoppy_core::Error`; CLI uses `anyhow`.
- Kotlin: official style, Views + Material 3 (no Compose in the PoC), minSdk 29.
- Keep the UI minimal: one primary action per screen, mascot on top, no settings screens.
- Commits: imperative mood, scope prefix (`core:`, `cli:`, `android:`, `protocol:`, `docs:`).
- Every behaviour change updates the doc it contradicts in the same commit.

## 6. Status and next steps

See `docs/roadmap.md`. Open questions are tracked as "Proposed" ADRs in `docs/decisions/`.
