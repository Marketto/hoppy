#!/usr/bin/env python3
"""Build (and optionally preview) the download portal in site/.

    python3 tools/site.py build --out _site --base-url https://<owner>.github.io/<repo> --repo <owner>/<repo>
    python3 tools/site.py serve            # build to a temp dir and serve on http://localhost:8000

Steps: copy site/, fill __BASE_URL__/__REPO__, generate qr.svg pointing at the base URL,
convert the bundled app fonts (already subsetted by tools/fonts.py) to WOFF2, copy the
icon, write sidestore.json (AltStore/SideStore source of the iPhone/iPad app, from the
latest GitHub release). Needs: pip install segno fonttools brotli.
Deployed by .github/workflows/pages.yml on pushes to main that touch the site, on every
published release and when release.yml asks for it after publishing (so the source always
lists the newest .ipa). `--require-release` (CI) fails instead of writing a source without
versions when the GitHub API cannot be reached; GH_TOKEN/GITHUB_TOKEN raise the rate limit.
"""
from __future__ import annotations

import argparse
import functools
import http.server
import json
import os
import shutil
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
FONTS = ROOT / "apps/android/app/src/main/res/font"
WEB_FONTS = ["mochiy_pop_one", "zen_maru_gothic_medium", "zen_maru_gothic_black"]
DEFAULT_REPO = "Marketto/hoppy"
IOS_ASSET = "Hoppy-ios-unsigned.ipa"  # release.yml job `ios`; never rename (docs/release.md)
IOS_BUNDLE_ID = "app.hoppy.ios"
IOS_MIN_OS = "15.0"
SOURCE_FILE = "sidestore.json"


def latest_release(repo: str, timeout: float = 20) -> dict | None:
    """`releases/latest` of the repository; None when there is no release yet (404)."""
    req = urllib.request.Request(f"https://api.github.com/repos/{repo}/releases/latest",
                                 headers={"Accept": "application/vnd.github+json", "User-Agent": "hoppy-site"})
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:  # noqa: S310 (fixed https URL)
            return json.load(r)
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        raise


def sidestore_source(base_url: str, repo: str, release: dict | None) -> dict:
    """AltStore/SideStore source (https://faq.altstore.io/developers/make-a-source) listing the
    iPhone/iPad app with the .ipa of `release`, or no version when it has none. The version must
    equal the app's CFBundleShortVersionString (build-ipa.sh drops a `-suffix`), or SideStore
    refuses the download."""
    app = {
        "name": "Hoppy",
        "bundleIdentifier": IOS_BUNDLE_ID,
        "developerName": "Marco Ricupero",
        "subtitle": "Send files to nearby devices with a hop.",
        "localizedDescription": (
            "Hoppy sends files between nearby devices (Windows, Mac, Ubuntu, Android, iPhone and iPad) "
            "on the same Wi-Fi: Share → Hoppy, tap a device, done. Encrypted, with optional device "
            "verification. Receives while the app is open."),
        "iconURL": f"{base_url}/hoppy-512.png",
        "tintColor": "#A82B52",
        "category": "utilities",
        "screenshots": [],
        "versions": [],
        # Declared because SideStore/AltStore compare them with the app's Info.plist.
        "appPermissions": {
            "entitlements": [],
            "privacy": {
                "NSLocalNetworkUsageDescription":
                    "Hoppy looks for your devices on this Wi-Fi network to send and receive files.",
                "NSCameraUsageDescription":
                    "Hoppy uses the camera only to scan the QR code of another device and verify it.",
            },
        },
    }
    asset = next((a for a in (release or {}).get("assets", []) if a.get("name") == IOS_ASSET), None)
    if release and asset:
        tag = str(release.get("tag_name", ""))
        version = tag[1:] if tag.startswith("v") else tag
        app["versions"].append({
            "version": version.split("-", 1)[0],
            "date": release.get("published_at") or release.get("created_at"),
            "localizedDescription": f"Hoppy {version}: {release.get('html_url', f'https://github.com/{repo}/releases')}",
            "downloadURL": f"https://github.com/{repo}/releases/download/{tag}/{IOS_ASSET}",
            "size": int(asset.get("size", 0)),
            "minOSVersion": IOS_MIN_OS,
        })
    return {
        "name": "Hoppy",
        "identifier": "io.github.marketto.hoppy",
        "subtitle": "Hoppy for iPhone and iPad",
        "description": "Official source of Hoppy (unsigned .ipa from the GitHub releases).",
        "iconURL": f"{base_url}/hoppy-512.png",
        "website": f"{base_url}/",
        "tintColor": "#A82B52",
        "featuredApps": [IOS_BUNDLE_ID],
        "apps": [app],
        "news": [],
    }


def build(out: Path, base_url: str, repo: str, require_release: bool = False) -> None:
    import segno
    from fontTools.ttLib import TTFont

    base_url = base_url.rstrip("/")
    if out.exists():
        shutil.rmtree(out)
    shutil.copytree(SITE, out)
    for f in out.rglob("*.html"):
        f.write_text(f.read_text(encoding="utf-8").replace("__BASE_URL__", base_url).replace("__REPO__", repo),
                     encoding="utf-8")
    for f in out.rglob("*.js"):
        f.write_text(f.read_text(encoding="utf-8").replace("__REPO__", repo), encoding="utf-8")

    segno.make(base_url + "/", error="m").save(str(out / "qr.svg"), scale=8, border=2, dark="#5B3A44")
    (out / "fonts").mkdir(exist_ok=True)
    for name in WEB_FONTS:
        font = TTFont(FONTS / f"{name}.ttf")
        font.flavor = "woff2"
        font.save(out / "fonts" / f"{name}.woff2")
    shutil.copy(ROOT / "assets/brand/hoppy-512.png", out / "hoppy-512.png")
    try:
        release = latest_release(repo)
    except (OSError, ValueError) as e:
        if require_release:
            raise SystemExit(f"✗ {SOURCE_FILE}: cannot read the latest release of {repo}: {e}") from e
        print(f"! {SOURCE_FILE}: latest release unavailable ({e}); writing it without versions")
        release = None
    source = sidestore_source(base_url, repo, release)
    (out / SOURCE_FILE).write_text(json.dumps(source, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    versions = [v["version"] for v in source["apps"][0]["versions"]]
    print(f"✓ {SOURCE_FILE}: {', '.join(versions) if versions else 'no iOS release yet'}")
    (out / ".nojekyll").write_text("")
    print(f"✓ site built in {out} for {base_url}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("--out", type=Path, default=ROOT / "_site")
    b.add_argument("--base-url", default=f"https://{DEFAULT_REPO.split('/')[0].lower()}.github.io/{DEFAULT_REPO.split('/')[1]}")
    b.add_argument("--repo", default=DEFAULT_REPO)
    b.add_argument("--require-release", action="store_true",
                   help=f"fail if the latest release cannot be read (CI), instead of an empty {SOURCE_FILE}")
    s = sub.add_parser("serve")
    s.add_argument("--port", type=int, default=8000)
    s.add_argument("--repo", default=DEFAULT_REPO)
    a = ap.parse_args()

    if a.cmd == "build":
        build(a.out, a.base_url, a.repo, a.require_release)
        return 0
    out = Path(tempfile.mkdtemp(prefix="hoppy-site-"))
    build(out, f"http://localhost:{a.port}", a.repo)
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(out))
    print(f"→ http://localhost:{a.port}/  (Ctrl+C to stop)")
    http.server.ThreadingHTTPServer(("127.0.0.1", a.port), handler).serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
