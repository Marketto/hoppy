// The acceptance screen (app: full-screen cover before the node starts; Share extension: the
// same view, compact, before its own node starts), the small «Termini · Privacy · Licenze»
// links and the page viewer. Texts come from the bundled Legal/legal.json (Legal.swift).
// No quit button: iOS apps must not offer one (Apple HIG); the app simply stays here.
// Extension-safe: no app-only UIKit API here, the caller passes `openExternal`.
import SwiftUI
import WebKit

struct LegalConsentView: View {
    let texts: LegalTexts?
    /// A version was accepted before: show `updated_title` / `updated_body`.
    let isUpdate: Bool
    /// Share extension: smaller mascot.
    var compact = false
    let palette: Palette
    let openExternal: (URL) -> Void
    let onAccept: () -> Void

    @State private var acceptTerms = false
    @State private var approveClauses = false
    @State private var readPrivacy = false

    private var allTicked: Bool { acceptTerms && approveClauses && readPrivacy }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                HStack {
                    Spacer()
                    Image("Mascot")
                        .resizable()
                        .scaledToFit()
                        .frame(width: compact ? 72 : 120, height: compact ? 72 : 120)
                        .accessibilityHidden(true)
                    Spacer()
                }
                if let texts = texts {
                    Text(texts.text(isUpdate ? "updated_title" : "title"))
                        .font((compact ? Font.title3 : Font.title2).weight(.bold))
                        .foregroundColor(palette.text)
                        .accessibilityAddTraits(.isHeader)
                    Text(texts.text(isUpdate ? "updated_body" : "body"))
                        .foregroundColor(palette.text)
                        .fixedSize(horizontal: false, vertical: true)
                    if isUpdate {
                        // The full disclaimer stays readable after an update too.
                        Text(texts.text("body"))
                            .font(.subheadline)
                            .foregroundColor(palette.muted)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    LegalLinks(texts: texts, palette: palette, openExternal: openExternal)
                    LegalCheckbox(label: texts.text("accept_terms"), isOn: $acceptTerms, palette: palette)
                    LegalCheckbox(label: texts.text("approve_clauses"), isOn: $approveClauses, palette: palette)
                    LegalCheckbox(label: texts.text("read_privacy"), isOn: $readPrivacy, palette: palette)
                    Button(texts.text("continue"), action: onAccept)
                        .buttonStyle(HoppyButtonStyle(palette: palette))
                        .disabled(!allTicked)
                        .opacity(allTicked ? 1 : 0.5)
                        .padding(.top, 6)
                } else {
                    // Broken bundle (ios_check.py guards against it): fail closed.
                    Text(L("legal_missing")).foregroundColor(palette.text)
                }
            }
            .padding(compact ? 20 : 24)
            .frame(maxWidth: 560)
            .frame(maxWidth: .infinity)
        }
        .background(palette.bg.ignoresSafeArea())
    }
}

/// A square checkbox with a wrapping label; VoiceOver reads it as one button with a value.
struct LegalCheckbox: View {
    let label: String
    @Binding var isOn: Bool
    let palette: Palette

    var body: some View {
        Button { isOn.toggle() } label: {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: isOn ? "checkmark.square.fill" : "square")
                    .font(.title2)
                    .foregroundColor(palette.accent)
                Text(label)
                    .foregroundColor(palette.text)
                    .multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
        .accessibilityValue(isOn ? L("legal_checked") : L("legal_unchecked"))
        .accessibilityAddTraits(.isButton)
    }
}

/// «Termini d'uso · Privacy · Licenze»: small links opening the bundled pages in a sheet.
struct LegalLinks: View {
    let texts: LegalTexts
    let palette: Palette
    let openExternal: (URL) -> Void

    struct Page: Identifiable {
        let id: String
        let title: String
        let url: URL
    }

    @State private var page: Page?

    var body: some View {
        HStack(spacing: 6) {
            link("terms")
            Text("·").foregroundColor(palette.muted).accessibilityHidden(true)
            link("privacy")
            Text("·").foregroundColor(palette.muted).accessibilityHidden(true)
            link("licenses")
        }
        .font(.footnote.weight(.semibold))
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(texts.text("legal"))
        .sheet(item: $page) { p in
            LegalPageSheet(title: p.title, url: p.url, palette: palette, openExternal: openExternal)
        }
    }

    private func link(_ name: String) -> some View {
        Button {
            if let url = texts.page(name) { page = Page(id: name, title: texts.text(name), url: url) }
        } label: {
            Text(texts.text(name)).underline().foregroundColor(palette.accent)
        }
        .buttonStyle(.plain)
        .frame(minHeight: 44)
    }
}

struct LegalPageSheet: View {
    let title: String
    let url: URL
    let palette: Palette
    let openExternal: (URL) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationView {
            LegalWebView(url: url, openExternal: openExternal)
                .ignoresSafeArea(edges: .bottom)
                .navigationTitle(title)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) { Button(L("done")) { dismiss() } }
                }
        }
        .navigationViewStyle(.stack)
        .tint(palette.accent)
    }
}

/// Bundled pages only: file URLs inside the Legal folder, JavaScript off, nothing stored;
/// `https://` links go to the system browser (`openExternal`), everything else is refused.
struct LegalWebView: UIViewRepresentable {
    let url: URL
    let openExternal: (URL) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(root: url.deletingLastPathComponent(), openExternal: openExternal)
    }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        config.websiteDataStore = .nonPersistent()
        config.dataDetectorTypes = []
        let web = WKWebView(frame: .zero, configuration: config)
        web.navigationDelegate = context.coordinator
        web.allowsLinkPreview = false
        web.loadFileURL(url, allowingReadAccessTo: context.coordinator.root)
        return web
    }

    func updateUIView(_ web: WKWebView, context: Context) {
        context.coordinator.openExternal = openExternal
    }

    final class Coordinator: NSObject, WKNavigationDelegate {
        let root: URL
        var openExternal: (URL) -> Void
        private let rootPath: String

        init(root: URL, openExternal: @escaping (URL) -> Void) {
            self.root = root
            self.openExternal = openExternal
            rootPath = root.standardizedFileURL.resolvingSymlinksInPath().path
        }

        func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                     decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            guard let target = action.request.url else { return decisionHandler(.cancel) }
            if target.isFileURL {
                let path = target.standardizedFileURL.resolvingSymlinksInPath().path
                decisionHandler(path.hasPrefix(rootPath + "/") ? .allow : .cancel)
                return
            }
            if target.scheme?.lowercased() == "https", action.navigationType == .linkActivated {
                openExternal(target)
            }
            decisionHandler(.cancel)
        }
    }
}
