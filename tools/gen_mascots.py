#!/usr/bin/env python3
"""Single source of truth for the Hoppy mascots and their animations.

Each mascot is a list of items on a 200x200 canvas: shapes, or named groups of
shapes (`group(...)`) that animations can move. This script writes:

  assets/brand/mascot-<name>.svg                       SVG (docs, desktop, web; groups get CSS classes)
  apps/android/app/src/main/res/drawable/mascot_<name>.xml       VectorDrawable (named groups)
  apps/android/app/src/main/res/drawable/mascot_<name>_anim.xml  AnimatedVectorDrawable
  apps/android/app/src/main/res/animator/mascot_*.xml            shared animators
  apps/android/app/src/main/res/drawable/ic_notification.xml     status-bar icon

Run after editing: python3 tools/gen_mascots.py   (or: python3 tools/dev.py mascots)
Only paths, fills, strokes, alpha and group rotate/scale/translate are used, so SVG and
VectorDrawable render identically. Palette: docs/design/themes.md.
"""
from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RES = ROOT / "apps/android/app/src/main/res"


def ellipse(cx, cy, rx, ry=None):
    ry = rx if ry is None else ry
    return (f"M{cx - rx:g},{cy:g} a{rx:g},{ry:g} 0 1,0 {2 * rx:g},0 "
            f"a{rx:g},{ry:g} 0 1,0 {-2 * rx:g},0 Z")


def rrect(x, y, w, h, r):
    return (f"M{x + r:g},{y:g} h{w - 2 * r:g} a{r:g},{r:g} 0 0,1 {r:g},{r:g} v{h - 2 * r:g} "
            f"a{r:g},{r:g} 0 0,1 {-r:g},{r:g} h{-(w - 2 * r):g} a{r:g},{r:g} 0 0,1 {-r:g},{-r:g} "
            f"v{-(h - 2 * r):g} a{r:g},{r:g} 0 0,1 {r:g},{-r:g} Z")


def shape(d, fill=None, stroke=None, width=0, opacity=1.0, rotate=None, name=None):
    return {"d": d, "fill": fill, "stroke": stroke, "width": width, "opacity": opacity,
            "rotate": rotate, "name": name}


def group(name, pivot, items):
    """A named, animatable group. `pivot` = (x, y) for rotation/scale."""
    return {"group": name, "pivot": pivot, "items": items}


# ---------------------------------------------------------------- Kawaii (mochi bunny)
# Japanese/Korean "mochi" style: squishy wide body, short ears (one folded),
# tiny dot eyes set wide and low, ㅅ mouth, manga blush lines, thin warm outline.
INK = "#6B4E45"
CREAM = "#FFFDF8"
EAR_PINK = "#FFD3DC"
EYE = "#3A2A26"

EAR_L = group("ear_l", (76, 112), [
    shape(ellipse(76, 72, 15, 36), CREAM, INK, 3, rotate=(-10, 76, 108)),
    shape(ellipse(76, 76, 6.5, 24), EAR_PINK, rotate=(-10, 76, 108)),
])
EAR_R = group("ear_r", (124, 112), [
    shape(ellipse(124, 80, 15, 28), CREAM, INK, 3, rotate=(12, 124, 108)),
    shape(ellipse(124, 84, 6.5, 17), EAR_PINK, rotate=(12, 124, 108)),
    shape("M127,60 C129,47 149,45 158,62 C161,70 154,74 149,69 C144,64 136,63 127,60 Z", CREAM, INK, 3),
    shape("M133,58 C138,52 148,53 153,62", None, EAR_PINK, 3),
])
BODY = [
    shape("M30,136 C30,98 62,84 100,84 C138,84 170,98 170,136 "
          "C170,166 142,180 100,180 C58,180 30,166 30,136 Z", CREAM, INK, 3),
    shape(ellipse(80, 176, 11, 6.5), CREAM, INK, 3),
    shape(ellipse(120, 176, 11, 6.5), CREAM, INK, 3),
]
CHEEKS = [
    shape(ellipse(60, 144, 11, 6), "#FFB3C1", opacity=0.85, name="cheek_l"),
    shape(ellipse(140, 144, 11, 6), "#FFB3C1", opacity=0.85, name="cheek_r"),
    shape("M54,147 l4,-6 M60,147 l4,-6 M66,147 l4,-6", None, "#F28CA0", 1.8),
    shape("M134,147 l4,-6 M140,147 l4,-6 M146,147 l4,-6", None, "#F28CA0", 1.8),
]
EYES_DOT = group("eyes", (100, 131), [
    shape(ellipse(74, 131, 5.2), EYE),
    shape(ellipse(126, 131, 5.2), EYE),
    shape(ellipse(75.8, 129.2, 1.5), "#FFFFFF"),
    shape(ellipse(127.8, 129.2, 1.5), "#FFFFFF"),
])
EYES_HAPPY = group("eyes", (100, 131), [
    shape("M66,134 Q74,124 82,134", None, EYE, 3.4),
    shape("M118,134 Q126,124 134,134", None, EYE, 3.4),
])
MOUTH_SMALL = shape("M93,145 Q98,143 100,137 Q102,143 107,145", None, INK, 2.6)  # ㅅ
MOUTH_OPEN = shape("M92,141 Q100,141 108,141 Q106,153 100,153 Q94,153 92,141 Z", "#E8758D", INK, 2.4)

