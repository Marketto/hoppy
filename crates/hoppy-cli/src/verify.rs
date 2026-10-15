//! What the desktop verification dialogs show (spec §7.6–7.7), independent of the toolkit.
//!
//! `gui.rs` describes a dialog with a [`DialogView`] (text, the 4-emoji code with its names
//! and the same value as 8 digits behind a "123" toggle, an optional QR code, buttons) and
//! hands it to `ui::show_dialog` / `ui::ask_offer`: a custom Win32 window on Windows, an
//! `NSAlert` with an accessory view on macOS, zenity on Linux. While it is open, workers talk
//! to it through a [`DialogLive`] ("a phone just verified this device": hide the QR code and
//! show ✓; "close": the transfer ended). The QR code is a [`QrMatrix`] that each backend
//! draws itself: rectangles (GDI), a BMP (AppKit), Unicode half blocks (zenity).
//!
//! Pure Rust, compiled and tested on every platform.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use qrcode::{Color, EcLevel, QrCode};

/// Light modules around the code (the QR standard asks for 4).
pub const QR_QUIET: usize = 4;
/// How often an open dialog checks its [`DialogLive`].
pub const LIVE_POLL: Duration = Duration::from_millis(200);

/// A dialog to show. The first button is the default one (Enter/Return) and the answer when
/// the dialog is closed, times out or is closed through [`DialogLive::close`]: it is always
/// the safe choice ("Decline", "Close", "Cancel"), so a stray Enter never accepts files.
#[derive(Debug, Clone, Default)]
pub struct DialogView {
    pub title: String,
    /// Main text (already cleaned with `display_*`; plain text, may have line breaks).
    pub text: String,
    /// Security state and warnings, shown after `text` (may be empty).
    pub note: String,
    pub code: Option<CodeView>,
    pub qr: Option<QrView>,
    /// `(id, label)`; at least one. Ids are non-zero.
    pub buttons: Vec<(u32, String)>,
    /// Linux: label of the notification button that opens the full dialog (the accept
    /// question is a notification first). Unused elsewhere.
    pub details: Option<String>,
    /// No answer within this = the default button.
    pub timeout: Option<Duration>,
}

impl DialogView {
    /// The default (first) button's id; 0 if there are no buttons.
    pub fn default_id(&self) -> u32 {
        self.buttons.first().map_or(0, |b| b.0)
    }

    /// Everything as plain text, for places that can show only text (a notification body,
    /// kdialog, the log): text, the code in both forms with the emoji names, the note.
    pub fn plain_text(&self) -> String {
        let mut parts = vec![self.text.clone()];
        if !self.note.is_empty() {
            parts.push(self.note.clone());
        }
        if let Some(code) = &self.code {
            parts.push(code.one_line());
        }
        parts.retain(|p| !p.is_empty());
        parts.join("\n\n")
    }
}

/// The short authentication string as the user compares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeView {
    pub emoji: [&'static str; 4],
    /// Names of the emoji in the UI language (screen readers, and people who cannot tell two
    /// emoji apart: GDI draws them in one colour).
    pub names: [&'static str; 4],
    /// The same value as "1234 5678".
    pub digits: String,
    /// Label of the toggle when the emoji are shown ("123") and when the digits are.
    pub to_digits: String,
    pub to_emoji: String,
}

impl CodeView {
    pub fn new(sas: &hoppy_core::Sas, italian: bool, to_digits: &str, to_emoji: &str) -> Self {
        let words = sas.words(italian);
        let names = std::array::from_fn(|i| words.get(i).copied().unwrap_or(""));
        let emoji =
            std::array::from_fn(|i| hoppy_core::SAS_EMOJI[usize::from(sas.emoji[i] & 63)].emoji);
        Self {
            emoji,
            names,
            digits: sas.digits.clone(),
            to_digits: to_digits.to_owned(),
            to_emoji: to_emoji.to_owned(),
        }
    }

    /// "🐘 🐢 🐙 🦋" (spaced so each stands alone).
    pub fn emoji_row(&self) -> String {
        self.emoji.join("  ")
    }

    /// "elefante · tartaruga · polpo · farfalla".
    pub fn names_row(&self) -> String {
        self.names.join(" · ")
    }

    /// "🐘 🐢 🐙 🦋 (elefante, tartaruga, polpo, farfalla) · 1234 5678".
    pub fn one_line(&self) -> String {
        format!(
            "{} ({}) · {}",
            self.emoji.join(" "),
            self.names.join(", "),
            self.digits
        )
    }
}

/// A QR code on screen: what to scan, what to say under it, what to say once scanned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrView {
    pub matrix: QrMatrix,
    /// "Scan with Hoppy on the phone" (also the accessible description of the image).
    pub caption: String,
    /// Replaces the code once [`DialogLive::verified`] is set.
    pub verified: String,
}

