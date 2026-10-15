// Protocol v1 verification UI shared by the app and the Share extension (spec §7, ADR 0010):
// the security line, the 4-emoji / 8-digit code with "Coincide", "Verifica con QR",
// "Mostra il mio QR", "Più tardi", and a dismissable banner. Same UX as the other platforms.
// No protocol logic: the code, the trust state and the checks come from the Rust core.
import SwiftUI

enum SecurityText {
    /// One line for the session on screen.
    static func headline(_ s: HoppySecurity) -> String {
        if !s.encrypted { return L("sec_plain") }
        if s.isVerified { return L("sec_verified") }
        if s.trust == "key_changed" { return L("sec_key_changed") }
        return L(s.trust == "known" ? "sec_known" : "sec_new")
    }

    /// Short label for a device row, nil while the core has not said.
    static func row(_ peer: HoppyPeer) -> String? {
        guard let encrypted = peer.encrypted else { return nil }
        if !encrypted { return L("row_plain") }
        return L(peer.trust == "verified" ? "row_verified" : "row_encrypted")
    }
}

/// The code: 4 large emoji (VoiceOver reads their names) or, with "123", 8 digits.
struct SasView: View {
    let sas: HoppySas
    let palette: Palette
    @State var digits = false
    @ScaledMetric(relativeTo: .largeTitle) var emojiSize: CGFloat = 44

    var body: some View {
        HStack(spacing: 12) {
            Group {
                if digits {
                    Text(sas.digits)
                        .font(.system(size: emojiSize * 0.75, weight: .bold, design: .monospaced))
                        .foregroundColor(palette.text)
                        .minimumScaleFactor(0.5)
                        .lineLimit(1)
                } else {
                    HStack(spacing: 8) {
                        ForEach(Array(sas.symbols.enumerated()), id: \.offset) { item in
                            Text(item.element).font(.system(size: emojiSize))
                        }
                    }
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(digits
                ? L("sec_code_digits_a11y", sas.digits.filter(\.isNumber).map(String.init).joined(separator: " "))
                : L("sec_code_emoji_a11y", sas.names.joined(separator: ", ")))
            Spacer(minLength: 0)
            Button(digits ? L("sec_emoji_short") : "123") { digits.toggle() }
                .buttonStyle(.bordered)
                .accessibilityLabel(L(digits ? "sec_show_emoji" : "sec_show_digits"))
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 14).fill(palette.bg))
    }
}

/// Security of one session (an incoming offer or the send in progress). The buttons are
/// shown only when the matching action is given (the Share extension has no camera and no
/// receiver, so no QR there).
struct SecurityPanel: View {
    let security: HoppySecurity
    let peerName: String
    let palette: Palette
    /// "Più tardi" pressed: only the security line stays.
    var collapsed = false
    var note: String?
    var onMatch: (() -> Void)?
    var onScanQR: (() -> Void)?
    var onShowQR: (() -> Void)?
    var onLater: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(SecurityText.headline(security))
                .font(.subheadline.weight(.semibold))
                .foregroundColor(security.encrypted ? palette.text : palette.accent)
                .fixedSize(horizontal: false, vertical: true)
            if security.nameMatchesKnown == true {
                Text(L("sec_name_clash"))
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(palette.accent)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if security.needsCheck, !collapsed, let sas = security.sas {
                Text(L("sec_compare", peerName))
                    .font(.subheadline)
                    .foregroundColor(palette.muted)
                    .fixedSize(horizontal: false, vertical: true)
                SasView(sas: sas, palette: palette)
                VStack(spacing: 8) {
                    if let onMatch = onMatch {
                        Button(action: onMatch) {
                            Text(L("sec_match")).frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.borderedProminent)
                        .accessibilityHint(L("sec_match_hint"))
                    }
                    if let onScanQR = onScanQR {
                        Button(action: onScanQR) {
                            Label(L("sec_scan_qr"), systemImage: "qrcode.viewfinder").frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.bordered)
                    }
                    if let onShowQR = onShowQR {
                        Button(action: onShowQR) {
                            Label(L("sec_show_my_qr"), systemImage: "qrcode").frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.bordered)
                    }
                    if let onLater = onLater {
                        Button(action: onLater) {
                            Text(L("sec_later")).frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.bordered)
                    }
                }
                .font(.subheadline.weight(.semibold))
            }
            if let note = note {
                Text(note)
                    .font(.subheadline)
                    .foregroundColor(palette.text)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityAddTraits(.updatesFrequently)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 18).fill(palette.card))
        .overlay(RoundedRectangle(cornerRadius: 18).stroke(security.encrypted ? palette.outline : palette.accent, lineWidth: 1))
    }
}

/// A one-line notice with a close button (security warnings, updates).
struct BannerView<Extra: View>: View {
    let text: String
    let palette: Palette
    let onClose: () -> Void
    @ViewBuilder var extra: () -> Extra

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            VStack(alignment: .leading, spacing: 6) {
                Text(text)
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(palette.text)
                    .fixedSize(horizontal: false, vertical: true)
                extra()
            }
            Spacer(minLength: 0)
            Button(action: onClose) {
                Image(systemName: "xmark.circle.fill").foregroundColor(palette.muted)
            }
            .accessibilityLabel(L("dismiss"))
        }
        .padding(14)
        .background(RoundedRectangle(cornerRadius: 16).fill(palette.card))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(palette.accent, lineWidth: 2))
        .accessibilityElement(children: .contain)
    }
}

extension BannerView where Extra == EmptyView {
    init(text: String, palette: Palette, onClose: @escaping () -> Void) {
        self.init(text: text, palette: palette, onClose: onClose) { EmptyView() }
    }
}

/// "Attenzione: la chiave di sicurezza di X è cambiata" (spec §7.5): blocks until forgotten.
struct KeyChangedAlert: Identifiable, Equatable {
    let peerId: String
    let name: String
    var id: String { peerId }
}
