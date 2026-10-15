// The one screen: mascot on top, "Pronto a ricevere", nearby devices, "Invia file"; the
// incoming-offer sheet with the verification code (spec §7), KeyChanged alert.
import PhotosUI
import QuickLook
import SwiftUI
import UIKit
import UniformTypeIdentifiers

struct ContentView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.colorScheme) private var scheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var chooseSource = false
    @State private var showFiles = false
    @State private var showPhotos = false
    /// Device tapped in the home list: the picked files go straight to it.
    @State private var target: HoppyPeer? = nil
    @State private var preview: [URL] = []
    @State private var shareItems: [URL] = []
    @State private var editing: TextPrompt? = nil

    private var palette: Palette { Palette.current(scheme) }

    var body: some View {
        NavigationView {
            ScrollView {
                VStack(spacing: 18) {
                    if let text = model.banner {
                        BannerView(text: text, palette: palette, onClose: { model.banner = nil })
                    }
                    if let version = model.update {
                        BannerView(text: L("update_banner", version), palette: palette, onClose: { model.dismissUpdate() }) {
                            Link(L("update_open"), destination: UpdateCheck.portal)
                                .font(.subheadline.weight(.semibold))
                                .foregroundColor(palette.accent)
                        }
                    }
                    mascot
                    receiveCard
                    if !model.lastReceived.isEmpty { receivedCard }
                    sendSection
                    nearbySection
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 32)
                .frame(maxWidth: 560)
                .frame(maxWidth: .infinity)
            }
            .background(palette.bg.ignoresSafeArea())
            .navigationTitle("Hoppy")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .navigationBarTrailing) { menu } }
            .refreshable { model.refresh() }
        }
        .navigationViewStyle(.stack)
        .tint(palette.accent)
        .confirmationDialog(L("send_files"), isPresented: $chooseSource, titleVisibility: .visible) {
            Button(L("pick_files")) { showFiles = true }
            Button(L("pick_photos")) { showPhotos = true }
            Button(L("cancel"), role: .cancel) { target = nil }
        }
        .sheet(isPresented: $showFiles) {
            DocumentPicker { urls in picked(urls) }.ignoresSafeArea()
        }
        .sheet(isPresented: $showPhotos) {
            PhotoPicker { providers in
                guard !providers.isEmpty else { target = nil; return }
                model.setPreparing()
                Outbox.stage(providers) { urls in picked(urls) }
            }
            .ignoresSafeArea()
        }
        .sheet(isPresented: Binding(get: { !model.pendingFiles.isEmpty }, set: { if !$0 { model.cancelPending() } })) {
            DeviceSheet(files: model.pendingFiles).environmentObject(model)
        }
        .sheet(isPresented: Binding(get: { !preview.isEmpty }, set: { if !$0 { preview = [] } })) {
            QuickLookView(urls: preview).ignoresSafeArea()
        }
        .sheet(isPresented: Binding(get: { !shareItems.isEmpty }, set: { if !$0 { shareItems = [] } })) {
            ActivityView(items: shareItems).ignoresSafeArea()
        }
        .sheet(item: $editing) { prompt in TextPromptSheet(prompt: prompt, palette: palette).environmentObject(model) }
        // Swiping the sheet away declines (model.decide ignores an offer already answered):
        // never auto-accept.
        .sheet(item: Binding(get: { model.offer }, set: { if $0 == nil { model.decide(accept: false) } })) { offer in
            OfferSheet(offer: offer).environmentObject(model)
        }
        .alert(L("key_changed_title", model.keyChanged?.name ?? ""),
               isPresented: Binding(get: { model.keyChanged != nil && model.offer == nil }, set: { if !$0 { model.keyChanged = nil } }),
               presenting: model.keyChanged) { alert in
            Button(L("forget_device"), role: .destructive) { model.forget(alert.peerId) }
            Button(L("ok"), role: .cancel) { model.keyChanged = nil }
        } message: { alert in
            Text(L("key_changed_text", alert.name))
        }
        .alert(L("receive_error_title"), isPresented: Binding(get: { model.receiveError != nil }, set: { if !$0 { model.receiveError = nil } })) {
            Button(L("ok"), role: .cancel) { model.receiveError = nil }
        } message: {
            Text(model.receiveError ?? "")
        }
    }

    // MARK: parts

    private var mascotName: String {
        if case .delivered = model.sendStatus { return "MascotHappy" }
        return model.lastReceived.isEmpty ? "Mascot" : "MascotHappy"
    }

    private var mascot: some View {
        Image(mascotName)
            .resizable()
            .scaledToFit()
            .frame(width: 150, height: 150)
            .padding(.top, 8)
            .accessibilityLabel(L("mascot"))
            .animation(reduceMotion ? nil : .spring(response: 0.4, dampingFraction: 0.5), value: mascotName)
    }

    private var receiveCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            Toggle(isOn: Binding(get: { model.receiving }, set: { model.setReceiving($0) })) {
                Text(L("receive_toggle")).font(.headline).foregroundColor(palette.text)
            }
            .toggleStyle(SwitchToggleStyle(tint: palette.accent))
            Text(model.receiving ? L("receive_on_hint") : L("receive_off_hint"))
                .font(.subheadline)
                .foregroundColor(palette.muted)
                .fixedSize(horizontal: false, vertical: true)
            Button {
                editing = TextPrompt(kind: .rename, value: model.deviceName)
            } label: {
                Label(L("visible_as", model.deviceName), systemImage: "pencil")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(palette.accent)
            }
            .accessibilityHint(L("rename"))
            if let percent = model.receivePercent {
                ProgressView(value: Double(percent), total: 100) {
                    Text(L("receiving_progress", percent)).font(.subheadline).foregroundColor(palette.text)
                }
            }
            if let error = model.startError {
                Text(L("start_error", error)).font(.subheadline).foregroundColor(palette.accent)
            }
        }
        .padding(16)
        .background(RoundedRectangle(cornerRadius: 20).fill(palette.card))
        .overlay(RoundedRectangle(cornerRadius: 20).stroke(palette.outline, lineWidth: 1))
    }

    private var receivedCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(String.localizedStringWithFormat(NSLocalizedString("received_title", comment: ""),
                                                  model.lastReceived.count, model.lastReceivedFrom))
                .font(.headline).foregroundColor(palette.text)
            Text(model.lastReceived.prefix(3).map { $0.lastPathComponent }.joined(separator: ", ")
                 + (model.lastReceived.count > 3 ? "…" : ""))
                .font(.subheadline).foregroundColor(palette.muted).lineLimit(2)
            Text(L("received_where")).font(.footnote).foregroundColor(palette.muted)
            HStack(spacing: 10) {
                Button(L("open")) { preview = model.lastReceived }
                Button(L("share")) { shareItems = model.lastReceived }
                Button(L("show_in_files")) { openReceivedFolder() }
                Spacer()
                Button {
                    model.clearReceived()
                } label: {
                    Image(systemName: "xmark.circle.fill").foregroundColor(palette.muted)
                }
                .accessibilityLabel(L("dismiss"))
            }
            .buttonStyle(.bordered)
            .font(.subheadline.weight(.semibold))
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 20).fill(palette.card))
        .overlay(RoundedRectangle(cornerRadius: 20).stroke(palette.accent, lineWidth: 2))
    }

    private var sendSection: some View {
        VStack(spacing: 10) {
            Button {
                target = nil
                chooseSource = true
            } label: {
                Label(L("send_files"), systemImage: "paperplane.fill")
            }
            .buttonStyle(HoppyButtonStyle(palette: palette))
            if let text = statusText {
                Text(text)
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(palette.text)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityAddTraits(.updatesFrequently)
            }
            if case let .progress(_, percent) = model.sendStatus {
                ProgressView(value: Double(percent), total: 100).accessibilityHidden(true)
            }
            if let security = model.sendSecurity {
                SecurityPanel(security: security, peerName: sendPeerName, palette: palette,
                              collapsed: model.sendCheckHidden,
                              note: model.noteFor == .send ? model.securityNote : nil,
                              onMatch: { model.confirmMatch(.send) },
                              onLater: { model.later(.send) })
            } else if model.noteFor == .send, let note = model.securityNote {
                Text(note).font(.subheadline).foregroundColor(palette.text)
            }
        }
    }

    private var sendPeerName: String {
        switch model.sendStatus {
        case let .waiting(n), let .fallback(n), let .progress(n, _), let .delivered(n), let .declined(n): return n
        default: return model.sendSecurity?.peer?.name ?? "?"
        }
    }

    private var nearbySection: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(L("nearby")).font(.headline).foregroundColor(palette.text).accessibilityAddTraits(.isHeader)
                Spacer()
                Button {
                    model.refresh()
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .accessibilityLabel(L("search_again"))
            }
            if model.peers.isEmpty {
                HStack(spacing: 10) {
                    ProgressView()
                    Text(L("no_peers")).font(.subheadline).foregroundColor(palette.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Text(L("local_network_hint")).font(.footnote).foregroundColor(palette.muted)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                ForEach(model.peers) { peer in
                    Button {
                        target = peer
                        chooseSource = true
                    } label: {
                        PeerRow(peer: peer, palette: palette)
                    }
                    .buttonStyle(.plain)
                    .accessibilityHint(L("send_to_hint"))
                    .contextMenu {
                        if peer.trust == "known" || peer.trust == "verified" {
                            Button(role: .destructive) { model.forget(peer.id) } label: {
                                Label(L("forget_device"), systemImage: "trash")
                            }
                        }
                    }
                }
            }
        }
        .padding(.top, 6)
    }

    private var menu: some View {
        Menu {
            Button { editing = TextPrompt(kind: .rename, value: model.deviceName) } label: {
                Label(L("rename"), systemImage: "pencil")
            }
            Button { editing = TextPrompt(kind: .address, value: "") } label: {
                Label(L("add_address"), systemImage: "plus.circle")
            }
            Button { openReceivedFolder() } label: {
                Label(L("open_received"), systemImage: "folder")
            }
            Toggle(L("sound"), isOn: $model.soundOn)
        } label: {
            Image(systemName: "ellipsis.circle").imageScale(.large)
        }
        .accessibilityLabel(L("menu"))
    }

    private var statusText: String? {
        switch model.sendStatus {
        case .idle: return nil
        case .preparing: return L("preparing")
        case let .waiting(name): return L("waiting_accept", name)
        case let .fallback(name): return L("send_trying_callback", name)
        case let .progress(name, percent): return L("sending_progress", name, percent)
        case let .delivered(name): return L("delivered", name)
        case let .declined(name): return L("declined", name)
        case let .failed(message): return message
        }
    }

    // MARK: actions

    private func picked(_ urls: [URL]) {
        defer { target = nil }
        guard !urls.isEmpty else {
            model.clearSendStatus()
            return
        }
        if let peer = target, model.peers.contains(where: { $0.id == peer.id }) {
            model.send(urls, to: peer)
        } else {
            model.clearSendStatus()
            // Let the picker sheet finish dismissing before the "Invia a" sheet appears.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { model.offerToSend(urls) }
        }
    }

    /// Files app at Hoppy's folder ("On My iPad › Hoppy").
    private func openReceivedFolder() {
        let path = model.receiveDir.path
        if let url = URL(string: "shareddocuments://" + (path.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? path)) {
            UIApplication.shared.open(url)
        }
    }

    static func offerText(_ offer: HoppyOffer) -> String {
        let platform = PeerRow.platformName(offer.fromPlatform)
        var text = String.localizedStringWithFormat(NSLocalizedString("incoming_message", comment: ""),
                                                    offer.fromName, platform, offer.count, Format.size(offer.total))
        let names = offer.files.prefix(5).map(\.name)
        if !names.isEmpty {
            text += "\n\n" + names.joined(separator: "\n") + (offer.count > names.count ? "\n…" : "")
        }
        return text
    }
}

