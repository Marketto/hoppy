//! Receiver-side file name sanitizing (spec §4). Test cases are shared with the
//! other implementations through `docs/test-vectors.json`.

use std::path::{Path, PathBuf};

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
const MAX_CHARS: usize = 200;
const MAX_EXT_CHARS: usize = 20;

/// Returns a safe file name, or `None` if nothing usable is left (offer is invalid).
pub fn sanitize_file_name(raw: &str) -> Option<String> {
    let unified = raw.replace('\\', "/");
    let last = unified.rsplit('/').next().unwrap_or("");
    let cleaned: String = last
        .chars()
        .filter(|c| !(('\u{0}'..='\u{1f}').contains(c) || *c == '\u{7f}'))
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c| c == ' ' || c == '.');
    if trimmed.is_empty() {
        return None;
    }
    let mut name = trimmed.to_string();
    let stem = name.split('.').next().unwrap_or("");
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        name.insert(0, '_');
    }
    Some(truncate(&name))
}

fn truncate(name: &str) -> String {
    let count = name.chars().count();
    if count <= MAX_CHARS {
        return name.to_string();
    }
    if let Some(dot) = name.rfind('.') {
        let ext = &name[dot..]; // includes the dot
        let ext_chars = ext.chars().count() - 1;
        if ext_chars <= MAX_EXT_CHARS && dot > 0 {
            let keep = MAX_CHARS - ext.chars().count();
            let stem: String = name[..dot].chars().take(keep).collect();
            return format!("{stem}{ext}");
        }
    }
    name.chars().take(MAX_CHARS).collect()
}

/// First path in `dir` named `name`, `name (1).ext`, `name (2).ext`, … that does not exist.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1u32..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("an unused name always exists")
}

/// Characters that must never reach a dialog: controls (Cc), line/paragraph separators,
/// and format characters (Cf) — bidi overrides/isolates/marks can make "gpj.exe" look
/// like "exe.jpg". ZWJ/ZWNJ (U+200C/D) are kept: they only join glyphs (emoji, Indic,
/// Persian) and cannot reorder text.
pub fn is_hidden_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061C}'
                | '\u{06DD}'
                | '\u{070F}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08E2}'
                | '\u{180E}'
                | '\u{200B}'
                | '\u{200E}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206F}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{110BD}'
                | '\u{110CD}'
                | '\u{13430}'..='\u{1343F}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0001}'
                | '\u{E0020}'..='\u{E007F}'
        )
}

/// Network text (device names, file names) made safe to show: hidden characters removed, trimmed, and cut to
/// `max` characters (ending in "…" when cut). May be empty.
pub fn display_text(raw: &str, max: usize) -> String {
    let clean: String = raw.chars().filter(|&c| !is_hidden_char(c)).collect();
    let clean = clean.trim();
    if clean.chars().count() <= max {
        return clean.to_owned();
    }
    let mut out: String = clean.chars().take(max.saturating_sub(1)).collect();
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Vectors {
        sanitize: Vec<Case>,
    }
    #[derive(serde::Deserialize)]
    struct Case {
        input: String,
        output: Option<String>,
    }

    #[test]
    fn shared_test_vectors() {
        let raw = include_str!("../../../docs/test-vectors.json");
        let v: Vectors = serde_json::from_str(raw).unwrap();
        assert!(!v.sanitize.is_empty());
        for case in v.sanitize {
            assert_eq!(
                sanitize_file_name(&case.input),
                case.output,
                "input {:?}",
                case.input
            );
        }
    }

    #[test]
    fn long_names_keep_extension() {
        let long = format!("{}.jpeg", "a".repeat(300));
        let out = sanitize_file_name(&long).unwrap();
        assert_eq!(out.chars().count(), 200);
        assert!(out.ends_with(".jpeg"));
    }

    #[test]
    fn unique_path_adds_counter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("a (1).txt"), b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.txt"),
            dir.path().join("a (2).txt")
        );
        assert_eq!(unique_path(dir.path(), "b"), dir.path().join("b"));
    }
}