/// The modules of a QR code, without the quiet zone. `true` = dark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrMatrix {
    width: usize,
    dark: Vec<bool>,
}

impl QrMatrix {
    /// Encodes `payload` (error correction M: ~15 % damage or glare tolerated). `None` if it
    /// does not fit a QR code (never for spec §7.7 payloads).
    pub fn encode(payload: &str) -> Option<Self> {
        let code = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::M).ok()?;
        let width = code.width();
        let dark = code
            .to_colors()
            .into_iter()
            .map(|c| c == Color::Dark)
            .collect();
        Some(Self { width, dark })
    }

    /// Modules per side, without the quiet zone.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Modules per side including the quiet zone on both sides.
    pub fn full_width(&self) -> usize {
        self.width + 2 * QR_QUIET
    }

    /// Module at (x, y) counted from the top-left corner of the quiet zone; outside the code
    /// (quiet zone, or beyond) = light.
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        let (Some(x), Some(y)) = (x.checked_sub(QR_QUIET), y.checked_sub(QR_QUIET)) else {
            return false;
        };
        x < self.width && y < self.width && self.dark[y * self.width + x]
    }

    /// The code as text, two modules per character cell (`█ ▀ ▄` and space; dark = glyph),
    /// quiet zone included, rows separated by `\n`. Shown in a monospace font, black on
    /// white, it scans like a picture (zenity on Linux).
    pub fn half_blocks(&self) -> String {
        let n = self.full_width();
        let mut rows = Vec::with_capacity(n.div_ceil(2));
        for y in (0..n).step_by(2) {
            let row: String = (0..n)
                .map(|x| match (self.is_dark(x, y), self.is_dark(x, y + 1)) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                })
                .collect();
            rows.push(row);
        }
        rows.join("\n")
    }

    /// The code as an uncompressed 24-bit BMP, `scale` pixels per module, quiet zone
    /// included (macOS: `NSImage` reads it as is; no image crate needed).
    pub fn bmp(&self, scale: usize) -> Vec<u8> {
        let scale = scale.max(1);
        let side = self.full_width() * scale;
        let row_len = (side * 3).div_ceil(4) * 4;
        let pixels = row_len * side;
        let file_len = 54 + pixels;
        let mut out = Vec::with_capacity(file_len);
        let u32le = |v: usize| u32::try_from(v).unwrap_or(u32::MAX).to_le_bytes();
        // BITMAPFILEHEADER
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&u32le(file_len));
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&u32le(54));
        // BITMAPINFOHEADER: positive height = bottom-up rows.
        out.extend_from_slice(&u32le(40));
        out.extend_from_slice(&u32le(side));
        out.extend_from_slice(&u32le(side));
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&[0; 4]); // BI_RGB
        out.extend_from_slice(&u32le(pixels));
        out.extend_from_slice(&u32le(2835)); // 72 dpi
        out.extend_from_slice(&u32le(2835));
        out.extend_from_slice(&[0; 8]);
        for py in (0..side).rev() {
            let start = out.len();
            for px in 0..side {
                let v = if self.is_dark(px / scale, py / scale) {
                    0
                } else {
                    255
                };
                out.extend_from_slice(&[v, v, v]);
            }
            out.resize(start + row_len, 0);
        }
        out
    }
}

/// Shared between an open dialog and the code that may change it from another thread.
/// Cheap to clone; every backend polls it every [`LIVE_POLL`].
#[derive(Debug, Clone, Default)]
pub struct DialogLive(Arc<LiveFlags>);

#[derive(Debug, Default)]
struct LiveFlags {
    verified: AtomicBool,
    closed: AtomicBool,
}

impl DialogLive {
    pub fn new() -> Self {
        Self::default()
    }

    /// The QR code was scanned and confirmed: show [`QrView::verified`] instead of it.
    pub fn set_verified(&self) {
        self.0.verified.store(true, Ordering::Release);
    }

    pub fn verified(&self) -> bool {
        self.0.verified.load(Ordering::Acquire)
    }

    /// Close the dialog as if its default button had been chosen.
    pub fn close(&self) {
        self.0.closed.store(true, Ordering::Release);
    }