// MARK: - incoming offer

/// "File in arrivo": sender, files, security (emoji code / ✓ / non cifrato), Rifiuta first.
struct OfferSheet: View {
    let offer: HoppyOffer
    @EnvironmentObject var model: AppModel
    @Environment(\.colorScheme) var scheme

    var body: some View {
        let palette = Palette.current(scheme)
        let current = model.offer ?? offer
        NavigationView {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    Text(ContentView.offerText(current))
                        .font(.body)
                        .foregroundColor(palette.text)
                        .fixedSize(horizontal: false, vertical: true)
                    SecurityPanel(security: current.security, peerName: current.fromName, palette: palette,
                                  collapsed: model.offerCheckHidden,
                                  note: model.noteFor == .offer ? model.securityNote : nil,
                                  onMatch: { model.confirmMatch(.offer) },
                                  onLater: { model.later(.offer) })
                    // Decline first: the safe choice; no answer in 110 s also declines.
                    VStack(spacing: 10) {
                        Button { model.decide(accept: false) } label: {
                            Text(L("decline")).frame(maxWidth: .infinity)
                        }
                        .buttonStyle(HoppyButtonStyle(palette: palette))
                        .keyboardShortcut(.cancelAction)
                        Button { model.decide(accept: true) } label: {
                            Text(L("accept")).font(.headline).frame(maxWidth: .infinity, minHeight: 44)
                        }
                        .buttonStyle(.bordered)
                    }
                    .padding(.top, 4)
                }
                .padding(20)
                .frame(maxWidth: 560)
                .frame(maxWidth: .infinity)
            }
            .background(palette.bg.ignoresSafeArea())
            .navigationTitle(L("incoming_title"))
            .navigationBarTitleDisplayMode(.inline)
        }
        .navigationViewStyle(.stack)
        .tint(palette.accent)
    }
}

