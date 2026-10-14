# 0005. Five animated themes, chosen at first launch
Status: Accepted
Date: 2026-10-14

## Context
The product owner approved four animated Japanese-advertising design directions and
asked to ship all of them plus a dark theme in the same playful spirit, letting the user
choose "at installation". Android has no install-time UI.

## Decision
- Themes: Purikura (default), Showa, Manga, Yume, Night.
- Choice happens in a first-run picker with live previews; pre-selection follows the OS:
  high contrast → Manga, dark mode → Night, otherwise Purikura.
- Animations are native (AnimatedVectorDrawable for the mascot, Canvas-drawn views for
  backgrounds/stages, ObjectAnimators for body motion and button effects), all stopped
  off-screen and disabled when system animations are off.
- Japanese display fonts (SIL OFL) are bundled, subsetted to the app's characters by
  `tools/fonts.py` (~1.2 MB instead of ~40 MB).

## Consequences
Visual changes must be checked on five themes. New Japanese copy requires re-running the
font subsetter. Desktop clients will adopt the same theme names when they get a GUI.
