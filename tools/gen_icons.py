#!/usr/bin/env python3
"""Render app icons from the generated mascot SVGs (needs Playwright + Pillow).

Writes assets/brand/hoppy.ico (Windows installer/exe) and assets/brand/hoppy-512.png
(web portal, social preview). Run after tools/gen_mascots.py changes the Kawaii mascot.
"""
from pathlib import Path

from PIL import Image
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
BRAND = ROOT / "assets" / "brand"


def main() -> None:
    svg = (BRAND / "mascot-kawaii.svg").read_text().replace('width="200" height="200"', 'width="460" height="460"')
    html = ('<body style="margin:0;background:transparent"><div style="width:512px;height:512px;'
            'border-radius:112px;background:#FFE9F0;display:flex;align-items:center;justify-content:center">'
            f'{svg}</div></body>')
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