// MARK: - "Invia a" sheet

struct DeviceSheet: View {
    let files: [URL]
    @EnvironmentObject var model: AppModel
    @Environment(\.colorScheme) var scheme

    var body: some View {
        let palette = Palette.current(scheme)
        NavigationView {
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    Text(String.localizedStringWithFormat(NSLocalizedString("files_summary", comment: ""),
                                                          files.count, Format.size(totalSize)))
                        .font(.subheadline).foregroundColor(palette.muted)
                    if model.peers.isEmpty {
                        HStack(spacing: 10) {
                            ProgressView()
                            Text(L("searching")).foregroundColor(palette.text)
                        }
                        .padding(.vertical, 8)
                        Text(L("no_peers")).font(.subheadline).foregroundColor(palette.muted)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    ForEach(model.peers) { peer in
                        Button {
                            model.send(files, to: peer)
                        } label: {
                            PeerRow(peer: peer, palette: palette)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(20)
            }
            .background(palette.bg.ignoresSafeArea())
            .navigationTitle(L("send_to"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("cancel")) { model.cancelPending() }
                }
            }
            .onAppear { model.refresh() }
        }
        .navigationViewStyle(.stack)
    }

    private var totalSize: UInt64 {
        files.reduce(0) { sum, url in
            let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
            return sum + UInt64(max(size, 0))
        }
    }
}

// MARK: - text prompt (rename, add address): iOS 15 alerts have no text fields

struct TextPrompt: Identifiable {
    enum Kind { case rename, address }
    let kind: Kind
    var value: String
    var id: String { kind == .rename ? "rename" : "address" }
}

struct TextPromptSheet: View {
    @State var prompt: TextPrompt
    let palette: Palette
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) var dismiss
    @State var invalid = false

