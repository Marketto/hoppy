---
name: hoppy-themes
description: Use when touching Hoppy's look and sound - the five themes (Purikura, Showa, Manga, Yume, Night), the first-run theme picker, mascots and their animations, theme music, palettes, bundled fonts, app/notification icons, or adding a new theme.
---

# Themes and mascots

Spec and palettes: `docs/design/themes.md`. Decision: `docs/decisions/0005-five-themes-first-run-picker.md`.

## Where things are

| What | Where |
| --- | --- |
| Theme enum, styles, migration | `apps/android/app/src/main/java/app/hoppy/ui/theme/HoppyTheme.kt`, `res/values/themes.xml`, `res/values/attrs.xml` |
| OS-based suggestion (+ unit test) | `ui/theme/ThemeSuggestion.kt`, `app/src/test/java/app/hoppy/ui/theme/ThemeSuggestionTest.kt` |
| First-run picker | `ui/ThemePickerActivity.kt`, `res/layout/activity_theme_picker.xml`, `item_theme_card.xml` |
| Animated views | `ui/theme/ThemeBackgroundView.kt`, `MascotStageView.kt`, `MascotView.kt`, `HoppyButton.kt`, `ShineDrawable.kt`, `ButtonFx.kt`, `HomeDecor.kt`, `Motion.kt` |
| Mascots + animations (generated) | `tools/gen_mascots.py` → `res/drawable/mascot_*.xml`, `res/animator/mascot_*.xml`, `assets/brand/` |
| App icon (generated) | `tools/gen_mascots.py` (`ICON_*`) → `res/drawable/ic_launcher_{background,foreground,monochrome}.xml`, `assets/brand/hoppy-icon.svg`; then `tools/gen_icons.py` → `assets/brand/hoppy.ico`, `hoppy-512.png`. Spec: `docs/design/themes.md` "App icon" |
| Japanese decor copy | `res/values/decor.xml` |
| Desktop send window (5 themes as CSS, motion, WOFF2 fonts) | `crates/hoppy-cli/src/sendui/web/` (`app.css` palettes checked by `tools/contrast.py`; fonts by `tools/fonts.py --desktop`), choice in `crates/hoppy-cli/src/theme.rs`; spec `docs/design/themes.md` "Desktop send window" |
| Music (generated) | `tools/gen_music.py` → `res/raw/music_<theme>_<idle\|transfer\|done>.ogg`, `assets/music/desktop_*.wav`, `assets/music/manifest.json` |
| Fonts (generated, OFL) | `tools/fonts.py` → `res/font/*.ttf`, licences in `apps/android/app/src/main/assets/licenses/fonts/` |

## Rules

- Never edit generated files: run `python3 tools/dev.py mascots`, `python3 tools/dev.py music`
  or `python3 tools/fonts.py`. `python3 tools/gen_music.py --check` tells if audio is stale.
- Music: edit the note data / arrangement in `tools/gen_music.py` (notation in its docstring);
  loops must stay an exact number of bars (the script verifies length, seam, clipping, DC);
  keep the Android set under ~1.5 MB and each desktop WAV under ~250 KB. Original tunes only.
  Playback rules (sender only, app visible, mute) are in `docs/design/themes.md` "Music".
- After adding Japanese text anywhere in `res/values*/`, run `python3 tools/fonts.py`.
- The `hoppyThemeKind` attr enum order must match `HoppyTheme` order.
- Motion stops in `onStop`, on detach, when hidden, and when system animations are off
  (`Motion`); new animated views implement `HostAware`. Draw with Canvas, no per-frame bitmaps.
- Original characters only; only paths, fills, strokes, alpha and group rotate/scale/translate
  in mascots (SVG = VectorDrawable).
- Contrast ≥ 4.5:1; Manga is the high-contrast theme and must stay that way.

## Adding a theme (checklist)

1. Mood, palette, fonts, motion, music in `docs/design/themes.md`; a `Theme` in `tools/gen_music.py`.
2. Value in `HoppyTheme` (style, mascot, label, isDark) and in the `hoppyThemeKind` enum (same order).
3. `Theme.Hoppy.<Name>` in `themes.xml` (Material color roles, fonts, `shapeAppearanceCorner*`).
4. Drawing branches in `ThemeBackgroundView`, `MascotStageView`, `HoppyButton`, `HomeDecor`; body motion in `MascotView`.
5. Label + description strings in `values/strings.xml` and `values-it/strings.xml`; new fonts in `tools/fonts.py`.
   Desktop: a `Theme` variant in `crates/hoppy-cli/src/theme.rs`, a `[data-theme]` block + motion
   in `sendui/web/app.css` (every variable `tools/contrast.py` checks), `python3 tools/fonts.py --desktop`.
6. If it should be auto-suggested, extend `ThemeSuggestion` and its test.
