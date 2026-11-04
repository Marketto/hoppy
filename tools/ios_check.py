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
- Protocol v1 (ADR 0010): every node of the app and the Share extension starts through
  `Identity.startNode` (persistent key_file/trust_file in Application Support, never in
  Documents); verification is the 4-emoji code compared by eye only, so no camera, no
  NSCameraUsageDescription, no QR or digit code anywhere; the update check asks for the
  release asset the release workflow publishes.
- Terms / privacy (docs/legal/README.md): Hoppy/Legal (legal.json with every modal key in
  it/en, version = docs/legal/version.json, the six pages) is a folder resource of the app and
  of the Share extension; the Swift code reads only keys that exist; both gate their node on
  the acceptance; the page viewer has JavaScript off; no quit button; Shared/ stays
  extension-safe (no UIApplication.shared).
"""
from __future__ import annotations

import json
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
    for lang in LANGS:
        if "NSLocalNetworkUsageDescription" not in tables["Hoppy/InfoPlist"][lang]:
            bad.append(f"apps/ios/Hoppy/{lang}.lproj/InfoPlist.strings: missing NSLocalNetworkUsageDescription")
    # Verification is the 4 emoji compared by eye only: Hoppy never asks for the camera.
    for name, plist in (("Hoppy", app), ("ShareExtension", ext)):
        if "NSCameraUsageDescription" in plist:
            bad.append(f"apps/ios/{name}/Info.plist: NSCameraUsageDescription must not exist (no camera, emoji-only verification)")
    for lang in LANGS:
        if "NSCameraUsageDescription" in tables["Hoppy/InfoPlist"][lang]:
            bad.append(f"apps/ios/Hoppy/{lang}.lproj/InfoPlist.strings: NSCameraUsageDescription must not exist")
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

    # 4b. Protocol v1 identity, emoji-only verification, updates (ADR 0010)
    identity = (IOS / "Shared" / "Identity.swift").read_text(encoding="utf-8")
    for needle in (".key_file = keyFile.path", ".trust_file = trustFile.path", ".applicationSupportDirectory", "isExcludedFromBackup"):
        if needle not in identity:
            bad.append(f"apps/ios/Shared/Identity.swift: {needle!r} missing (persistent v1 identity)")
    for folder in ("Hoppy", "ShareExtension"):
        for f in (IOS / folder).glob("*.swift"):
            text = f.read_text(encoding="utf-8")
            if "HoppyNode(config:" in text:
                bad.append(f"{f.relative_to(ROOT)}: start nodes with Identity.startNode (persistent key), not HoppyNode(config:)")
            if "Identity.startNode" in text and folder == "ShareExtension" and "shareExtensionName" not in text:
                bad.append(f"{f.relative_to(ROOT)}: the Share extension must use Identity.shareExtensionName")
    forbidden = (("AVCapture", "camera capture"), ("CIQRCodeGenerator", "QR rendering"),
                 ("hoppy_node_qr_", "QR exports"), ("qr_done", "the qr_done event"),
                 ("sas.digits", "the digit code"), ("let digits", "the digit code"))
    for f in swift:
        text = f.read_text(encoding="utf-8")
        for needle, what in forbidden:
            if needle in text:
                bad.append(f"{f.relative_to(ROOT)}: {what} ({needle!r}): verification is the 4 emoji compared by eye only")
    if (IOS / "Hoppy" / "QRViews.swift").exists():
        bad.append("apps/ios/Hoppy/QRViews.swift: QR verification was removed (emoji only)")
    update = IOS / "Hoppy" / "UpdateCheck.swift"
    release = (ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
    if not update.exists() or 'asset = "Hoppy-ios-unsigned.ipa"' not in update.read_text(encoding="utf-8") \
            or "Hoppy-ios-unsigned.ipa" not in release:
        bad.append("apps/ios/Hoppy/UpdateCheck.swift: must look for the release asset Hoppy-ios-unsigned.ipa")

    # 4c. Terms / privacy acceptance (docs/legal/README.md): the generated Legal/ folder is a
    # resource of the app AND the Share extension (each asks itself: no App Group), the modal
    # keys the Swift code reads exist in it/en, the node starts only after acceptance, the page
    # viewer runs no JavaScript, and there is no quit button (Apple HIG).
    legal_dir = IOS / "Hoppy" / "Legal"
    legal_keys = ("title", "body", "updated_title", "updated_body", "accept_terms", "approve_clauses",
                  "read_privacy", "continue", "legal", "terms", "privacy", "licenses")
    legal: dict = {}
    try:
        legal = json.loads((legal_dir / "legal.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        bad.append(f"apps/ios/Hoppy/Legal/legal.json: {e} (python3 tools/dev.py legal)")
    if legal:
        try:
            wanted = json.loads((ROOT / "docs" / "legal" / "version.json").read_text(encoding="utf-8"))
            if legal.get("version") != wanted:
                bad.append(f"apps/ios/Hoppy/Legal/legal.json: version {legal.get('version')} != docs/legal/version.json {wanted} (python3 tools/dev.py legal)")
        except (OSError, ValueError) as e:
            bad.append(f"docs/legal/version.json: {e}")
        for lang in LANGS:
            for key in legal_keys:
                if not legal.get("strings", {}).get(lang, {}).get(key):
                    bad.append(f"apps/ios/Hoppy/Legal/legal.json: strings.{lang}.{key} missing")
        for page in ("terms", "privacy", "licenses"):
            for lang in LANGS:
                name = legal.get("pages", {}).get(page, {}).get(lang)
                if not name or not (legal_dir / name).is_file():
                    bad.append(f"apps/ios/Hoppy/Legal: page {page}.{lang} ({name}) missing")
    blocks = dict(re.findall(r"^  (\w+):\n((?:    .*\n|\s*\n)*)", spec.split("\ntargets:\n", 1)[-1].split("\nschemes:", 1)[0], re.M))
    for target in ("Hoppy", "HoppyShare"):
        if not re.search(r"- path: Hoppy/Legal\n\s+type: folder\n\s+buildPhase: resources", blocks.get(target, "")):
            bad.append(f"apps/ios/project.yml: target {target} must bundle Hoppy/Legal (type: folder, buildPhase: resources)")
    if not re.search(r"- path: Hoppy\n\s+excludes: \[\"Legal\"\]", blocks.get("Hoppy", "")):
        bad.append('apps/ios/project.yml: target Hoppy must exclude "Legal" from its Hoppy source group (it is a folder reference)')
    used_keys: set[str] = set()
    for f in swift:
        text = f.read_text(encoding="utf-8")
        # texts.text("key") and texts.text(cond ? "a" : "b"): every quoted key in the call.
        for args in re.findall(r'\btext\(([^()]*"[^()]*)\)', text):
            used_keys |= set(re.findall(r'"(\w+)"', args))
        if '.text("quit")' in text:
            bad.append(f"{f.relative_to(ROOT)}: no quit button on iOS (Apple HIG): the app stays on the acceptance screen")
    for key in sorted(used_keys - set(legal.get("strings", {}).get("en", {}))):
        bad.append(f"apps/ios: legal string {key!r} is not in Legal/legal.json")
    for path, needles in (
        (IOS / "Hoppy" / "AppModel.swift", ("LegalConsent.needsAcceptance", "guard node == nil, !isTesting, !needsLegal")),
        (IOS / "Hoppy" / "ContentView.swift", ("fullScreenCover", "LegalConsentView", "LegalLinks")),
        (IOS / "ShareExtension" / "ShareViewController.swift", ("LegalConsent.needsAcceptance", "LegalConsentView",
                                                                "guard node == nil, !needsLegal")),
        (IOS / "Shared" / "LegalViews.swift", ("allowsContentJavaScript = false", "loadFileURL", ".nonPersistent()")),
    ):
        text = path.read_text(encoding="utf-8") if path.exists() else ""
        for needle in needles:
            if needle not in text:
                bad.append(f"{path.relative_to(ROOT)}: {needle!r} missing (terms acceptance, docs/legal/README.md)")
    for f in (IOS / "Shared").glob("*.swift"):
        if "UIApplication.shared" in f.read_text(encoding="utf-8"):
            bad.append(f"{f.relative_to(ROOT)}: UIApplication.shared is not extension-safe (Shared/ is in HoppyShare)")

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
