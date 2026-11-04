# Legal texts: terms, privacy, third-party licences

Single source of truth for everything legal that Hoppy shows to users. Italian and English
are kept in step; the Italian text prevails (terms §9). These are templates written for the
project, **not legal advice**: have them reviewed by a lawyer before a wider distribution.

| File | What | Edited by |
| --- | --- | --- |
| `docs/legal/terms.it.md`, `docs/legal/terms.en.md` | Terms of use and disclaimer (AS IS, user responsibilities, release and indemnity of the author and of the library authors, sideloading, changes, law) | hand |
| `docs/legal/privacy.it.md`, `docs/legal/privacy.en.md` | Privacy policy (GDPR): what stays on the device, what the LAN sees, GitHub update checks, portal | hand |
| `docs/legal/version.json` | `{"terms": N, "privacy": N}`: the version the apps ask the user to accept | hand |
| `docs/legal/strings.json` | Copy of the acceptance modal (IT/EN, same keys) | hand |
| `docs/legal/third-party-licenses.md`, `docs/legal/third-party-licenses.json` | Every shipped third-party component, its licence, copyright lines and each licence text once | `tools/licenses.py` |
| `site/legal/*.html` | Portal pages (portal CSS, light/dark, language switch, footer links) | `tools/legal.py` |
| `crates/hoppy-cli/src/sendui/web/legal/` | Desktop apps (Windows, macOS, Linux): pages + `legal.json` | `tools/legal.py` |
| `apps/android/app/src/main/assets/legal/` | Android app: pages + `legal.json` | `tools/legal.py` |
| `apps/ios/Hoppy/Legal/` | iPhone/iPad app: pages + `legal.json` | `tools/legal.py` |
| `apps/windows/legal/terms.{it,en}.txt` | Plain-text terms (UTF-8 + BOM): Windows installer licence page, .dmg, .deb docs | `tools/legal.py` |

Generated files are never edited by hand (like mascots and music, AGENTS.md invariant 6):

```
python3 tools/dev.py licenses   # after a dependency/font change: licences md+json, then the pages
python3 tools/dev.py legal      # after editing a .md, version.json or strings.json
```

`python3 tools/dev.py check` runs `tools/licenses.py --check` and `tools/legal.py --check`, so a
stale output fails CI. `tools/site.py build` renders the portal pages again from the sources;
`tools/site_audit.py` (`python3 tools/dev.py a11y`) audits all six of them in light/dark, desktop/phone.

## Changing the terms or the privacy policy

1. Edit both languages. Keep the line `**Versione N — <date>**` / `**Version N — <date>**`
   right under the title.
2. For a change in substance, bump the number in `version.json` **and** in both headers
   (`tools/legal.py` refuses a mismatch). Typos and wording that do not change the meaning
   keep the number (users are not asked again).
3. `python3 tools/dev.py legal`, then commit sources and outputs together.

## For app integrators

Every app directory holds the same set of files:

- `terms.{it,en}.html`, `privacy.{it,en}.html`, `licenses.{it,en}.html`: self-contained pages
  (inline CSS, `prefers-color-scheme` light/dark, system font, no scripts, no network). Links
  between them are relative (`privacy.it.html`); `https://` links must open in the system
  browser. Each app's licences page lists only the components that app ships (desktop:
  Windows/macOS/Linux crates + desktop fonts; Android: AndroidX, Material, BouncyCastle,
  Kotlin, fonts; iOS: the crates of `hoppy-ffi`).
- `legal.json`: `{"version": {"terms": N, "privacy": N}, "strings": {"it": {…}, "en": {…}},
  "pages": {"terms": {"it": "terms.it.html", "en": "terms.en.html"}, …}}`. String keys:
  `title`, `body`, `updated_title`, `updated_body`, `accept_terms`, `approve_clauses`,
  `read_privacy`, `continue`, `quit`, `legal`, `terms`, `privacy`, `licenses`.

Required behaviour (owner request; the terms rely on it):

- **At first run**, before anything else (before discovery starts or a file is sent or
  received), show a modal with `title` + `body`, links to the Terms and the Privacy policy, a
  checkbox `accept_terms`, a checkbox `read_privacy` and, recommended for Italian law
  (arts. 1341–1342 c.c.), a separate checkbox `approve_clauses`. `continue` stays disabled until
  every checkbox is ticked; `quit` closes the app without doing anything.
- Store the accepted `version` (both numbers) locally with the other settings. When either
  number in the bundled `legal.json` is higher than the stored one, show the modal again with
  `updated_title` / `updated_body`.
- Somewhere always reachable (the About dialog, the send window's footer, the overflow menu):
  small links `terms` · `privacy` · `licenses` opening the bundled pages in the user's language
  (Italian if the system language is Italian, else English).
- The command-line `hoppy` has no modal; it should mention the terms in `--help`.

Where it is implemented: desktop (Windows, macOS, Linux) — `crates/hoppy-cli/src/legal.rs`
(versions, `device.json` acceptance, pages), the send window's legal screen (`sendui`, also run
alone as `hoppyw legal` before the tray starts) with a native fallback dialog, About → *Legal…*,
`hoppy legal` + `--help`; installer licence page = `apps/windows/legal/terms.{it,en}.txt`
(`tools/legal.py`), also in the .dmg and `/usr/share/doc/hoppy/legal/`
(`docs/platforms/windows.md` "Terms and privacy").
Android — `ui/LegalActivity.kt` + `ui/LegalPageActivity.kt`, gate in
`Node.acquire` / `ReceiveService` (`docs/platforms/android.md` "PoC experience").
