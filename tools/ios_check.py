#!/usr/bin/env python3
"""Static checks of the iPhone/iPad app that run on Linux (part of `python3 tools/dev.py
check`); the real build and simulator tests run in CI (job `ios` in .github/workflows/ci.yml).

- Localizable.strings / .stringsdict / InfoPlist.strings: same keys and the same format
  specifiers in English and Italian; every key the Swift code uses exists.
- Info.plists parse and carry what the design needs (ADR 0009): Local Network text, Files
  sharing, no App Group / multicast entitlement, Share extension principal class.
- project.yml: every source path exists; iOS 15 everywhere.
- Every `hoppy_*` C function the Swift code calls is declared in crates/hoppy-ffi/include/hoppy.h.
- Generated assets exist: an opaque 1024 px app icon, the mascot image sets.
"""
from __future__ import annotations

import plistlib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
IOS = ROOT / "apps" / "ios"
HEADER = ROOT / "crates" / "hoppy-ffi" / "include" / "hoppy.h"
LANGS = ("en", "it")
STRINGS_LINE = re.compile(r'^"((?:[^"\\]|\\.)+)"\s*=\s*"((?:[^"\\]|\\.)*)";\s*$')
SPEC = re.compile(r"%(?:\d+\$)?(?:#@\w+@|[@dDuUxXfeEgGcCsSpaAF]|l[ldu]|ll[du])")


