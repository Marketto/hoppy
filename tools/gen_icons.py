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

`--macos` writes assets/brand/macos/ (Playwright + Pillow): hoppy-1024.png (the app icon on
Apple's 1024 grid: an 824 px rounded square with a soft shadow), Hoppy.icns (every size,
packaged by apps/macos/build-app.sh) and menubar-template.png (36 px = 18 pt @2x: the bunny
as a black-and-alpha template image for the menu bar, with eyes, mouth and inner ears cut
out; embedded by crates/hoppy-cli/src/macos/ui.rs). Also run by the default (full) mode.
"""
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
BRAND = ROOT / "assets" / "brand"
LINUX = BRAND / "linux"
MACOS = BRAND / "macos"
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


# Menu-bar template: body, ears and feet of mascot-happy.svg (viewBox 0 0 200 200) as the
# shape; eyes, mouth and inner ears cut out (thicker than the mascot's so they survive 18 pt).
TEMPLATE_SVG = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="24 30 152 152" width="{size}" height="{size}">
<defs><mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="200">
<g fill="#fff" stroke="#fff" stroke-width="6" stroke-linejoin="round">
<path d="M61,72 a15,36 0 1,0 30,0 a15,36 0 1,0 -30,0 Z" transform="rotate(-10 76 108)"/>
<path d="M109,80 a15,28 0 1,0 30,0 a15,28 0 1,0 -30,0 Z" transform="rotate(12 124 108)"/>
<path d="M127,60 C129,47 149,45 158,62 C161,70 154,74 149,69 C144,64 136,63 127,60 Z"/>
<path d="M30,136 C30,98 62,84 100,84 C138,84 170,98 170,136 C170,166 142,180 100,180 C58,180 30,166 30,136 Z"/>
<path d="M69,176 a11,6.5 0 1,0 22,0 a11,6.5 0 1,0 -22,0 Z"/>
<path d="M109,176 a11,6.5 0 1,0 22,0 a11,6.5 0 1,0 -22,0 Z"/>
</g>
<g fill="#000">
<path d="M70.5,80 a5.5,22 0 1,0 11,0 a5.5,22 0 1,0 -11,0 Z" transform="rotate(-10 76 108)"/>
<path d="M118.5,86 a5.5,15 0 1,0 11,0 a5.5,15 0 1,0 -11,0 Z" transform="rotate(12 124 108)"/>
<path d="M90,140 Q100,140 110,140 Q108,156 100,156 Q92,156 90,140 Z"/>
</g>
<g fill="none" stroke="#000" stroke-width="9" stroke-linecap="round">
<path d="M64,134 Q74,121 84,134"/><path d="M116,134 Q126,121 136,134"/>
</g>
</mask></defs>
<rect x="0" y="0" width="200" height="200" fill="#000" mask="url(#m)"/>
</svg>"""


def write_macos_icons(page) -> None:
    """Renders the macOS app icon and menu-bar template with a Playwright `page`."""
    MACOS.mkdir(exist_ok=True)
    svg = (BRAND / "hoppy-icon.svg").read_text().replace(
        SVG_HEADER, f'viewBox="{CROP}" width="824" height="824"')
    assert f'viewBox="{CROP}"' in svg, "unexpected hoppy-icon.svg header"
    radius = round(824 * RADIUS)
    page.set_viewport_size({"width": 1024, "height": 1024})
    page.set_content(
        '<body style="margin:0;background:transparent"><div style="position:absolute;left:100px;'
        f'top:88px;width:824px;height:824px;border-radius:{radius}px;overflow:hidden;'
        f'filter:drop-shadow(0 12px 14px rgba(0,0,0,.28))">{svg}</div></body>')
    master = MACOS / "hoppy-1024.png"
    page.screenshot(path=str(master), omit_background=True)
    big = Image.open(master).convert("RGBA")
    sizes = [16, 32, 64, 128, 256, 512]
    big.save(MACOS / "Hoppy.icns", append_images=[big.resize((s, s), Image.LANCZOS) for s in sizes])
    # Template: render large, keep only alpha (black), then downsample for clean edges.
    page.set_viewport_size({"width": 288, "height": 288})
    page.set_content('<body style="margin:0;background:transparent">'
                     + TEMPLATE_SVG.format(size=288) + "</body>")
    tmp = MACOS / "menubar-template.png"
    page.screenshot(path=str(tmp), omit_background=True)
    alpha = Image.open(tmp).convert("RGBA").getchannel("A").resize((36, 36), Image.LANCZOS)
    out = Image.new("RGBA", (36, 36), (0, 0, 0, 0))
    out.putalpha(alpha)
    out.save(tmp, optimize=True)
    print("wrote assets/brand/macos/hoppy-1024.png, Hoppy.icns, menubar-template.png")