    var body: some View {
        NavigationView {
            Form {
                Section {
                    TextField(prompt.kind == .rename ? L("rename") : "192.168.1.20", text: $prompt.value)
                        .keyboardType(prompt.kind == .rename ? .default : .numbersAndPunctuation)
                        .disableAutocorrection(true)
                        .textInputAutocapitalization(prompt.kind == .rename ? .words : .never)
                        .submitLabel(.done)
                        .onSubmit(save)
                } footer: {
                    Text(prompt.kind == .rename ? L("rename_hint") : (invalid ? L("invalid_address") : L("add_address_hint")))
                }
            }
            .navigationTitle(prompt.kind == .rename ? L("rename") : L("add_address"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(L("cancel")) { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button(L("save"), action: save) }
            }
        }
        .navigationViewStyle(.stack)
    }

    private func save() {
        switch prompt.kind {
        case .rename:
            model.rename(prompt.value)
            dismiss()
        case .address:
            if model.addAddress(prompt.value) { dismiss() } else { invalid = true }
        }
    }
}

// MARK: - UIKit bridges

struct DocumentPicker: UIViewControllerRepresentable {
    let onPick: ([URL]) -> Void

    func makeUIViewController(context: Context) -> UIDocumentPickerViewController {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.item], asCopy: true)
        picker.allowsMultipleSelection = true
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ controller: UIDocumentPickerViewController, context: Context) {}
    func makeCoordinator() -> Coordinator { Coordinator(onPick) }

    final class Coordinator: NSObject, UIDocumentPickerDelegate {
        let onPick: ([URL]) -> Void
        init(_ onPick: @escaping ([URL]) -> Void) { self.onPick = onPick }

        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
            // asCopy: these are our own temporary copies; move them into the outbox.
            let staged = urls.compactMap { url -> URL? in
                defer { try? FileManager.default.removeItem(at: url) }
                return try? Outbox.stage(url)
            }
            onPick(staged)
        }

        func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { onPick([]) }
    }
}

