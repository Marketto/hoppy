#!/usr/bin/env python3
"""WCAG 2.2 AA contrast check for the Android themes' text/background pairs.

Reads the real colors from apps/android/app/src/main/res/values/colors.xml and checks
every pair below (text color on the backgrounds it is drawn over). Part of
`python3 tools/dev.py check`. Thresholds: 4.5:1 for text, 3:1 for large text (≥ 24sp, or
≥ 18.66sp bold) and for non-text UI parts that convey state.
When you add a theme or draw text on a new background, add the pair here.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
COLORS = ROOT / "apps/android/app/src/main/res/values/colors.xml"

TEXT, LARGE = 4.5, 3.0

# (theme, what, text color, [backgrounds], threshold)
PAIRS = [
    ("purikura", "body text", "purikura_text", ["purikura_bg", "purikura_check_a", "purikura_check_b", "purikura_container", "white"], TEXT),
    ("purikura", "muted text", "purikura_muted", ["purikura_bg", "purikura_check_a", "purikura_container", "white"], TEXT),
    ("purikura", "button label", "white", ["purikura_btn_top", "purikura_btn_bottom"], TEXT),
    ("purikura", "pyon sticker", "purikura_primary_dark", ["purikura_yellow"], TEXT),
    ("purikura", "accent text", "purikura_primary_dark", ["purikura_bg", "purikura_check_a", "white"], TEXT),
    ("showa", "body text", "showa_ink", ["showa_bg", "showa_card", "showa_container"], TEXT),
    ("showa", "muted text", "showa_muted", ["showa_bg", "showa_card", "showa_container"], TEXT),
    ("showa", "button label / stamp", "showa_bg", ["showa_vermilion"], TEXT),
    ("showa", "logo (large)", "showa_teal", ["showa_bg"], LARGE),
    ("manga", "body text", "manga_ink", ["manga_bg", "manga_container", "manga_pink"], TEXT),
    ("manga", "muted text", "manga_muted", ["manga_bg", "manga_container"], TEXT),
    ("manga", "button label", "white", ["manga_ink"], TEXT),
    ("yume", "body text", "yume_text", ["yume_bg_1", "yume_bg_2", "yume_bg_3", "yume_bg_4", "yume_container"], TEXT),
    ("yume", "muted text", "yume_muted", ["yume_bg_1", "yume_bg_2", "yume_bg_3", "yume_bg_4", "yume_container"], TEXT),
    ("yume", "button label", "white", ["yume_btn_1", "yume_btn_2", "yume_btn_3"], TEXT),
    ("night", "body text", "night_text", ["night_bg", "night_surface", "night_container"], TEXT),
    ("night", "muted text", "night_muted", ["night_bg", "night_surface", "night_container"], TEXT),
    ("night", "button label", "night_on_teal", ["night_teal"], TEXT),
    ("night", "accent text", "night_teal", ["night_bg", "night_surface", "night_container"], TEXT),
    ("night", "chip text", "night_on_teal_container", ["night_teal_container"], TEXT),
]


def load_colors() -> dict[str, str]:
    text = COLORS.read_text(encoding="utf-8")
    return {m.group(1): m.group(2) for m in re.finditer(r'<color name="([a-z0-9_]+)">#([0-9A-Fa-f]{6,8})</color>', text)}


def to_rgb(hex_color: str, over: tuple[float, float, float] = (1.0, 1.0, 1.0)) -> tuple[float, float, float]:
    """#RRGGBB or #AARRGGBB (Android order); translucent colors are composited over `over`."""
    if len(hex_color) == 8:
        a = int(hex_color[:2], 16) / 255
        rgb = [int(hex_color[i:i + 2], 16) / 255 for i in (2, 4, 6)]
        return tuple(a * c + (1 - a) * o for c, o in zip(rgb, over))
    return tuple(int(hex_color[i:i + 2], 16) / 255 for i in (0, 2, 4))


def luminance(rgb: tuple[float, float, float]) -> float:
    f = [c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4 for c in rgb]
    return 0.2126 * f[0] + 0.7152 * f[1] + 0.0722 * f[2]


def ratio(a, b) -> float:
    la, lb = sorted([luminance(a), luminance(b)], reverse=True)
    return (la + 0.05) / (lb + 0.05)


def main() -> int:
    colors = load_colors()
    # The Yume sky is a gradient drawn in code (ThemeBackgroundView); check every stop.
    colors.update({"yume_bg_1": "DCCBFF", "yume_bg_2": "F6D8F2", "yume_bg_3": "FFE6EF", "yume_bg_4": "D6F5EC"})
    bad = 0
    for theme, what, fg, bgs, need in PAIRS:
        for bg in bgs:
            if fg not in colors or bg not in colors:
                print(f"✗ {theme}: unknown color {fg if fg not in colors else bg}")
                bad += 1
                continue
            r = ratio(to_rgb(colors[fg]), to_rgb(colors[bg]))
            if r < need:
                print(f"✗ {theme:<9} {what:<22} {fg} on {bg}: {r:.2f}:1 < {need}:1")
                bad += 1
    if bad:
        print(f"✗ contrast: {bad} pair(s) below WCAG AA")
        return 1
    print(f"✓ contrast: {sum(len(p[3]) for p in PAIRS)} theme color pairs meet WCAG AA")
    return 0


if __name__ == "__main__":
    sys.exit(main())
