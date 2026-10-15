#!/usr/bin/env python3
"""Render desktop/web app icons from the generated launcher icon (needs Playwright + Pillow).

Input: assets/brand/hoppy-icon.svg (108x108 adaptive icon: gradient + sticker bunny),
written by tools/gen_mascots.py. Writes assets/brand/hoppy.ico (Windows installer/exe) and
assets/brand/hoppy-512.png (web portal, social preview) as a rounded square showing the
central 80 units, so the bunny is as big as on Android. Run after tools/gen_mascots.py
changes the icon.

Also writes the Linux hicolor set in assets/brand/linux/ (packaged by cargo-deb, the tray
icon is embedded from hoppy-48.png / hoppy-128.png): PNGs resized from hoppy-512.png and a
scalable hoppy.svg with the same crop and rounded corners.
`python3 tools/gen_icons.py --linux` redoes only that part (Pillow only, no browser).
"""
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
BRAND = ROOT / "assets" / "brand"
LINUX = BRAND / "linux"
CROP = "14 14 80 80"  # viewBox: everything the 72dp launcher mask shows, plus a margin
RADIUS = 112 / 512  # corner radius as a fraction of the side (same as the PNG)
LINUX_SIZES = (48, 128, 256, 512)
SVG_HEADER = 'viewBox="0 0 108 108" width="108" height="108"'


def write_linux_icons() -> None:
    LINUX.mkdir(exist_ok=True)
    src = Image.open(BRAND / "hoppy-512.png").convert("RGBA")
    for size in LINUX_SIZES:
        img = src if size == 512 else src.resize((size, size), Image.LANCZOS)
        img.save(LINUX / f"hoppy-{size}.png", optimize=True)
    svg = (BRAND / "hoppy-icon.svg").read_text()
    assert SVG_HEADER in svg, "unexpected hoppy-icon.svg header"
    x, y, w, h = (float(v) for v in CROP.split())
    head, body = svg.split(">", 1)  # the <svg …> opening tag, then the drawing
    head = head.replace(SVG_HEADER, f'viewBox="{CROP}" width="256" height="256"')
    body = body.rsplit("</svg>", 1)[0]
    clip = (f'<defs><clipPath id="hoppy-round"><rect x="{x:g}" y="{y:g}" width="{w:g}" '
            f'height="{h:g}" rx="{w * RADIUS:g}"/></clipPath></defs>')
    (LINUX / "hoppy.svg").write_text(f'{head}>{clip}<g clip-path="url(#hoppy-round)">{body}</g></svg>\n')
    print("wrote assets/brand/linux/hoppy-{" + ",".join(map(str, LINUX_SIZES)) + "}.png, hoppy.svg")


def main() -> None:
    if "--linux" not in sys.argv[1:]:
        from playwright.sync_api import sync_playwright

        svg = (BRAND / "hoppy-icon.svg").read_text().replace(
            SVG_HEADER, f'viewBox="{CROP}" width="512" height="512"')
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
    write_linux_icons()


if __name__ == "__main__":
    main()
