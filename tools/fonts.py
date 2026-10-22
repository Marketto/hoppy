#!/usr/bin/env python3
"""Bundle the theme fonts into the Android app, subset to what the app shows.

Source: the @expo-google-fonts npm packages (the Google Fonts TTFs, SIL OFL 1.1).
Output: apps/android/app/src/main/res/font/<name>.ttf + licences in
        apps/android/app/src/main/assets/licenses/fonts/.

Glyphs kept: Latin (incl. Italian accents), punctuation, arrows/stars, full kana,
plus every character found in the app's string resources. **Re-run after adding
Japanese text to any res/values*/*.xml**, otherwise Android falls back to the
system font for the missing characters (no tofu, just a style mismatch).

Needs: npm, fonttools (`pip install fonttools`).   Usage: python3 tools/fonts.py

Desktop send window (`python3 tools/fonts.py --desktop`, needs fonttools + brotli, no npm):
the bundled TTFs above are subset again to Latin + kana + the Japanese decor used by
crates/hoppy-cli/src/sendui/ and written as WOFF2 to crates/hoppy-cli/src/sendui/web/fonts/
(embedded in hoppyw). Re-run after changing the decor text there. The fonts keep their
name tables (copyright + OFL notice); the licences are copied next to them.
"""
from __future__ import annotations

import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RES = ROOT / "apps/android/app/src/main/res"
OUT = RES / "font"
LICENSES = ROOT / "apps/android/app/src/main/assets/licenses/fonts"

# resource name -> (npm package, file inside the package)
FONTS = {
    "mochiy_pop_one": ("mochiy-pop-one", "400Regular/MochiyPopOne_400Regular.ttf"),
    "yusei_magic": ("yusei-magic", "400Regular/YuseiMagic_400Regular.ttf"),
    "zen_maru_gothic_medium": ("zen-maru-gothic", "500Medium/ZenMaruGothic_500Medium.ttf"),
    "zen_maru_gothic_black": ("zen-maru-gothic", "900Black/ZenMaruGothic_900Black.ttf"),
    "dela_gothic_one": ("dela-gothic-one", "400Regular/DelaGothicOne_400Regular.ttf"),
    "zen_old_mincho_bold": ("zen-old-mincho", "700Bold/ZenOldMincho_700Bold.ttf"),
    "zen_kaku_gothic_new_medium": ("zen-kaku-gothic-new", "500Medium/ZenKakuGothicNew_500Medium.ttf"),
    "zen_kaku_gothic_new_black": ("zen-kaku-gothic-new", "900Black/ZenKakuGothicNew_900Black.ttf"),
    "hachi_maru_pop": ("hachi-maru-pop", "400Regular/HachiMaruPop_400Regular.ttf"),
    "kiwi_maru_medium": ("kiwi-maru", "500Medium/KiwiMaru_500Medium.ttf"),
}

BASE_RANGES = [
    (0x0020, 0x007E),  # Basic Latin
    (0x00A0, 0x017F),  # Latin-1 + Latin Extended-A (àèéìòù…)
    (0x2010, 0x2027),  # dashes, quotes, ellipsis, bullet
    (0x2190, 0x21FF),  # arrows (→ ⇄)
    (0x2605, 0x2606),  # ★ ☆
    (0x266A, 0x266B),  # ♪ ♫
    (0x3000, 0x303F),  # CJK punctuation
    (0x3040, 0x30FF),  # hiragana + katakana
    (0xFF01, 0xFF5E),  # fullwidth forms (！？)
]


def app_text() -> str:
    """All text in the app's string/decor resources."""
    chunks = []
    for xml in RES.glob("values*/*.xml"):
        chunks.append(re.sub(r"<[^>]+>", " ", xml.read_text(encoding="utf-8")))
    return "".join(chunks)