    pub fn closed(&self) -> bool {
        self.0.closed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str = "hoppy:verify:1:3f2b8c1e-9a7d-4c55-8e21-0b6f4d2a9c11:\
                           MFRGGZDFMZTWQ2LKNNWG23TPOBYXE43UOV3HO6DZPJQWEY3EMVTGO2DJ:\
                           0123456789abcdef0123456789abcdef";

    #[test]
    fn qr_matrix_has_finder_patterns_and_a_quiet_zone() {
        let m = QrMatrix::encode(PAYLOAD).unwrap();
        assert_eq!(m.width() % 4, 1, "QR sides are 17 + 4·version");
        assert!(
            m.width() <= 57,
            "a §7.7 payload stays a small code: {}",
            m.width()
        );
        let q = QR_QUIET;
        // Top-left finder: dark ring, light ring, dark 3×3 centre.
        assert!(m.is_dark(q, q) && m.is_dark(q + 6, q + 6) && m.is_dark(q + 3, q + 3));
        assert!(!m.is_dark(q + 1, q + 1));
        // Quiet zone and beyond are light.
        assert!((0..m.full_width()).all(|i| !m.is_dark(i, 0) && !m.is_dark(0, i)));
        assert!(!m.is_dark(10_000, 10_000));
    }

    #[test]
    fn half_blocks_pack_two_rows_per_line() {
        let m = QrMatrix::encode("hoppy").unwrap();
        let text = m.half_blocks();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), m.full_width().div_ceil(2));
        assert!(lines.iter().all(|l| l.chars().count() == m.full_width()));
        // First line: the quiet zone (blank); third line, column 4: the finder's top edge,
        // dark in both of its module rows (y = 4, 5).
        assert!(lines[0].chars().all(|c| c == ' '));
        assert_eq!(lines[2].chars().nth(QR_QUIET), Some('█'));
        assert!(text.chars().all(|c| " █▀▄\n".contains(c)));
    }

    #[test]
    fn bmp_is_a_valid_bottom_up_bitmap() {
        let m = QrMatrix::encode("hoppy").unwrap();
        let scale = 3;
        let bmp = m.bmp(scale);
        let side = m.full_width() * scale;
        let row = (side * 3).div_ceil(4) * 4;
        assert_eq!(&bmp[..2], b"BM");
        let le = |at: usize| u32::from_le_bytes(bmp[at..at + 4].try_into().unwrap()) as usize;
        assert_eq!(le(2), bmp.len());
        assert_eq!(le(10), 54);
        assert_eq!((le(18), le(22)), (side, side));
        assert_eq!(bmp.len(), 54 + row * side);
        // Pixel (x, y) from the top lives in file row side-1-y.
        let pixel = |x: usize, y: usize| bmp[54 + (side - 1 - y) * row + x * 3];
        assert_eq!(pixel(0, 0), 255); // quiet zone
        let finder = QR_QUIET * scale;
        assert_eq!(pixel(finder, finder), 0);
        assert_eq!(pixel(finder + scale, finder + scale), 255);
    }

    #[test]
    fn code_view_shows_the_same_value_both_ways() {
        let sas = hoppy_core::Sas::from_value(0x05_1234);
        let it = CodeView::new(&sas, true, "123", "Emoji");
        let en = CodeView::new(&sas, false, "123", "Emoji");
        assert_eq!(it.emoji, en.emoji);
        assert_eq!(it.emoji.concat(), sas.text);
        assert_eq!(it.digits, sas.digits);
        assert_eq!(it.names.to_vec(), sas.words(true));
        assert_eq!(en.names.to_vec(), sas.words(false));
        assert!(it.one_line().ends_with(&sas.digits));
        assert!(it.one_line().contains(it.names[3]));
    }

    #[test]
    fn plain_text_carries_code_and_note() {
        let sas = hoppy_core::Sas::from_value(0);
        let view = DialogView {
            text: "Pixel wants to send you 1 file".into(),
            note: "🔒 Encrypted".into(),
            code: Some(CodeView::new(&sas, false, "123", "Emoji")),
            buttons: vec![(1, "Decline".into()), (2, "Accept".into())],
            ..Default::default()
        };
        assert_eq!(view.default_id(), 1);
        assert_eq!(
            view.plain_text(),
            "Pixel wants to send you 1 file\n\n🔒 Encrypted\n\n🐘 🐘 🐘 🐘 (elephant, elephant, \
             elephant, elephant) · 0000 0000"
        );
        assert_eq!(DialogView::default().default_id(), 0);
    }

    #[test]
    fn live_flags_cross_threads() {
        let live = DialogLive::new();
        let other = live.clone();
        std::thread::spawn(move || {
            other.set_verified();
            other.close();
        })
        .join()
        .unwrap();
        assert!(live.verified() && live.closed());
    }
}
