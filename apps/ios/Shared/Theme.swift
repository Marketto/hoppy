// Purikura (light, default) and Night (dark) palettes: the same colors as the Android
// themes (apps/android/app/src/main/res/values/colors.xml). Every text/background pair is
// checked for WCAG AA by tools/contrast.py, which reads the `static let … = 0x……` lines
// below: keep that format (invariant 10).
import SwiftUI

enum HoppyColors {
    // Purikura
    static let purikuraBg = 0xFFE9F0
    static let purikuraCard = 0xFFF7FA
    static let purikuraText = 0x5B3A44
    static let purikuraMuted = 0x7A4C5A
    static let purikuraAccent = 0xA82B52
    static let purikuraButtonTop = 0xC2385F
    static let purikuraButtonBottom = 0xA82B52
    static let purikuraOnButton = 0xFFFFFF
    static let purikuraOutline = 0xF7B6CA
    // Night
    static let nightBg = 0x14151B
    static let nightCard = 0x1E2027
    static let nightText = 0xECECF1
    static let nightMuted = 0x9A9AA8
    static let nightAccent = 0x4FD1C5
    static let nightButtonTop = 0x4FD1C5
    static let nightButtonBottom = 0x4FD1C5
    static let nightOnButton = 0x00201D
    static let nightOutline = 0x34374A
}

extension Color {
    init(hex: Int) {
        self.init(.sRGB,
                  red: Double((hex >> 16) & 0xFF) / 255,
                  green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255,
                  opacity: 1)
    }
}

struct Palette {
    let bg, card, text, muted, accent, buttonTop, buttonBottom, onButton, outline: Color

    static let purikura = Palette(
        bg: Color(hex: HoppyColors.purikuraBg), card: Color(hex: HoppyColors.purikuraCard),
        text: Color(hex: HoppyColors.purikuraText), muted: Color(hex: HoppyColors.purikuraMuted),
        accent: Color(hex: HoppyColors.purikuraAccent),
        buttonTop: Color(hex: HoppyColors.purikuraButtonTop), buttonBottom: Color(hex: HoppyColors.purikuraButtonBottom),
        onButton: Color(hex: HoppyColors.purikuraOnButton), outline: Color(hex: HoppyColors.purikuraOutline))

    static let night = Palette(
        bg: Color(hex: HoppyColors.nightBg), card: Color(hex: HoppyColors.nightCard),
        text: Color(hex: HoppyColors.nightText), muted: Color(hex: HoppyColors.nightMuted),
        accent: Color(hex: HoppyColors.nightAccent),
        buttonTop: Color(hex: HoppyColors.nightButtonTop), buttonBottom: Color(hex: HoppyColors.nightButtonBottom),
        onButton: Color(hex: HoppyColors.nightOnButton), outline: Color(hex: HoppyColors.nightOutline))

    static func current(_ scheme: ColorScheme) -> Palette { scheme == .dark ? .night : .purikura }
}

/// The big rounded primary button ("Invia file").
struct HoppyButtonStyle: ButtonStyle {
    let palette: Palette
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.headline)
            .foregroundColor(palette.onButton)
            .frame(maxWidth: .infinity, minHeight: 52)
            .padding(.horizontal, 20)
            .background(
                LinearGradient(colors: [palette.buttonTop, palette.buttonBottom], startPoint: .top, endPoint: .bottom)
                    .clipShape(Capsule())
            )
            .opacity(configuration.isPressed ? 0.85 : 1)
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
    }
}

/// A device row: generic shape icon (never a vendor logo), name, platform, security.
struct PeerRow: View {
    let peer: HoppyPeer
    let palette: Palette

    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: Self.symbol(peer.platform))
                .font(.title2)
                .foregroundColor(palette.accent)
                .frame(width: 32)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(peer.name).font(.body.weight(.semibold)).foregroundColor(palette.text)
                Text(Self.platformName(peer.platform)).font(.subheadline).foregroundColor(palette.muted)
                // 🔒 / ✓ / ⚠ non cifrato (spec §7): text, so VoiceOver reads it too.
                if let security = SecurityText.row(peer) {
                    Text(security)
                        .font(.footnote.weight(.semibold))
                        .foregroundColor(peer.encrypted == false ? palette.accent : palette.muted)
                }
            }
            Spacer()
            Image(systemName: "chevron.right").foregroundColor(palette.muted).accessibilityHidden(true)
        }
        .padding(.vertical, 12)
        .padding(.horizontal, 16)
        .background(RoundedRectangle(cornerRadius: 16).fill(palette.card))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(palette.outline, lineWidth: 1))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine) // always inside a Button: it adds the trait
    }

    static func symbol(_ platform: String) -> String {
        switch platform {
        case "android", "ios": return "iphone"
        case "windows", "macos": return "laptopcomputer"
        case "linux": return "desktopcomputer"
        default: return "questionmark.circle"
        }
    }

    static func platformName(_ platform: String) -> String {
        switch platform {
        case "windows": return "Windows"
        case "android": return "Android"
        case "linux": return "Linux"
        case "macos": return "Mac"
        case "ios": return "iPhone / iPad"
        default: return platform
        }
    }
}

/// What the sender UI shows (app and Share extension).
enum SendStatus: Equatable {
    case idle
    case preparing
    case waiting(String)          // waiting for <name> to accept
    case fallback(String)         // direct connect failed, asking it to connect back
    case progress(String, Int)    // sending to <name> · n %
    case delivered(String)
    case declined(String)
    case failed(String)           // already localized
}

enum Format {
    static func size(_ bytes: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: bytes), countStyle: .file)
    }

    static func percent(_ done: UInt64, _ total: UInt64) -> Int {
        total == 0 ? 100 : Int(min(100, done * 100 / max(total, 1)))
    }
}

/// Localized text from the shared Localizable.strings / .stringsdict.
func L(_ key: String, _ args: CVarArg...) -> String {
    let format = NSLocalizedString(key, comment: "")
    return args.isEmpty ? format : String(format: format, locale: Locale.current, arguments: args)
}
