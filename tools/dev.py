#!/usr/bin/env python3
"""Single entry point for humans, agents and CI. Run from anywhere:

    python3 tools/dev.py <command>

Commands (each prints what it runs and exits non-zero on failure):
  doctor         which toolchains are available here
  check          fast gate: rust fmt + clippy + tests, python selftest, docs lint
  rust           cargo test for the workspace
  interop        Rust <-> Python (+ Kotlin core if Gradle is available) wire tests
  android-test   :core JVM unit tests via Gradle
  android-build  assemble the debug APK (needs Android SDK)
  mascots        regenerate mascot SVG + Android vectors from tools/gen_mascots.py
  docs           lint AI docs: links in AGENTS.md/skills exist, skill frontmatter valid
  secrets        scan tracked + new files for credentials and personal e-mails
  contrast       WCAG AA contrast of every Android theme text/background pair
  a11y           axe-core audit of the download portal (needs Playwright + npm)
  all            check + interop + android-test (what CI runs on Linux)
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ANDROID = ROOT / "apps" / "android"


def run(cmd: list[str] | str, cwd: Path = ROOT, env: dict | None = None, capture: bool = False) -> str:
    shown = cmd if isinstance(cmd, str) else " ".join(cmd)
    print(f"$ {shown}", flush=True)
    res = subprocess.run(cmd, cwd=cwd, shell=isinstance(cmd, str), env={**os.environ, **(env or {})},
                         text=True, capture_output=capture)
    if res.returncode != 0:
        if capture:
            sys.stderr.write(res.stdout + res.stderr)
        sys.exit(f"✗ failed: {shown}")
    return res.stdout if capture else ""


def gradle() -> str | None:
    """Wrapper if present, else a gradle on PATH."""
    wrapper = ANDROID / ("gradlew.bat" if os.name == "nt" else "gradlew")
    if wrapper.exists():
        return str(wrapper)
    return shutil.which("gradle")


def cmd_doctor() -> None:
    tools = {
        "cargo": shutil.which("cargo"),
        "python3": sys.executable,
        "java": shutil.which("java"),
        "gradle (or wrapper)": gradle(),
        "ANDROID_HOME": os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT"),
    }
    for name, where in tools.items():
        print(f"{'✓' if where else '✗'} {name:<20} {where or 'missing'}")
    if not tools["ANDROID_HOME"]:
        print("  → Android app can't be built here; CI builds it (.github/workflows/ci.yml).")


def cmd_rust() -> None:
    run(["cargo", "test", "--workspace", "--quiet"])


def cmd_check() -> None:
    run(["cargo", "fmt", "--all", "--check"])
    run(["cargo", "clippy", "--workspace", "--all-targets", "--quiet", "--", "-D", "warnings"])
    cmd_rust()
    run([sys.executable, "tools/hoppy_ref.py", "selftest"])
    cmd_docs()
    cmd_contrast()
    cmd_secrets()


def cmd_interop() -> None:
    env = {}
    g = gradle()
    strict = bool(os.environ.get("CI"))  # in CI the Kotlin legs are mandatory
    if g and shutil.which("java"):
        print(f"$ {g} -q :core:printInteropClasspath", flush=True)
        res = subprocess.run([g, "-q", ":core:printInteropClasspath"], cwd=ANDROID, text=True, capture_output=True)
        lines = res.stdout.strip().splitlines()
        if res.returncode == 0 and lines:
            env["HOPPY_KT_CP"] = lines[-1]
        elif strict:
            sys.stderr.write(res.stdout + res.stderr)
            sys.exit("✗ could not build the Kotlin core for interop")
        else:
            print("(Gradle could not build :core here, e.g. offline: skipping the Kotlin legs)")
    elif strict:
        sys.exit("✗ CI needs Gradle + Java for the Kotlin interop legs")
    else:
        print("(no Gradle: skipping the Kotlin legs of the interop test)")
    run(["bash", "tools/interop.sh"], env=env)


def cmd_android_test() -> None:
    g = gradle() or sys.exit("✗ no Gradle; install it or open apps/android in Android Studio")
    run([g, ":core:test"], cwd=ANDROID)


def cmd_android_build() -> None:
    g = gradle() or sys.exit("✗ no Gradle")
    if not (os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT") or (ANDROID / "local.properties").exists()):
        sys.exit("✗ no Android SDK (set ANDROID_HOME)")
    run([g, ":app:assembleDebug"], cwd=ANDROID)
    print("APK: apps/android/app/build/outputs/apk/debug/app-debug.apk")


def cmd_mascots() -> None:
    run([sys.executable, "tools/gen_mascots.py"])


LINK = re.compile(r"`((?:docs|tools|crates|apps|assets|\.claude|\.github)/[^`\s]*?)`|\]\(((?!https?:)[^)#\s]+)")


def cmd_docs() -> None:
    """Every repo path mentioned in AI-facing docs must exist; skills need frontmatter."""
    files = [ROOT / "AGENTS.md", ROOT / "CLAUDE.md", *ROOT.glob("docs/**/*.md"),
             *ROOT.glob(".claude/skills/*/SKILL.md"), *ROOT.glob(".claude/agents/*.md")]
    bad = []
    for f in files:
        text = f.read_text(encoding="utf-8")
        for m in LINK.finditer(text):
            target = (m.group(1) or m.group(2)).rstrip(".,:;")
            if any(ch in target for ch in "*<>{}"):
                continue  # globs and placeholders
            base = ROOT if m.group(1) else f.parent
            if not (base / target).exists() and not (ROOT / target).exists():
                bad.append(f"{f.relative_to(ROOT)} → {target}")
    for skill in ROOT.glob(".claude/skills/*/SKILL.md"):
        head = skill.read_text(encoding="utf-8").split("---")
        if len(head) < 3 or "name:" not in head[1] or "description:" not in head[1]:
            bad.append(f"{skill.relative_to(ROOT)}: missing name/description frontmatter")
        elif f"name: {skill.parent.name}" not in head[1]:
            bad.append(f"{skill.relative_to(ROOT)}: name must equal folder name")
    if bad:
        print("\n".join("✗ " + b for b in bad))
        sys.exit(f"✗ docs lint: {len(bad)} problem(s)")
    print(f"✓ docs lint: {len(files)} files OK")


SECRET_PATTERNS = [
    (re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"), "private key"),
    (re.compile(r"\b(ghp|gho|ghu|ghs|github_pat)_[A-Za-z0-9_]{20,}"), "GitHub token"),
    (re.compile(r"\bAKIA[0-9A-Z]{16}\b"), "AWS access key"),
    (re.compile(r"\bAIza[0-9A-Za-z_\-]{35}\b"), "Google API key"),
    (re.compile(r"\bsk-[A-Za-z0-9_\-]{20,}"), "API secret key"),
    (re.compile(r"\bxox[abpors]-[A-Za-z0-9-]{10,}"), "Slack token"),
    (re.compile(r"(?i)\b(password|passwd|secret|api[_-]?key|token)\s*[:=]\s*['\"][^'\"\s]{6,}['\"]"), "hard-coded credential"),
    (re.compile(r"[A-Za-z0-9._%+-]+@(?!users\.noreply\.github\.com|anthropic\.com)[A-Za-z0-9.-]+\.[a-z]{2,}"), "e-mail address"),
]
SECRET_FILES = re.compile(r"(^|/)(\.env(\..*)?|.*\.(pem|key|p12|pfx|jks|keystore)|key(store)?\.properties|google-services\.json)$")


def cmd_secrets() -> None:
    """Fail if tracked or staged files contain credentials or personal e-mails."""
    files = run(["git", "ls-files", "--cached", "--others", "--exclude-standard"], capture=True).split("\n")
    bad = []
    for name in filter(None, files):
        if SECRET_FILES.search(name):
            bad.append(f"{name}: secret-like file name")
            continue
        path = ROOT / name
        if not path.is_file() or path.stat().st_size > 2_000_000:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue  # binary
        for lineno, line in enumerate(text.splitlines(), 1):
            for pattern, label in SECRET_PATTERNS:
                if pattern.search(line):
                    bad.append(f"{name}:{lineno}: {label}")
    if bad:
        print("\n".join("✗ " + b for b in bad))
        sys.exit("✗ secrets scan: remove these before committing")
    print("✓ secrets scan: clean")


def cmd_contrast() -> None:
    run([sys.executable, "tools/contrast.py"])


def cmd_a11y() -> None:
    run([sys.executable, "tools/site_audit.py"])


def cmd_all() -> None:
    cmd_check()
    cmd_interop()
    if gradle():
        cmd_android_test()


COMMANDS = {
    "doctor": cmd_doctor, "check": cmd_check, "rust": cmd_rust, "interop": cmd_interop,
    "android-test": cmd_android_test, "android-build": cmd_android_build,
    "mascots": cmd_mascots, "docs": cmd_docs, "secrets": cmd_secrets, "contrast": cmd_contrast, "a11y": cmd_a11y, "all": cmd_all,
}

if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1] not in COMMANDS:
        print(__doc__)
        sys.exit(2)
    COMMANDS[sys.argv[1]]()
