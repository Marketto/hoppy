#!/usr/bin/env python3
"""Accessibility audit + screenshots of the desktop send window (crates/hoppy-cli/src/sendui).

    python3 tools/sendui_audit.py [--shots DIR] [--lang it|en]

Renders the real page (index.html with the mascots inlined, app.css, app.js, fonts) in
Playwright Chromium at the window's size, feeds it every sample view produced by Rust
(`hoppyw --dump-views`, built with cargo), in every theme, and runs axe-core (WCAG 2.2 AA +
best practice) on each; with reduced motion so nothing moves. `--shots DIR` also saves
`send-ui-chromium-<lang>-<theme>-<screen>.png`. Exits 1 on any violation.

Needs: cargo, npm (axe-core), Playwright for Python (+ chromium).
"""
from __future__ import annotations

import functools
import http.server
import json
import subprocess
import sys
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "crates/hoppy-cli/src/sendui/web"
BRAND = ROOT / "assets/brand"
THEMES = ["purikura", "showa", "manga", "yume", "night"]
PORT = 8798

sys.path.insert(0, str(ROOT / "tools"))
from site_audit import axe_source  # noqa: E402

IPC_STUB = "window.ipc = { postMessage: (m) => { (window.__sent = window.__sent || []).push(m); } };"


def page_html() -> str:
    html = (WEB / "index.html").read_text(encoding="utf-8")
    for name in ("kawaii", "happy", "pro"):
        html = html.replace(f"<!--MASCOT:{name}-->", (BRAND / f"mascot-{name}.svg").read_text(encoding="utf-8").strip())
    return html


def dump_views(lang: str) -> dict:
    out = subprocess.run(["cargo", "run", "-q", "-p", "hoppy", "--bin", "hoppyw", "--", "--dump-views", "--lang", lang],
                         cwd=ROOT, check=True, capture_output=True, text=True).stdout
    return json.loads(out.strip().splitlines()[-1])


def main() -> int:
    from playwright.sync_api import sync_playwright

    args = sys.argv[1:]
    shots = Path(args[args.index("--shots") + 1]) if "--shots" in args else None
    lang = args[args.index("--lang") + 1] if "--lang" in args else "it"
    data = dump_views(lang)
    html = page_html().encode()

    class Handler(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):  # noqa: N802
            if self.path in ("/", "/index.html"):
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.end_headers()
                self.wfile.write(html)
                return
            super().do_GET()

    server = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), functools.partial(Handler, directory=str(WEB)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    axe = axe_source()
    failures = 0
    with sync_playwright() as p:
        browser = p.chromium.launch()
        ctx = browser.new_context(viewport={"width": 420, "height": 580}, reduced_motion="reduce", locale=f"{lang}-IT",
                                  bypass_csp=True)  # only to inject axe; the page itself never needs it
        page = ctx.new_page()
        page.add_init_script(IPC_STUB)
        page.goto(f"http://127.0.0.1:{PORT}/index.html")
        page.wait_for_function("window.hoppy && (window.__sent || []).length > 0")
        page.add_script_tag(content=axe)
        for theme in THEMES:
            for item in data["views"]:
                page.evaluate("([i, v]) => { hoppy.init(i); hoppy.render(v); }",
                              [{"theme": theme, "lang": data["lang"], "title": data["title"]}, item["view"]])
                page.evaluate("document.fonts.ready")
                page.wait_for_timeout(60)
                res = page.evaluate("""async () => axe.run(document, { runOnly: { type: 'tag',
                    values: ['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa', 'best-practice'] } })""")
                problems = [(v["id"], n["target"], n.get("failureSummary", "").split("\n")[-1].strip())
                            for v in res["violations"] for n in v["nodes"]]
                # Keyboard: something is focused, and it is a button.
                focus = page.evaluate("document.activeElement && document.activeElement.tagName")
                if focus != "BUTTON":
                    problems.append(("focus", item["name"], f"focus on {focus}, not a button"))
                if shots:
                    shots.mkdir(parents=True, exist_ok=True)
                    page.screenshot(path=str(shots / f"send-ui-chromium-{lang}-{theme}-{item['name']}.png"))
                mark = "✓" if not problems else "✗"
                print(f"{mark} {theme:<9} {item['name']:<14} {len(problems)} problem(s)")
                for pid, target, why in problems:
                    print(f"   - {pid} {target} {why}")
                failures += len(problems)
        # Keyboard flow on the device list: arrows move, Enter picks, Esc asks to leave.
        devices = next(i for i in data["views"] if i["name"] == "devices")
        page.evaluate("v => { window.__sent = []; hoppy.render(v); }", devices["view"])
        page.keyboard.press("ArrowDown")
        page.keyboard.press("Enter")
        page.keyboard.press("Escape")
        sent = page.evaluate("window.__sent")
        if sent != ['{"t":"pick","i":1}', '{"t":"escape"}']:
            print(f"✗ keyboard: sent {sent}")
            failures += 1
        else:
            print("✓ keyboard: ↓ Enter picks the 2nd device, Esc asks to close")
        browser.close()
    server.shutdown()
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
