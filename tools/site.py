#!/usr/bin/env python3
"""Build (and optionally preview) the download portal in site/.

    python3 tools/site.py build --out _site --base-url https://<owner>.github.io/<repo> --repo <owner>/<repo>
    python3 tools/site.py serve            # build to a temp dir and serve on http://localhost:8000

Steps: copy site/, fill __BASE_URL__/__REPO__, generate qr.svg pointing at the base URL,
convert the bundled app fonts (already subsetted by tools/fonts.py) to WOFF2, copy the
icon. Needs: pip install segno fonttools brotli.
Deployed by .github/workflows/pages.yml on every push to main that touches the site.
"""
from __future__ import annotations

import argparse
import functools
import http.server
import shutil
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
FONTS = ROOT / "apps/android/app/src/main/res/font"
WEB_FONTS = ["mochiy_pop_one", "zen_maru_gothic_medium", "zen_maru_gothic_black"]
DEFAULT_REPO = "Marketto/hoppy"


def build(out: Path, base_url: str, repo: str) -> None:
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
    (out / ".nojekyll").write_text("")
    print(f"✓ site built in {out} for {base_url}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("--out", type=Path, default=ROOT / "_site")
    b.add_argument("--base-url", default=f"https://{DEFAULT_REPO.split('/')[0].lower()}.github.io/{DEFAULT_REPO.split('/')[1]}")
    b.add_argument("--repo", default=DEFAULT_REPO)
    s = sub.add_parser("serve")
    s.add_argument("--port", type=int, default=8000)
    s.add_argument("--repo", default=DEFAULT_REPO)
    a = ap.parse_args()

    if a.cmd == "build":
        build(a.out, a.base_url, a.repo)
        return 0
    out = Path(tempfile.mkdtemp(prefix="hoppy-site-"))
    build(out, f"http://localhost:{a.port}", a.repo)
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(out))
    print(f"→ http://localhost:{a.port}/  (Ctrl+C to stop)")
    http.server.ThreadingHTTPServer(("127.0.0.1", a.port), handler).serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
