---
name: hoppy-themes
description: Use when touching Hoppy's look - the Kawaii and Pro themes, mascots, palette, corner radii, app/notification icons, or adding a new theme.
---

# Themes and mascots

Spec: `docs/design/themes.md` (palette, mood, do/don't). Two themes today:

- **Kawaii** — Japanese/Korean cute style: mochi-shaped cream bunny, short ears (one
  folded), tiny dot eyes set wide and low, ㅅ mouth, manga blush lines, thin cocoa outline,
  pastel strawberry-milk palette, 24dp corners.
- **Pro** — stylized flat grey bunny with black sunglasses and a teal accent, graphite
  dark palette, 10dp corners.

## Mascots are code

`tools/gen_mascots.py` defines each mascot as shapes on a 200×200 canvas and writes:
`assets/brand/mascot-<theme>.svg` and `apps/android/app/src/main/res/drawable/mascot_<theme>.xml`
(+ `ic_notification.xml`). **Never edit the outputs.** After editing run
`python3 tools/dev.py mascots`, then render a preview (any browser) and check both themes at
300px and 64px.

## Adding a theme (checklist)

1. Palette + mood in `docs/design/themes.md`.
2. Mascot list in `tools/gen_mascots.py` (`MASCOTS["name"]`).
3. Colors in `res/values/colors.xml`, style `Theme.Hoppy.<Name>` in `res/values/themes.xml`
   setting `hoppyMascot`, Material color roles and `shapeAppearanceCorner*`.
4. Enum value in `Prefs.Theme`, mapping in `Prefs.themeRes`, label string (en + it),
   status-bar style in `ThemedActivity`.

## Rules

- Original characters only; no resemblance to existing mascots/brands.
- Only paths, fills, strokes, opacity and group rotation (so SVG = VectorDrawable).
- Contrast: text colors on backgrounds ≥ 4.5:1.