def parse_strings(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = line.strip()
        if not line or line.startswith(("/*", "//", "*")):
            continue
        m = STRINGS_LINE.match(line)
        if not m:
            raise ValueError(f"{path.relative_to(ROOT)}:{n}: not a \"key\" = \"value\"; line")
        out[m.group(1)] = m.group(2)
    return out


def specs(value: str) -> list[str]:
    """Format specifiers, positional ones normalised so order changes are allowed."""
    return sorted(re.sub(r"\d+\$", "", s) for s in SPEC.findall(value.replace("%%", "")))


def main() -> int:
    bad: list[str] = []

    # 1. Strings
    tables: dict[str, dict[str, dict[str, str]]] = {}
    for table, folder in (("Localizable", "Shared"), ("InfoPlist", "Hoppy"), ("InfoPlist", "ShareExtension")):
        per_lang = {}
        for lang in LANGS:
            path = IOS / folder / f"{lang}.lproj" / f"{table}.strings"
            try:
                per_lang[lang] = parse_strings(path)
            except (OSError, ValueError) as e:
                bad.append(str(e))
                per_lang[lang] = {}
        tables[f"{folder}/{table}"] = per_lang
        en, it = per_lang["en"], per_lang["it"]
        for k in sorted(set(en) ^ set(it)):
            bad.append(f"{folder}/{table}.strings: key {k!r} only in {'en' if k in en else 'it'}")
        for k in sorted(set(en) & set(it)):
            if specs(en[k]) != specs(it[k]):
                bad.append(f"{folder}/{table}.strings: {k!r} format differs: {en[k]!r} vs {it[k]!r}")
    plurals = {}
    for lang in LANGS:
        path = IOS / "Shared" / f"{lang}.lproj" / "Localizable.stringsdict"
        try:
            plurals[lang] = plistlib.loads(path.read_bytes())
        except Exception as e:  # noqa: BLE001
            bad.append(f"{path.relative_to(ROOT)}: {e}")
            plurals[lang] = {}
    for k in sorted(set(plurals["en"]) ^ set(plurals["it"])):
        bad.append(f"Localizable.stringsdict: key {k!r} missing in one language")
    for k, entry in plurals["en"].items():
        for lang in LANGS:
            e = plurals[lang].get(k, {})
            fmt = e.get("NSStringLocalizedFormatKey", "")
            var = re.findall(r"#@(\w+)@", fmt)
            if not var or var[0] not in e or "other" not in e[var[0]]:
                bad.append(f"{lang}.lproj/Localizable.stringsdict: {k!r} needs a variable with an 'other' form")
            elif specs(e[var[0]]["other"]) != specs(entry[re.findall(r'#@(\w+)@', entry['NSStringLocalizedFormatKey'])[0]]["other"]):
                bad.append(f"{lang}.lproj/Localizable.stringsdict: {k!r} format differs from English")
    known = set(tables["Shared/Localizable"]["en"]) | set(plurals["en"])
    swift = list(IOS.rglob("*.swift"))
    for f in swift:
        text = f.read_text(encoding="utf-8")
        for key in re.findall(r'\bL\("([^"]+)"', text) + re.findall(r'NSLocalizedString\("([^"]+)"', text):
            if key not in known:
                bad.append(f"{f.relative_to(ROOT)}: string {key!r} is not in Localizable.strings(dict)")

    # 2. Info.plists
    def load(p: Path) -> dict:
        try:
            return plistlib.loads(p.read_bytes())
        except Exception as e:  # noqa: BLE001
            bad.append(f"{p.relative_to(ROOT)}: {e}")
            return {}
    app = load(IOS / "Hoppy" / "Info.plist")
    ext = load(IOS / "ShareExtension" / "Info.plist")
    for key in ("NSLocalNetworkUsageDescription", "UIFileSharingEnabled", "LSSupportsOpeningDocumentsInPlace",
                "CFBundleDocumentTypes", "UILaunchScreen"):
        if key not in app:
            bad.append(f"apps/ios/Hoppy/Info.plist: missing {key}")
    if "NSLocalNetworkUsageDescription" not in ext:
        bad.append("apps/ios/ShareExtension/Info.plist: missing NSLocalNetworkUsageDescription")
    nsext = ext.get("NSExtension", {})
    if nsext.get("NSExtensionPointIdentifier") != "com.apple.share-services" or \
            nsext.get("NSExtensionPrincipalClass") != "$(PRODUCT_MODULE_NAME).ShareViewController":
        bad.append("apps/ios/ShareExtension/Info.plist: share-services extension with ShareViewController expected")
    if not any("ShareViewController" in f.read_text(encoding="utf-8") and "class ShareViewController" in f.read_text(encoding="utf-8")
               for f in (IOS / "ShareExtension").glob("*.swift")):
        bad.append("apps/ios/ShareExtension: class ShareViewController not found")
    for p in IOS.rglob("*.entitlements"):
        text = p.read_text(encoding="utf-8")
        for forbidden in ("application-groups", "networking.multicast", "icloud", "aps-environment"):
            if forbidden in text:
                bad.append(f"{p.relative_to(ROOT)}: {forbidden} needs a paid Apple account (ADR 0009)")

    # 3. project.yml
    spec = (IOS / "project.yml").read_text(encoding="utf-8")
    for m in re.finditer(r"^\s*-?\s*path:\s*(\S+)", spec, re.M):
        if not (IOS / m.group(1)).exists():
            bad.append(f"apps/ios/project.yml: path {m.group(1)} does not exist")
    for m in re.finditer(r"framework:\s*(\S+)", spec):
        if m.group(1) != "build/HoppyFFI.xcframework":
            bad.append(f"apps/ios/project.yml: unexpected framework {m.group(1)}")
    targets = set(re.findall(r'(?:iOS|IPHONEOS_DEPLOYMENT_TARGET):\s*"([\d.]+)"', spec))
    script = (IOS / "build-xcframework.sh").read_text(encoding="utf-8")
    targets |= set(re.findall(r"IPHONEOS_DEPLOYMENT_TARGET=([\d.]+)", script))
    if targets != {"15.0"}:
        bad.append(f"iOS deployment target must be 15.0 everywhere, found {sorted(targets)}")

    # 4. C functions used from Swift
    declared = set(re.findall(r"\b(hoppy_\w+)\s*\(", HEADER.read_text(encoding="utf-8")))
    for f in swift:
        for fn in set(re.findall(r"\b(hoppy_[a-z_]+)\s*\(", f.read_text(encoding="utf-8"))):
            if fn not in declared:
                bad.append(f"{f.relative_to(ROOT)}: {fn} is not declared in hoppy.h")

    # 5. Generated assets (tools/gen_icons.py --ios)
    icon = IOS / "Hoppy" / "Assets.xcassets" / "AppIcon.appiconset" / "AppIcon-1024.png"
    try:
        from PIL import Image
        with Image.open(icon) as im:
            if im.size != (1024, 1024) or im.mode != "RGB":
                bad.append(f"{icon.relative_to(ROOT)}: must be an opaque 1024x1024 PNG (is {im.size} {im.mode})")
    except ImportError:
        if not icon.exists():
            bad.append(f"{icon.relative_to(ROOT)}: missing (python3 tools/gen_icons.py --ios)")
    except OSError as e:
        bad.append(f"{icon.relative_to(ROOT)}: {e} (python3 tools/gen_icons.py --ios)")
    for name in ("Mascot", "MascotHappy"):
        for scale in (2, 3):
            p = IOS / "Shared" / "Mascots.xcassets" / f"{name}.imageset" / f"{name}-{scale}x.png"
            if not p.exists():
                bad.append(f"{p.relative_to(ROOT)}: missing (python3 tools/gen_icons.py --ios)")
    for name in ("Mascot", "MascotHappy"):
        if not any(f'"{name}"' in f.read_text(encoding="utf-8") for f in swift):
            bad.append(f"image {name} is never used")

    if bad:
        print("\n".join("✗ " + b for b in bad))
        print(f"✗ iOS static check: {len(bad)} problem(s)")
        return 1
    print(f"✓ iOS static check: {len(swift)} Swift files, strings en/it, plists, project.yml OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
