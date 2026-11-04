#!/usr/bin/env python3
"""Third-party licences of everything Hoppy ships, from the build manifests.

    python3 tools/licenses.py           # writes docs/legal/third-party-licenses.{md,json}
    python3 tools/licenses.py --check   # exit 1 if the committed files are out of date

Sources (stdlib only, deterministic output):
- Rust: `cargo metadata --locked` (offline first) filtered per shipped target: hoppy-cli for
  Windows, Linux and macOS (both architectures), hoppy-ffi for iOS. Normal dependencies only
  (dev and build dependencies are not distributed). Licence texts and copyright lines come
  from each crate's LICENSE/COPYING/NOTICE files in the Cargo registry.
- Android: `apps/android/gradle/libs.versions.toml` (implementation libraries only; test
  libraries are not shipped), with the known licence of each library.
- Fonts: the OFL texts bundled by tools/fonts.py (Android assets, desktop send window,
  portal).

When a crate offers a choice ("MIT OR Apache-2.0"), Hoppy uses it under the first licence
in PREFERENCE for which the crate ships a text. Each distinct licence text is written once;
copyright lines are listed per component. E-mail addresses are removed from copyright lines
(AGENTS.md invariant 9); the linked repositories carry the originals.
docs/legal/README.md explains how the output feeds the portal and the apps (tools/legal.py).
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_MD = ROOT / "docs/legal/third-party-licenses.md"
OUT_JSON = ROOT / "docs/legal/third-party-licenses.json"
ANDROID_TOML = ROOT / "apps/android/gradle/libs.versions.toml"
ANDROID_FONTS = ROOT / "apps/android/app/src/main/assets/licenses/fonts"
DESKTOP_FONTS = ROOT / "crates/hoppy-cli/src/sendui/web/fonts"
PORTAL_FONTS = {"mochiy-pop-one", "zen-maru-gothic"}  # tools/site.py WEB_FONTS

# (platform, target triple, root package name) — the binaries in the release.
TARGETS = [
    ("windows", "x86_64-pc-windows-msvc", "hoppy"),
    ("linux", "x86_64-unknown-linux-gnu", "hoppy"),
    ("macos", "aarch64-apple-darwin", "hoppy"),
    ("macos", "x86_64-apple-darwin", "hoppy"),
    ("ios", "aarch64-apple-ios", "hoppy-ffi"),
]
PREFERENCE = ["MIT", "Apache-2.0", "Zlib", "BSD-3-Clause", "ISC", "Unicode-3.0", "MPL-2.0",
              "Unlicense", "OFL-1.1"]
NAMES = {
    "MIT": "MIT License",
    "Apache-2.0": "Apache License 2.0",
    "Zlib": "zlib License",
    "BSD-3-Clause": "BSD 3-Clause License",
    "ISC": "ISC License",
    "Unicode-3.0": "Unicode License v3",
    "MPL-2.0": "Mozilla Public License 2.0",
    "Unlicense": "The Unlicense",
    "OFL-1.1": "SIL Open Font License 1.1",
}
# First line of the licence body (everything before it is title/copyright).
ANCHORS = {
    "MIT": re.compile(r"Permission is hereby granted, free of charge"),
    "Apache-2.0": re.compile(r"^\s*Apache License\s*$|Apache License,? Version 2\.0, January 2004"),
    "Zlib": re.compile(r"provided ['\"‘’“”]as-is['\"‘’“”]"),
    "BSD-3-Clause": re.compile(r"Redistribution and use in source and binary forms"),
    "ISC": re.compile(r"Permission to use, copy, modify, and(/or)? distribute"),
    "Unicode-3.0": re.compile(r"NOTICE TO USER"),
    "MPL-2.0": re.compile(r"Mozilla Public License,? [Vv]ersion 2\.0"),
    "Unlicense": re.compile(r"This is free and unencumbered software"),
    "OFL-1.1": re.compile(r"This Font Software is licensed under the SIL Open Font License"),
}
CLASSIFY = [  # (id, test on the whole text); order matters
    ("Unicode-3.0", lambda t: "UNICODE LICENSE V3" in t or "Unicode, Inc." in t and "NOTICE TO USER" in t),
    ("OFL-1.1", lambda t: "SIL Open Font License" in t or "SIL OPEN FONT LICENSE" in t),
    ("MPL-2.0", lambda t: "Mozilla Public License" in t),
    ("Apache-2.0", lambda t: "Apache License" in t and ("Version 2.0" in t or "VERSION 2.0" in t)),
    ("Unlicense", lambda t: "This is free and unencumbered software" in t),
    ("BSD-3-Clause", lambda t: "Redistribution and use in source and binary forms" in t and "Neither the name" in t),
    ("Zlib", lambda t: bool(ANCHORS["Zlib"].search(t)) and "source versions must be plainly marked" in t.lower()),
    ("ISC", lambda t: bool(ANCHORS["ISC"].search(t)) and "above copyright notice" in t),
    ("MIT", lambda t: "Permission is hereby granted, free of charge" in t and "shall be included" in t),
]
COPYRIGHT = re.compile(r"^\s*(?:Copyright\b|COPYRIGHT\b|©|\([cC]\)\s*\d)")
NOT_COPYRIGHT = re.compile(r"\[yyyy\]|\{yyyy\}|<year>|\[year\]|COPYRIGHT HOLDER|Copyright Holder|"
                           r"COPYRIGHT AND PERMISSION|copyright notice|Copyright License|Copyright \(C\) <")
EMAIL = re.compile(r"\s*[<(]?(?:mailto:)?[\w.+-]+@[\w-]+(?:\.[\w-]+)+[>)]?")
LICENSE_FILE = re.compile(r"^(LICEN[CS]E|COPYING|UNLICENSE|NOTICE)", re.I)

ANDROID_LIBS = {  # module → (licence, url, copyright)
    "androidx.core:core-ktx": ("Apache-2.0", "https://developer.android.com/jetpack/androidx/releases/core",
                               "Copyright The Android Open Source Project"),
    "androidx.appcompat:appcompat": ("Apache-2.0", "https://developer.android.com/jetpack/androidx/releases/appcompat",
                                     "Copyright The Android Open Source Project"),
    "androidx.activity:activity-ktx": ("Apache-2.0", "https://developer.android.com/jetpack/androidx/releases/activity",
                                       "Copyright The Android Open Source Project"),
    "androidx.recyclerview:recyclerview": ("Apache-2.0",
                                           "https://developer.android.com/jetpack/androidx/releases/recyclerview",
                                           "Copyright The Android Open Source Project"),
    "com.google.android.material:material": ("Apache-2.0",
                                             "https://github.com/material-components/material-components-android",
                                             "Copyright The Android Open Source Project"),
    "org.bouncycastle:bcprov-jdk18on": ("MIT", "https://www.bouncycastle.org/about/license/",
                                        "Copyright (c) 2000-2024 The Legion of the Bouncy Castle Inc. "
                                        "(https://www.bouncycastle.org)"),
}
KOTLIN_STDLIB = ("org.jetbrains.kotlin:kotlin-stdlib", "Apache-2.0", "https://github.com/JetBrains/kotlin",
                 "Copyright 2010-2024 JetBrains s.r.o. and Kotlin Programming Language contributors")


class Expr:
    """Tiny SPDX expression parser: OR of ANDs, parentheses, `WITH`, legacy `/` = OR."""

    def __init__(self, text: str):
        self.toks = re.findall(r"\(|\)|[^\s()]+", text.replace("/", " OR "))
        self.i = 0

    def parse(self) -> list[list[str]]:  # list of alternatives, each a list of ids (AND)
        alts = self._or()
        return alts

    def _peek(self):
        return self.toks[self.i] if self.i < len(self.toks) else None

    def _or(self):
        alts = self._and()
        while self._peek() == "OR":
            self.i += 1
            alts = alts + self._and()
        return alts

    def _and(self):
        alts = self._atom()
        while self._peek() == "AND":
            self.i += 1
            right = self._atom()
            alts = [a + b for a in alts for b in right]
        return alts

    def _atom(self):
        tok = self._peek()
        self.i += 1
        if tok == "(":
            alts = self._or()
            self.i += 1  # ")"
            return alts
        if self._peek() == "WITH":
            self.i += 2
            return [[f"{tok} WITH {self.toks[self.i - 1]}"]]
        return [[tok]]


def classify(text: str) -> str | None:
    for lid, test in CLASSIFY:
        if test(text):
            return lid
    return None


def body(lid: str, text: str) -> str | None:
    lines = text.replace("\r\n", "\n").split("\n")
    for n, line in enumerate(lines):
        if ANCHORS[lid].search(line):
            out = [ln.rstrip() for ln in lines[n:]]
            while out and not out[-1]:
                out.pop()
            return "\n".join(out)
    return None


def copyright_lines(text: str) -> list[str]:
    out = []
    for line in text.replace("\r\n", "\n").split("\n"):
        if COPYRIGHT.match(line) and not NOT_COPYRIGHT.search(line):
            clean = " ".join(EMAIL.sub("", line).split()).rstrip(",;")
            if len(clean) > len("Copyright") + 2 and clean not in out:
                out.append(clean)
    return out


def norm(text: str) -> str:
    return " ".join(text.split())


def cargo_metadata(triple: str) -> dict:
    base = ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", triple]
    for extra in (["--offline"], []):
        res = subprocess.run(base + extra, cwd=ROOT, capture_output=True, text=True)
        if res.returncode == 0:
            return json.loads(res.stdout)
    sys.exit(f"✗ cargo metadata failed for {triple}:\n{res.stderr}")


def rust_components(texts: dict[str, Counter], notices: list) -> list[dict]:
    crates: dict[tuple, dict] = {}
    for platform, triple, root in TARGETS:
        meta = cargo_metadata(triple)
        pkgs = {p["id"]: p for p in meta["packages"]}
        nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
        members = set(meta["workspace_members"])
        start = next(i for i in members if pkgs[i]["name"] == root)
        seen, stack = set(), [start]
        while stack:
            i = stack.pop()
            if i in seen:
                continue
            seen.add(i)
            for d in nodes[i]["deps"]:
                if any(k["kind"] is None for k in d["dep_kinds"]):
                    stack.append(d["pkg"])
        for i in seen - members:
            p = pkgs[i]
            key = (p["name"], p["version"])
            if key not in crates:
                crates[key] = {"pkg": p, "platforms": set()}
            crates[key]["platforms"].add(platform)

    out = []
    for (name, version), entry in sorted(crates.items()):
        p = entry["pkg"]
        expr = p.get("license") or ""
        folder = Path(p["manifest_path"]).parent
        files = {}
        for f in sorted(folder.iterdir()):
            if f.is_file() and LICENSE_FILE.match(f.name):
                try:
                    files[f.name] = f.read_text(encoding="utf-8", errors="replace")
                except OSError:
                    continue
        by_id: dict[str, list[str]] = {}
        for fname, text in files.items():
            if fname.upper().startswith("NOTICE"):
                continue
            lid = classify(text)
            if lid:
                by_id.setdefault(lid, []).append(fname)
        alts = Expr(expr).parse() if expr else [["NOASSERTION"]]

        def rank(alt: list[str]) -> tuple:
            known = all(x in PREFERENCE for x in alt)
            shipped = all(x in by_id for x in alt)
            return (not known, not shipped, sum(PREFERENCE.index(x) if x in PREFERENCE else 99 for x in alt))

        chosen = sorted(alts, key=rank)[0]
        if not all(x in PREFERENCE for x in chosen):
            sys.exit(f"✗ {name} {version}: no supported licence in {expr!r}; extend PREFERENCE/NAMES/ANCHORS")
        lines: list[str] = []
        for lid in chosen:
            for fname in by_id.get(lid, []):
                b = body(lid, files[fname])
                if b:
                    texts.setdefault(lid, Counter())[b] += 1
                lines += [ln for ln in copyright_lines(files[fname]) if ln not in lines]
        if not lines:
            authors = [" ".join(EMAIL.sub("", a).split()) for a in p.get("authors") or []]
            authors = [a for a in authors if a]
            lines = [f"Copyright the {name} authors" + (f" ({', '.join(authors)})" if authors else "")]
        for fname, text in files.items():
            if fname.upper().startswith("NOTICE") and any(x.startswith("Apache") for x in chosen):
                notices.append({"component": f"{name} {version}", "text": EMAIL.sub("", text).strip()})
        out.append({
            "kind": "rust", "name": name, "version": version, "license": expr, "applies": chosen,
            "platforms": sorted(entry["platforms"], key=["windows", "macos", "linux", "ios"].index),
            "url": p.get("repository") or f"https://crates.io/crates/{name}", "copyright": lines,
        })
    return out


def android_components() -> list[dict]:
    toml = tomllib.loads(ANDROID_TOML.read_text(encoding="utf-8"))
    versions, libs = toml["versions"], toml["libraries"]
    out = []
    for lib in libs.values():
        module = lib["module"]
        if module not in ANDROID_LIBS:
            continue  # test-only (junit, kotlin-test): not shipped
        lid, url, notice = ANDROID_LIBS[module]
        out.append({"kind": "android", "name": module, "version": versions[lib["version"]["ref"]]
                    if isinstance(lib.get("version"), dict) else str(lib.get("version", "")),
                    "license": lid, "applies": [lid], "platforms": ["android"], "url": url,
                    "copyright": [notice]})
    used = {lib["module"] for lib in libs.values()}
    missing = set(ANDROID_LIBS) - used
    if missing:
        sys.exit(f"✗ ANDROID_LIBS lists modules no longer in {ANDROID_TOML.name}: {sorted(missing)}")
    module, lid, url, notice = KOTLIN_STDLIB
    out.append({"kind": "android", "name": module, "version": versions["kotlin"], "license": lid,
                "applies": [lid], "platforms": ["android"], "url": url, "copyright": [notice]})
    return sorted(out, key=lambda c: c["name"])


def font_components(texts: dict[str, Counter]) -> list[dict]:
    out = []
    for f in sorted(ANDROID_FONTS.glob("*-OFL.txt")):
        slug = f.name[: -len("-OFL.txt")]
        text = f.read_text(encoding="utf-8")
        b = body("OFL-1.1", text)
        if b:
            texts.setdefault("OFL-1.1", Counter())[b] += 1
        platforms = ["android"]
        if (DESKTOP_FONTS / f.name).exists():
            platforms[:0] = ["windows", "macos", "linux"]
        if slug in PORTAL_FONTS:
            platforms.append("portal")
        url = next((m.group(1) for m in [re.search(r"\((https://[^)]+)\)", text)] if m),
                   "https://fonts.google.com")
        out.append({"kind": "font", "name": " ".join(w.capitalize() for w in slug.split("-")),
                    "version": "", "license": "OFL-1.1", "applies": ["OFL-1.1"], "platforms": platforms,
                    "url": url, "copyright": copyright_lines(text)})
    return out


def collect() -> dict:
    texts: dict[str, Counter] = {}
    notices: list = []
    components = rust_components(texts, notices) + android_components() + font_components(texts)
    used = sorted({lid for c in components for lid in c["applies"]}, key=PREFERENCE.index)
    licenses = {}
    for lid in used:
        variants = texts.get(lid)
        if not variants:
            sys.exit(f"✗ no text found for {lid} in any shipped component")
        # The most common wording wins; ties are broken alphabetically (deterministic).
        best = sorted(variants.items(), key=lambda kv: (-kv[1], norm(kv[0])))[0][0]
        licenses[lid] = {"name": NAMES[lid], "text": best}
    return {
        "generated_by": "tools/licenses.py",
        "project": {"name": "Hoppy", "license": "MIT", "copyright": "Copyright (c) 2026 Marco Ricupero",
                    "url": "https://github.com/Marketto/hoppy"},
        "components": components,
        "licenses": licenses,
        "notices": sorted(notices, key=lambda n: n["component"]),
    }


PLATFORM_LABEL = {"windows": "Windows", "macos": "macOS", "linux": "Linux", "ios": "iOS",
                  "android": "Android", "portal": "portal"}
NOT_DISTRIBUTED = [
    "Python reference implementation and tools (`tools/`): Python standard library only.",
    "Portal build: segno, fontTools, brotli (run by `tools/site.py`; only their output — a QR "
    "code and WOFF2 conversions of the fonts above — is published).",
    "Tests and audits: JUnit, kotlin-test, Playwright, axe-core, Rust dev-dependencies.",
    "Build tools: Rust toolchain, Gradle and the Android Gradle Plugin, Xcode/XcodeGen, cargo-deb, "
    "Inno Setup (jrsoftware.org; its setup program is part of the Windows installer under the "
    "Inno Setup License).",
]
SYSTEM = [
    "Windows: Microsoft Edge WebView2 Runtime (part of Windows / installed by Microsoft).",
    "Linux: WebKitGTK, GTK, D-Bus, zenity/kdialog, curl/wget (system packages).",
    "macOS and iOS: Apple system frameworks (AppKit, WebKit, UIKit, Foundation…).",
    "Android: the Android platform libraries.",
]


def md_escape(s: str) -> str:
    return s.replace("|", "\\|")


def render_md(data: dict) -> str:
    comps = data["components"]
    L = ["# Third-party licences", "",
         "<!-- Generated by tools/licenses.py from Cargo.lock, apps/android/gradle/libs.versions.toml "
         "and the bundled font licences. Do not edit: run `python3 tools/licenses.py`. -->", "",
         f"Hoppy itself is released under the MIT licence, {data['project']['copyright']} "
         f"(<{data['project']['url']}>). It includes the third-party components listed below, each "
         "provided by its authors **\"as is\", without warranty of any kind**, under its own licence. "
         "When a component offers a choice of licences, Hoppy uses it under the licence in the "
         "*Used under* column. Each licence text is reproduced once, after the copyright notices of "
         "the components that use it. E-mail addresses are omitted from the copyright notices; the "
         "originals are in each component's repository.", "",
         "| Licence | Components |", "| --- | --- |"]
    count = Counter(lid for c in comps for lid in c["applies"])
    for lid in data["licenses"]:
        L.append(f"| [{NAMES[lid]}](#{anchor(lid)}) | {count[lid]} |")
    sections = [("rust", "Rust crates (desktop apps; the core of the iPhone/iPad app)", "Crate"),
                ("android", "Android libraries (and their AndroidX/Kotlin dependencies, Apache 2.0)", "Library"),
                ("font", "Fonts", "Font")]
    for kind, title, col in sections:
        L += ["", f"## {title}", "", f"| {col} | Version | Licence | Used under | Platforms |",
              "| --- | --- | --- | --- | --- |"]
        for c in (c for c in comps if c["kind"] == kind):
            L.append(f"| [{md_escape(c['name'])}]({c['url']}) | {c['version'] or '—'} | {md_escape(c['license'])} | "
                     f"{' AND '.join(c['applies'])} | {', '.join(PLATFORM_LABEL[p] for p in c['platforms'])} |")
    L += ["", "## Original assets", "",
          "Mascots, music, sounds and icons are original works of the Hoppy project, under its MIT licence.",
          "", "## Not distributed", "", "Used only to build or test Hoppy, not shipped to users:", ""]
    L += [f"- {s}" for s in NOT_DISTRIBUTED]
    L += ["", "System components used by the apps but not distributed with them (each under its "
          "vendor's terms):", ""]
    L += [f"- {s}" for s in SYSTEM]
    L += ["", "## Copyright notices and licence texts"]
    for lid, lic in data["licenses"].items():
        L += ["", f"### {lic['name']}", "", f"Used by {count[lid]} component(s):", ""]
        for c in (c for c in comps if lid in c["applies"]):
            label = f"{c['name']} {c['version']}".strip()
            L.append(f"- **{md_escape(label)}**: {md_escape('; '.join(c['copyright']))}")
        L += ["", "```text", lic["text"], "```"]
    if data["notices"]:
        L += ["", "## NOTICE files", "", "Reproduced as required by the Apache License 2.0, section 4(d)."]
        for n in data["notices"]:
            L += ["", f"### {n['component']}", "", "```text", n["text"], "```"]
    return "\n".join(L) + "\n"


def anchor(lid: str) -> str:
    return re.sub(r"[^a-z0-9 -]", "", NAMES[lid].lower()).replace(" ", "-")


def outputs() -> dict[Path, str]:
    data = collect()
    return {OUT_JSON: json.dumps(data, ensure_ascii=False, indent=1) + "\n", OUT_MD: render_md(data)}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="fail if the committed files are out of date")
    a = ap.parse_args()
    stale = []
    for path, text in outputs().items():
        rel = path.relative_to(ROOT)
        if a.check:
            if not path.exists() or path.read_text(encoding="utf-8") != text:
                stale.append(str(rel))
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
            print(f"✓ wrote {rel}")
    if stale:
        print("✗ out of date (run `python3 tools/licenses.py`): " + ", ".join(stale))
        return 1
    if a.check:
        print("✓ third-party licences up to date")
    return 0


if __name__ == "__main__":
    sys.exit(main())
