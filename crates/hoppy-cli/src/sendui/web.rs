//! The send window's page, embedded in the binary and served by the `hoppy:` custom
//! protocol (`window.rs`): nothing is read from disk and nothing comes from the network.
//! Unknown paths are 404; every response carries the CSP below.

use std::borrow::Cow;
use std::sync::OnceLock;

/// Same policy as the page's `<meta>`: only our own files, no network, no inline script.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; \
img-src 'self' data:; connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; \
worker-src 'none'; base-uri 'none'; form-action 'none'";

/// The custom protocol's name: `hoppy://localhost/` on macOS/Linux,
/// `http://hoppy.localhost/` on Windows (WebView2's mapping).
pub const SCHEME: &str = "hoppy";

/// Start page URL for this platform.
pub fn start_url() -> &'static str {
    if cfg!(windows) {
        "http://hoppy.localhost/index.html"
    } else {
        "hoppy://localhost/index.html"
    }
}

/// Whether a navigation to `url` stays inside the page (anything else is refused).
pub fn is_own_url(url: &str) -> bool {
    let rest = if cfg!(windows) {
        url.strip_prefix("http://hoppy.localhost/")
            .or_else(|| url.strip_prefix("https://hoppy.localhost/"))
    } else {
        url.strip_prefix("hoppy://localhost/")
    };
    rest.is_some()
}

const INDEX: &str = include_str!("web/index.html");
const CSS: &str = include_str!("web/app.css");
const JS: &str = include_str!("web/app.js");
const KAWAII: &str = include_str!("../../../../assets/brand/mascot-kawaii.svg");
const HAPPY: &str = include_str!("../../../../assets/brand/mascot-happy.svg");
const PRO: &str = include_str!("../../../../assets/brand/mascot-pro.svg");

/// The fonts of the five themes (WOFF2 subsets, `tools/fonts.py --desktop`).
const FONTS: [(&str, &[u8]); 9] = [
    (
        "mochiy_pop_one",
        include_bytes!("web/fonts/mochiy_pop_one.woff2"),
    ),
    (
        "zen_maru_gothic_medium",
        include_bytes!("web/fonts/zen_maru_gothic_medium.woff2"),
    ),
    (
        "zen_maru_gothic_black",
        include_bytes!("web/fonts/zen_maru_gothic_black.woff2"),
    ),
    (
        "dela_gothic_one",
        include_bytes!("web/fonts/dela_gothic_one.woff2"),
    ),
    (
        "zen_old_mincho_bold",
        include_bytes!("web/fonts/zen_old_mincho_bold.woff2"),
    ),
    (
        "zen_kaku_gothic_new_medium",
        include_bytes!("web/fonts/zen_kaku_gothic_new_medium.woff2"),
    ),
    (
        "zen_kaku_gothic_new_black",
        include_bytes!("web/fonts/zen_kaku_gothic_new_black.woff2"),
    ),
    (
        "hachi_maru_pop",
        include_bytes!("web/fonts/hachi_maru_pop.woff2"),
    ),
    (
        "kiwi_maru_medium",
        include_bytes!("web/fonts/kiwi_maru_medium.woff2"),
    ),
];

/// The page with the three mascots inlined (CSS animates their ears, eyes and glint).
pub fn index_html() -> &'static str {
    static PAGE: OnceLock<String> = OnceLock::new();
    PAGE.get_or_init(|| {
        INDEX
            .replace("<!--MASCOT:kawaii-->", KAWAII.trim())
            .replace("<!--MASCOT:happy-->", HAPPY.trim())
            .replace("<!--MASCOT:pro-->", PRO.trim())
    })
}

/// A file of the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub mime: &'static str,
    pub body: Cow<'static, [u8]>,
}

/// The file at `path` (URL path, query ignored), or `None` (404).
pub fn asset(path: &str) -> Option<Asset> {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let (mime, body): (&str, &'static [u8]) = match path {
        "/" | "/index.html" => ("text/html; charset=utf-8", index_html().as_bytes()),
        "/app.css" => ("text/css; charset=utf-8", CSS.as_bytes()),
        "/app.js" => ("text/javascript; charset=utf-8", JS.as_bytes()),
        _ => {
            let name = path.strip_prefix("/fonts/")?.strip_suffix(".woff2")?;
            let (_, data) = FONTS.iter().find(|(n, _)| *n == name)?;
            ("font/woff2", data)
        }
    };
    Some(Asset {
        mime,
        body: Cow::Borrowed(body),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_has_the_mascots_and_its_files() {
        let html = index_html();
        assert!(!html.contains("<!--MASCOT"), "every mascot inlined");
        assert_eq!(html.matches("<svg").count(), 3);
        assert!(html.contains(r#"href="app.css""#) && html.contains(r#"src="app.js""#));
        // No inline script or style blocks (the CSP would block them anyway).
        assert!(!html.contains("<script>") && !html.contains("<style"));
        assert!(!html.contains(" style=\"") && !html.contains(" onclick="));
        for p in ["/", "/index.html", "/app.css", "/app.js?x=1"] {
            assert!(asset(p).is_some(), "{p}");
        }
        assert_eq!(
            asset("/app.js").unwrap().mime,
            "text/javascript; charset=utf-8"
        );
    }

    #[test]
    fn every_font_in_the_css_is_embedded() {
        for url in CSS.split("url(\"fonts/").skip(1) {
            let file = url.split('"').next().unwrap();
            let a = asset(&format!("/fonts/{file}")).unwrap_or_else(|| panic!("{file}"));
            assert_eq!(a.mime, "font/woff2");
            assert!(a.body.starts_with(b"wOF2"), "{file}");
        }
        assert_eq!(CSS.matches("url(\"fonts/").count(), FONTS.len());
    }

    #[test]
    fn nothing_else_is_served() {
        for p in [
            "/../Cargo.toml",
            "/fonts/../app.js",
            "/fonts/x.woff2",
            "/fonts/kiwi_maru_medium.ttf",
            "/etc/passwd",
            "",
            "index.html",
            "/INDEX.HTML",
        ] {
            assert!(asset(p).is_none(), "{p}");
        }
    }

    #[test]
    fn only_own_urls_are_navigable() {
        assert!(is_own_url(start_url()));
        for bad in [
            "https://example.org/",
            "file:///etc/passwd",
            "about:blank",
            "hoppy://evil.example/",
            "http://hoppy.localhost.evil/",
            "javascript:alert(1)",
        ] {
            assert!(!is_own_url(bad), "{bad}");
        }
    }

    #[test]
    fn csp_matches_the_meta_tag() {
        let meta = INDEX
            .split("Content-Security-Policy\" content=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap();
        assert_eq!(meta, CSP);
        assert!(!CSP.contains("unsafe"));
    }
}