IOS_ASSETS = ROOT / "apps" / "ios" / "Hoppy" / "Assets.xcassets"  # app only (icon)
IOS_SHARED = ROOT / "apps" / "ios" / "Shared" / "Mascots.xcassets"  # app + Share extension
# In-app mascots (points): idle on the home screen, happy after a delivery.
IOS_MASCOTS = {"Mascot": "mascot-kawaii.svg", "MascotHappy": "mascot-happy.svg"}
IOS_MASCOT_PT = 160


def write_ios_assets(page) -> None:
    """App icon (one opaque 1024 px square: iOS rounds the corners itself, and an icon with
    alpha would be shown on black) + the mascot images, into the Xcode asset catalog."""
    import json

    icon_set = IOS_ASSETS / "AppIcon.appiconset"
    icon_set.mkdir(parents=True, exist_ok=True)
    svg = (BRAND / "hoppy-icon.svg").read_text().replace(
        SVG_HEADER, f'viewBox="{CROP}" width="1024" height="1024"')
    assert f'viewBox="{CROP}"' in svg, "unexpected hoppy-icon.svg header"
    page.set_viewport_size({"width": 1024, "height": 1024})
    page.set_content(f'<body style="margin:0;background:#fff">{svg}</body>')
    icon = icon_set / "AppIcon-1024.png"
    page.screenshot(path=str(icon))
    Image.open(icon).convert("RGB").save(icon, optimize=True)
    (icon_set / "Contents.json").write_text(json.dumps({
        "images": [{"filename": icon.name, "idiom": "universal", "platform": "ios", "size": "1024x1024"}],
        "info": {"author": "tools/gen_icons.py", "version": 1},
    }, indent=2) + "\n")
    for name, src in IOS_MASCOTS.items():
        folder = IOS_SHARED / f"{name}.imageset"
        folder.mkdir(parents=True, exist_ok=True)
        art = (BRAND / src).read_text()
        images = []
        for scale in (2, 3):
            px = IOS_MASCOT_PT * scale
            page.set_viewport_size({"width": px, "height": px})
            page.set_content('<body style="margin:0;background:transparent">'
                             + art.replace('width="200" height="200"', f'width="{px}" height="{px}"', 1)
                             + "</body>")
            out = folder / f"{name}-{scale}x.png"  # no "@": the secrets scan reads it as an e-mail
            page.screenshot(path=str(out), omit_background=True)
            Image.open(out).save(out, optimize=True)
            images.append({"filename": out.name, "idiom": "universal", "scale": f"{scale}x"})
        (folder / "Contents.json").write_text(json.dumps({
            "images": [{"idiom": "universal", "scale": "1x"}, *images],
            "info": {"author": "tools/gen_icons.py", "version": 1},
        }, indent=2) + "\n")
    (IOS_SHARED / "Contents.json").write_text(json.dumps(
        {"info": {"author": "tools/gen_icons.py", "version": 1}}, indent=2) + "\n")
    print(f"wrote {icon_set.relative_to(ROOT)} and {', '.join(IOS_MASCOTS)} in {IOS_SHARED.relative_to(ROOT)}")


def main() -> None:
    args = sys.argv[1:]
    if "--ios" in args:
        from playwright.sync_api import sync_playwright

        with sync_playwright() as p:
            browser = p.chromium.launch()
            write_ios_assets(browser.new_page())
            browser.close()
        return
    if "--macos" in args:
        from playwright.sync_api import sync_playwright

        with sync_playwright() as p:
            browser = p.chromium.launch()
            write_macos_icons(browser.new_page())
            browser.close()
        return
    if "--linux" not in args:
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
            write_macos_icons(page)
            write_ios_assets(page)
            browser.close()
        Image.open(png).save(BRAND / "hoppy.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
        print("wrote assets/brand/hoppy-512.png, assets/brand/hoppy.ico")
    write_linux_icons()


if __name__ == "__main__":
    main()
