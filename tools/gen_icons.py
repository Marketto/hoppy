#!/usr/bin/env python3
"""Render desktop/web app icons from the generated launcher icon (needs Playwright + Pillow).

Input: assets/brand/hoppy-icon.svg (108x108 adaptive icon: gradient + sticker bunny),
written by tools/gen_mascots.py. Writes assets/brand/hoppy.ico (Windows installer/exe) and
assets/brand/hoppy-512.png (web portal, social preview) as a rounded square showing the
central 80 units, so the bunny is as big as on Android. Run after tools/gen_mascots.py
changes the icon.
"""
from pathlib import Path

from PIL import Image
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
BRAND = ROOT / "assets" / "brand"
CROP = "14 14 80 80"  # viewBox: everything the 72dp launcher mask shows, plus a margin


def main() -> None:
    svg = (BRAND / "hoppy-icon.svg").read_text().replace(
        'viewBox="0 0 108 108" width="108" height="108"', f'viewBox="{CROP}" width="512" height="512"')
    assert f'viewBox="{CROP}"' in svg, "unexpected hoppy-icon.svg header"
    html = ('<body style="margin:0;background:transparent"><div style="width:512px;height:512px;'
            f'border-radius:112px;overflow:hidden">{svg}</div></body>')
    png = BRAND / "hoppy-512.png"
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 512, "height": 512})
        page.set_content(html)
        page.screenshot(path=str(png), omit_background=True)
        browser.close()
    Image.open(png).save(BRAND / "hoppy.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    print("wrote assets/brand/hoppy-512.png, assets/brand/hoppy.ico")


if __name__ == "__main__":
    main()
