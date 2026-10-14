---
name: hoppy-release
description: Use for anything about shipping Hoppy - the release workflow, Windows installer, Android APK signing, macOS/Ubuntu/iOS packages and how to unlock them, the GitHub Pages download portal (OS detection, QR code, coming-soon cards).
---

# Releases and portal

Full reference: `docs/release.md`. Platform targets: `docs/platforms/macos-ios.md`.

| What | Where |
| --- | --- |
| Release pipeline | `.github/workflows/release.yml` (tags `v*` or manual; `publish` input for dry runs) |
| Windows installer | `apps/windows/hoppy.iss` (Inno Setup, per-user, runs `hoppy install`); icon `assets/brand/hoppy.ico` (from `tools/gen_icons.py`), embedded in the exe by `crates/hoppy-cli/build.rs` |
| Android version + signing | `apps/android/app/build.gradle.kts` (`hoppyVersionName`/`hoppyVersionCode` properties, `HOPPY_KEYSTORE_*` env from secrets) |
| Portal | `site/` + `tools/site.py`; deployed by `.github/workflows/pages.yml` |

## Rules

- Release asset names are stable and versionless; the portal (`site/app.js` `PLATFORMS`)
  and `releases/latest/download/<asset>` depend on them. Rename in both places or nowhere.
- Signing material comes only from GitHub secrets; never commit keystores, certificates or
  passwords (`python3 tools/dev.py secrets` enforces it).
- A platform is "live" when its job is enabled (`ENABLE_*` repo variable) and the latest
  release contains its asset. Don't fake availability in the portal.
- Portal copy exists in Italian and English (`T` in `site/app.js`); keep both in sync.
- Device icons are generic shapes, never vendor logos.

## Verify

- Portal: `python3 tools/site.py serve`, check desktop (QR visible) and phone width (QR hidden).
- Pipeline: Actions → Release → Run workflow with *publish* unticked; download the artifacts.