struct PhotoPicker: UIViewControllerRepresentable {
    let onPick: ([NSItemProvider]) -> Void

    func makeUIViewController(context: Context) -> PHPickerViewController {
        var config = PHPickerConfiguration()
        config.selectionLimit = 0
        config.preferredAssetRepresentationMode = .current // originals (HEIC/HEVC), no transcoding
        let picker = PHPickerViewController(configuration: config)
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ controller: PHPickerViewController, context: Context) {}
    func makeCoordinator() -> Coordinator { Coordinator(onPick) }

    final class Coordinator: NSObject, PHPickerViewControllerDelegate {
        let onPick: ([NSItemProvider]) -> Void
        init(_ onPick: @escaping ([NSItemProvider]) -> Void) { self.onPick = onPick }

        func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
            picker.dismiss(animated: true)
            onPick(results.map(\.itemProvider))
        }
    }
}

struct QuickLookView: UIViewControllerRepresentable {
    let urls: [URL]

    func makeUIViewController(context: Context) -> UINavigationController {
        let ql = QLPreviewController()
        ql.dataSource = context.coordinator
        return UINavigationController(rootViewController: ql)
    }

    func updateUIViewController(_ controller: UINavigationController, context: Context) {}
    func makeCoordinator() -> Coordinator { Coordinator(urls) }

    final class Coordinator: NSObject, QLPreviewControllerDataSource {
        let urls: [URL]
        init(_ urls: [URL]) { self.urls = urls }
        func numberOfPreviewItems(in controller: QLPreviewController) -> Int { urls.count }
        func previewController(_ controller: QLPreviewController, previewItemAt index: Int) -> QLPreviewItem {
            urls[index] as NSURL
        }
    }
}

struct ActivityView: UIViewControllerRepresentable {
    let items: [URL]

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