DESKTOP_OUT = ROOT / "crates/hoppy-cli/src/sendui/web/fonts"
DESKTOP_SOURCES = [ROOT / "crates/hoppy-cli/src/sendui/mod.rs", ROOT / "crates/hoppy-cli/src/sendui/web/app.js"]
# Fonts the send window uses (docs/design/themes.md); Yusei Magic is Android-only.
DESKTOP_FONTS = [n for n in FONTS if n != "yusei_magic"]
DESKTOP_RANGES = [
    (0x0020, 0x007E), (0x00A0, 0x017F), (0x2010, 0x2027), (0x2190, 0x2193),
    (0x3000, 0x303F), (0x3040, 0x30FF), (0xFF01, 0xFF5E),
]


def desktop() -> int:
    """WOFF2 subsets of the bundled TTFs for the desktop send window."""
    try:
        from fontTools import subset  # noqa: F401
        import brotli  # noqa: F401
    except ImportError:
        sys.exit("✗ pip install fonttools brotli")
    chars = {chr(c) for lo, hi in DESKTOP_RANGES for c in range(lo, hi + 1)}
    for src in DESKTOP_SOURCES:
        # Japanese decor and other non-Latin text in the sources (CJK, kana, punctuation).
        chars |= {c for c in src.read_text(encoding="utf-8") if 0x3000 <= ord(c) <= 0x9FFF or 0xFF00 <= ord(c) <= 0xFFEF}
    text = "".join(sorted(chars))
    DESKTOP_OUT.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        text_file = Path(tmp) / "chars.txt"
        text_file.write_text(text, encoding="utf-8")
        total = 0
        for name in DESKTOP_FONTS:
            out = DESKTOP_OUT / f"{name}.woff2"
            subprocess.run([sys.executable, "-m", "fontTools.subset", str(OUT / f"{name}.ttf"),
                            f"--text-file={text_file}", f"--output-file={out}", "--flavor=woff2",
                            "--layout-features=kern,liga,palt", "--no-hinting", "--name-IDs=0,1,2,3,4,5,6,13,14",
                            "--name-languages=*"], check=True)
            total += out.stat().st_size
            print(f"✓ {out.relative_to(ROOT)}  {out.stat().st_size // 1024} KB")
    for pkg in sorted({FONTS[n][0] for n in DESKTOP_FONTS}):
        shutil.copy(LICENSES / f"{pkg}-OFL.txt", DESKTOP_OUT / f"{pkg}-OFL.txt")
    print(f"✓ {len(text)} characters, {total // 1024} KB in {DESKTOP_OUT.relative_to(ROOT)}")
    return 0


def main() -> int:
    if "--desktop" in sys.argv[1:]:
        return desktop()
    if not shutil.which("npm"):
        sys.exit("✗ npm is required")
    try:
        from fontTools import subset  # noqa: F401
    except ImportError:
        sys.exit("✗ pip install fonttools")

    chars = {chr(c) for lo, hi in BASE_RANGES for c in range(lo, hi + 1)}
    chars |= {c for c in app_text() if ord(c) >= 0x20}
    text = "".join(sorted(chars))

    OUT.mkdir(parents=True, exist_ok=True)
    LICENSES.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        packages = {pkg for pkg, _ in FONTS.values()}
        for pkg in sorted(packages):
            tgz = subprocess.run(["npm", "pack", "-q", f"@expo-google-fonts/{pkg}"], cwd=tmp,
                                 check=True, capture_output=True, text=True).stdout.strip().splitlines()[-1]
            dest = tmp_path / pkg
            with tarfile.open(tmp_path / tgz) as tar:
                tar.extractall(dest, filter="data")
            shutil.copy(dest / "package/LICENSE_FONT", LICENSES / f"{pkg}-OFL.txt")
        text_file = tmp_path / "chars.txt"
        text_file.write_text(text, encoding="utf-8")
        for name, (pkg, rel) in FONTS.items():
            src = tmp_path / pkg / "package" / rel
            out = OUT / f"{name}.ttf"
            subprocess.run([sys.executable, "-m", "fontTools.subset", str(src), f"--text-file={text_file}",
                            f"--output-file={out}", "--layout-features=*", "--no-hinting",
                            "--name-IDs=*", "--name-languages=*"], check=True)
            print(f"✓ {out.relative_to(ROOT)}  {out.stat().st_size // 1024} KB")
    print(f"✓ {len(text)} characters kept; licences in {LICENSES.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
