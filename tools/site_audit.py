#!/usr/bin/env python3
"""Accessibility audit of the download portal with axe-core (WCAG 2.2 AA).

    python3 tools/site_audit.py            # builds the site, audits 4 views, exits 1 on violations

Views: desktop/phone × light/dark, Italian, with animations frozen (reduced motion) and a
fake release so every button is rendered. In each view every "Come funziona" OS tab is
selected in turn and its mockups audited as well. axe cannot compute contrast for text over the
animated checkerboard; those "incomplete" items are re-checked here against BOTH tile
colors, so the worst case must still pass.

Needs: npm (for axe-core), Playwright for Python (+ chromium), segno, fonttools, brotli.
"""
from __future__ import annotations

import functools
import http.server
import json
import subprocess
import sys
import tarfile
import tempfile
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
import importlib.util  # noqa: E402

_spec = importlib.util.spec_from_file_location("hoppy_site", ROOT / "tools" / "site.py")
hoppy_site = importlib.util.module_from_spec(_spec)  # tools/site.py ("site" clashes with the stdlib)
_spec.loader.exec_module(hoppy_site)

FAKE_RELEASE = {"tag_name": "v0.1.0", "published_at": "2026-10-14T18:00:00Z",
                "assets": [{"name": "Hoppy-Setup-x64.exe"}, {"name": "Hoppy-android.apk"}]}
VIEWS = [
    ("desktop-light", {"width": 1280, "height": 900}, "light", False),
    ("desktop-dark", {"width": 1280, "height": 900}, "dark", False),
    ("phone-light", {"width": 390, "height": 844}, "light", True),
    ("phone-dark", {"width": 390, "height": 844}, "dark", True),
]
DESKTOP_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36"
PHONE_UA = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Mobile Safari/537.36"

# Re-check text drawn straight on the page background against both checkerboard tiles.
WORST_CASE_JS = """
() => {
  const lum = (c) => { const m = c.match(/[\\d.]+/g).map(Number); const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); };
    return 0.2126 * f(m[0]) + 0.7152 * f(m[1]) + 0.0722 * f(m[2]); };
  const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
  const css = getComputedStyle(document.documentElement);
  const tiles = ['--tile-a', '--tile-b', '--bg'].map((v) => {
    const el = document.createElement('div'); el.style.color = css.getPropertyValue(v).trim(); document.body.appendChild(el);
    const c = getComputedStyle(el).color; el.remove(); return c; });
  const out = [];
  const opaqueBg = (el) => { for (let e = el; e && e !== document.body; e = e.parentElement) {
      const s = getComputedStyle(e); if (s.backgroundImage !== 'none' && e !== el) return true;
      const m = s.backgroundColor.match(/[\\d.]+/g); if (m && (m.length < 4 || Number(m[3]) > 0.6)) return true; } return false; };
  for (const el of document.querySelectorAll('body *')) {
    if (!el.childNodes.length || ![...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim())) continue;
    const s = getComputedStyle(el); if (s.visibility === 'hidden' || s.display === 'none' || !el.getClientRects().length) continue;
    if (opaqueBg(el)) continue;
    const size = parseFloat(s.fontSize), bold = Number(s.fontWeight) >= 700;
    // Outlined (sticker) text: legibility comes from the outline, so compare fill vs outline.
    const outline = s.textShadow !== 'none' && s.textShadow.match(/rgba?\\([^)]*\\)/);
    if (outline) { const r = ratio(s.color, outline[0]); if (r < 4.5) out.push({ text: el.textContent.trim().slice(0, 40), ratio: r.toFixed(2), need: 4.5 }); continue; }
    const need = (size >= 24 || (bold && size >= 18.66)) ? 3 : 4.5;
    const worst = Math.min(...tiles.map((t) => ratio(s.color, t)));
    if (worst < need) out.push({ text: el.textContent.trim().slice(0, 40), ratio: worst.toFixed(2), need });
  }
  return out;
}
"""


def axe_source() -> str:
    tmp = Path(tempfile.mkdtemp())
    tgz = subprocess.run(["npm", "pack", "-q", "axe-core"], cwd=tmp, check=True, capture_output=True,
                         text=True).stdout.strip().splitlines()[-1]
    with tarfile.open(tmp / tgz) as t:
        t.extractall(tmp, filter="data")
    return (tmp / "package/axe.min.js").read_text()


def main() -> int:
    from playwright.sync_api import sync_playwright

    out = Path(tempfile.mkdtemp(prefix="hoppy-audit-"))
    hoppy_site.build(out, "http://127.0.0.1:8799", hoppy_site.DEFAULT_REPO)
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(out))

    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *args):
            pass

    handler = functools.partial(Quiet, directory=str(out))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 8799), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    axe = axe_source()

    failures = 0
    with sync_playwright() as p:
        browser = p.chromium.launch()
        for name, viewport, scheme, phone in VIEWS:
            ctx = browser.new_context(viewport=viewport, color_scheme=scheme, reduced_motion="reduce",
                                      user_agent=PHONE_UA if phone else DESKTOP_UA, is_mobile=phone,
                                      has_touch=phone, locale="it-IT")
            page = ctx.new_page()
            page.route("https://api.github.com/**", lambda r: r.fulfill(
                status=200, content_type="application/json", body=json.dumps(FAKE_RELEASE)))
            page.goto("http://127.0.0.1:8799/")
            page.wait_for_timeout(600)
            page.add_script_tag(content=axe)
            res = page.evaluate("""async () => axe.run(document, {
                runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa', 'best-practice'] } })""")
            worst = page.evaluate(WORST_CASE_JS)
            problems = [(v["id"], v["impact"], n["target"], n.get("failureSummary", "").split("\n")[-1].strip())
                        for v in res["violations"] for n in v["nodes"]]
            problems += [("contrast-on-checkerboard", "serious", w["text"], f"{w['ratio']}:1 < {w['need']}:1")
                         for w in worst]
            # "Come funziona": every OS tab draws different mockups; audit each one too.
            for tab in page.eval_on_selector_all("#os-tabs [role=tab]", "els => els.map((e) => e.id)"):
                page.click(f"#{tab}")
                page.wait_for_timeout(50)
                res = page.evaluate("""async () => axe.run('#come-funziona', {
                    runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa', 'best-practice'] } })""")
                problems += [(v["id"], v["impact"], f"{tab} {n['target']}", n.get("failureSummary", "").split("\n")[-1].strip())
                             for v in res["violations"] for n in v["nodes"]]
            print(f"{'✓' if not problems else '✗'} {name}: {len(problems)} problem(s)")
            for pid, impact, target, why in problems:
                print(f"   - [{impact}] {pid} {target} {why}")
            failures += len(problems)
            ctx.close()
        browser.close()
    server.shutdown()
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