KAWAII = [EAR_L, EAR_R, *BODY, *CHEEKS, EYES_DOT, MOUTH_SMALL]
HAPPY = [EAR_L, EAR_R, *BODY, *CHEEKS, EYES_HAPPY, MOUTH_OPEN]

# ---------------------------------------------------------------- Night (cool bunny)
# Stylized flat grey bunny, rounder and friendlier than v1: soft ears, faint blush,
# black wayfarers with a teal top line and a glint that sweeps across.
GREY = "#E9E9EC"
GREY_IN = "#C4C4CC"
SHADES = "#0E0F12"
PRO = [
    group("ear_l", (78, 108), [
        shape(rrect(63, 16, 30, 94, 15), GREY, rotate=(-9, 78, 104)),
        shape(rrect(71, 28, 14, 68, 7), GREY_IN, rotate=(-9, 78, 104)),
    ]),
    group("ear_r", (122, 108), [
        shape(rrect(107, 16, 30, 94, 15), GREY, rotate=(9, 122, 104)),
        shape(rrect(115, 28, 14, 68, 7), GREY_IN, rotate=(9, 122, 104)),
    ]),
    shape(ellipse(100, 132, 62, 52), GREY),
    shape(ellipse(62, 152, 10, 5.5), "#FF9EBB", opacity=0.45, name="cheek_l"),
    shape(ellipse(138, 152, 10, 5.5), "#FF9EBB", opacity=0.45, name="cheek_r"),
    group("glasses", (100, 124), [
        shape(rrect(44, 108, 112, 8, 4), SHADES),
        shape("M50,112 H97 L93,136 Q91,144 83,144 H64 Q55,144 53,136 Z", SHADES),
        shape("M103,112 H150 L147,136 Q145,144 136,144 H117 Q109,144 107,136 Z", SHADES),
        shape(rrect(44, 108, 112, 2.5, 1.25), "#4FD1C5"),
        group("glint", (70, 126), [
            shape("M60,118 L70,118 L62,138 L55,138 Z", "#FFFFFF", opacity=0.28, name="glint_l"),
            shape("M112,118 L122,118 L114,138 L107,138 Z", "#FFFFFF", opacity=0.28, name="glint_r"),
        ]),
    ]),
    shape(ellipse(100, 152, 5, 3.5), "#B5B5BD"),
    shape("M93,163 q9,5 17,-3", None, "#5A5A62", 3),
]

# Monochrome status-bar icon (24x24, white): bunny head silhouette.
NOTIFICATION = [
    shape(rrect(6.5, 1.5, 4, 11, 2), "#FFFFFF", rotate=(-8, 8.5, 12)),
    shape(rrect(13.5, 1.5, 4, 11, 2), "#FFFFFF", rotate=(8, 15.5, 12)),
    shape(ellipse(12, 15, 8, 6.5), "#FFFFFF"),
]

MASCOTS = {"kawaii": KAWAII, "happy": HAPPY, "pro": PRO}

# ---------------------------------------------------------------- animations
# name -> (duration ms, property, [(fraction, value), ...])
ANIMATORS = {
    "mascot_ear_l": (2800, "rotation", [(0, 0), (.2, -9), (.38, 4), (.55, -3), (.7, 0), (1, 0)]),
    "mascot_ear_r": (2800, "rotation", [(0, 0), (.24, 10), (.42, -4), (.6, 5), (.75, 0), (1, 0)]),
    "mascot_blink": (4200, "scaleY", [(0, 1), (.9, 1), (.93, .08), (.96, 1), (1, 1)]),
    "mascot_blush": (2400, "fillAlpha", [(0, .85), (.5, .45), (1, .85)]),
    "mascot_glint": (3200, "translateX", [(0, -14), (.55, -14), (.85, 22), (1, 22)]),
}
# which animator each named target gets, per mascot
TARGETS = {
    "kawaii": {"ear_l": "mascot_ear_l", "ear_r": "mascot_ear_r", "eyes": "mascot_blink",
               "cheek_l": "mascot_blush", "cheek_r": "mascot_blush"},
    "happy": {"ear_l": "mascot_ear_l", "ear_r": "mascot_ear_r",
              "cheek_l": "mascot_blush", "cheek_r": "mascot_blush"},
    "pro": {"ear_l": "mascot_ear_l", "ear_r": "mascot_ear_r", "glint": "mascot_glint"},
}


# ---------------------------------------------------------------- writers
def svg_shape(s):
    attrs = [f'd="{s["d"]}"', f'fill="{s["fill"] or "none"}"']
    if s["stroke"]:
        attrs += [f'stroke="{s["stroke"]}"', f'stroke-width="{s["width"]:g}"',
                  'stroke-linecap="round"', 'stroke-linejoin="round"']
    if s["opacity"] != 1.0:
        attrs.append(f'opacity="{s["opacity"]:g}"')
    if s["rotate"]:
        a, x, y = s["rotate"]
        attrs.append(f'transform="rotate({a:g} {x:g} {y:g})"')
    if s["name"]:
        attrs.append(f'class="{s["name"]}"')
    return f'<path {" ".join(attrs)}/>'


def svg_items(items, indent="  "):
    out = []
    for it in items:
        if "group" in it:
            out.append(f'{indent}<g class="{it["group"]}">')
            out += svg_items(it["items"], indent + "  ")
            out.append(f"{indent}</g>")
        else:
            out.append(indent + svg_shape(it))
    return out


def to_svg(items, size=200):
    return "\n".join([f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" '
                      f'width="{size}" height="{size}">', *svg_items(items), "</svg>\n"])


def vd_shape(s, indent):
    attrs = [f'android:pathData="{s["d"]}"']
    if s["name"]:
        attrs.insert(0, f'android:name="{s["name"]}"')
    if s["fill"]:
        attrs.append(f'android:fillColor="{s["fill"]}"')
        if s["opacity"] != 1.0:
            attrs.append(f'android:fillAlpha="{s["opacity"]:g}"')
    if s["stroke"]:
        attrs += [f'android:strokeColor="{s["stroke"]}"', f'android:strokeWidth="{s["width"]:g}"',
                  'android:strokeLineCap="round"', 'android:strokeLineJoin="round"']
    path = f"{indent}<path " + f"\n{indent}    ".join(attrs) + " />"
    if s["rotate"]:
        a, x, y = s["rotate"]
        return [f'{indent}<group android:rotation="{a:g}" android:pivotX="{x:g}" android:pivotY="{y:g}">',
                "  " + path, f"{indent}</group>"]
    return [path]


def vd_items(items, indent="    "):
    out = []
    for it in items:
        if "group" in it:
            px, py = it["pivot"]
            out.append(f'{indent}<group android:name="{it["group"]}" android:pivotX="{px:g}" android:pivotY="{py:g}">')
            out += vd_items(it["items"], indent + "    ")
            out.append(f"{indent}</group>")
        else:
            out += vd_shape(it, indent)
    return out


def to_vector(items, size=200, dp=200):
    return "\n".join([
        '<?xml version="1.0" encoding="utf-8"?>',
        "<!-- Generated by tools/gen_mascots.py. Do not edit by hand. -->",
        '<vector xmlns:android="http://schemas.android.com/apk/res/android"',
        f'    android:width="{dp}dp" android:height="{dp}dp"',
        f'    android:viewportWidth="{size}" android:viewportHeight="{size}">',
        *vd_items(items), "</vector>\n"])


def to_animator(duration, prop, frames):
    kfs = "\n".join(f'        <keyframe android:fraction="{f:g}" android:value="{v:g}" />' for f, v in frames)
    return f"""<?xml version="1.0" encoding="utf-8"?>
<!-- Generated by tools/gen_mascots.py. Do not edit by hand. -->
<objectAnimator xmlns:android="http://schemas.android.com/apk/res/android"
    android:duration="{duration}"
    android:repeatCount="infinite"
    android:interpolator="@android:interpolator/linear">
    <propertyValuesHolder android:propertyName="{prop}" android:valueType="floatType">
{kfs}
    </propertyValuesHolder>
</objectAnimator>
"""


def to_avd(name, targets):
    lines = "\n".join(f'    <target android:name="{t}" android:animation="@animator/{a}" />'
                      for t, a in targets.items())
    return f"""<?xml version="1.0" encoding="utf-8"?>
<!-- Generated by tools/gen_mascots.py. Do not edit by hand. Start it from code (Mascot.kt). -->
<animated-vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:drawable="@drawable/mascot_{name}">
{lines}
</animated-vector>
"""


def names_in(items):
    for it in items:
        if "group" in it:
            yield it["group"]
            yield from names_in(it["items"])
        elif it["name"]:
            yield it["name"]


def main():
    brand = ROOT / "assets" / "brand"
    drawable = RES / "drawable"
    animator = RES / "animator"
    for d in (brand, drawable, animator):
        d.mkdir(parents=True, exist_ok=True)
    for name, items in MASCOTS.items():
        missing = set(TARGETS[name]) - set(names_in(items))
        assert not missing, f"{name}: animation targets without a named node: {missing}"
        (brand / f"mascot-{name}.svg").write_text(to_svg(items))
        (drawable / f"mascot_{name}.xml").write_text(to_vector(items))
        (drawable / f"mascot_{name}_anim.xml").write_text(to_avd(name, TARGETS[name]))
        print(f"wrote mascot-{name} (+ animated)")
    for name, (duration, prop, frames) in ANIMATORS.items():
        (animator / f"{name}.xml").write_text(to_animator(duration, prop, frames))
    (drawable / "ic_notification.xml").write_text(to_vector(NOTIFICATION, size=24, dp=24))
    print(f"wrote {len(ANIMATORS)} animators + ic_notification")


if __name__ == "__main__":
    main()
