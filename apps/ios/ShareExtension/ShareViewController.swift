// "Condividi → Hoppy": a self-contained Share extension. It stages the shared files into its
// own temporary folder (APFS clones, nothing is loaded into memory: extensions are killed
// above ~120 MB), runs its own send-only Rust node (ephemeral UDP port, unicast sweep), lets
// the user tap a device and streams the files. No App Group, no URL-scheme hand-off to the
// app (ADR 0009). Protocol v1 (ADR 0010): it is its own device to others ("iPad (Condividi)"),
// with a stable id, key and trust store in its own container, and shows the verification
// 4-emoji code ("Coincide" / "Più tardi"), the same as in the app.
import SwiftUI
import UIKit

final class ShareViewController: UIViewController {
    private let model = ShareModel()

    override func viewDidLoad() {
        super.viewDidLoad()
        model.close = { [weak self] done in
            guard let context = self?.extensionContext else { return }
            if done {
                context.completeRequest(returningItems: nil)
            } else {
                context.cancelRequest(withError: NSError(domain: "app.hoppy.share", code: 1))
            }
        }
        let host = UIHostingController(rootView: ShareView().environmentObject(model))
        addChild(host)
        host.view.frame = view.bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(host.view)
        host.didMove(toParent: self)

        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? [])
            .flatMap { $0.attachments ?? [] }
        model.load(providers)
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        model.shutdown()
    }
}

final class ShareModel: ObservableObject {
    @Published private(set) var files: [URL] = []
    @Published private(set) var loading = true
    @Published private(set) var peers: [HoppyPeer] = []
    @Published private(set) var status: SendStatus = .idle
    @Published private(set) var security: HoppySecurity?
    @Published var checkHidden = false
    @Published private(set) var note: String?
    @Published var keyChanged: KeyChangedAlert?
    @Published var banner: String?
    var close: (Bool) -> Void = { _ in }

    private var node: HoppyNode?
    private var transfer: (id: UInt64, peerId: String, peer: String)?

    var sending: Bool {
        switch status {
        case .waiting, .fallback, .progress: return true
        default: return false
        }
    }

    func load(_ providers: [NSItemProvider]) {
        Outbox.clear() // leftovers of an extension killed mid-send
        startNode()
        Outbox.stage(providers) { [weak self] urls in
            self?.files = urls
            self?.loading = false
        }
    }

    private func startNode() {
        // Own container: own id, key and trust store; named "<model> (Condividi)".
        let seeds = Identity.seeds
        node = try? Identity.startNode(name: Identity.shareExtensionName, configure: { config in
            config.udp_port = 0 // never take 47470 from the app
            config.receive = false
            config.seeds = seeds
        }, onEvent: { [weak self] event in
            DispatchQueue.main.async { self?.handle(event) }
        })
        if node == nil { status = .failed(L("send_failed", L("not_running"))) }
        if let warning = node?.info?.trust_warning { banner = L("trust_problem", warning) }
    }

    func send(to peer: HoppyPeer) {
        guard let node = node, !files.isEmpty, !sending else { return }
        let id = node.send(files, to: peer.id)
        guard id != 0 else {
            status = .failed(L("nothing_to_send"))
            return
        }
        transfer = (id, peer.id, peer.name)
        security = nil
        checkHidden = false
        note = nil
        status = .waiting(peer.name)
    }

    /// "Coincide": both screens show the same code.
    func confirmMatch() {
        guard let node = node, let peer = security?.peer else { return }
        if node.verify(peer) {
            security?.trust = "verified"
            note = L("sec_verified_now", peer.name)
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        } else {
            note = L("sec_verify_failed", peer.name)
        }
    }

    func forget(_ peerId: String) {
        node?.forget(peerId)
        keyChanged = nil
    }

    func refresh() { node?.refresh() }

    func cancel() {
        shutdown()
        close(false)
    }

    func shutdown() {
        node?.stop()
        node = nil
        Outbox.clear()
    }

    private func handle(_ event: HoppyEvent) {
        guard node != nil else { return }
        switch event {
        case let .peers(list):
            peers = list
            Identity.remember(list)
        case let .sendProgress(id, done, total):
            guard let t = transfer, t.id == id else { return }
            status = .progress(t.peer, Format.percent(done, total))
        case let .sendFallback(id):
            guard let t = transfer, t.id == id else { return }
            status = .fallback(t.peer)
        case let .sendDone(id, outcome, message):
            guard let t = transfer, t.id == id else { return }
            transfer = nil
            switch outcome {
            case .delivered:
                status = .delivered(t.peer)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                // A code still open for comparison keeps the sheet up ("Fine" closes it).
                if !((security?.needsCheck ?? false) && !checkHidden) {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { [weak self] in
                        self?.shutdown()
                        self?.close(true)
                    }
                }
            case .declined: status = .declined(t.peer)
            case .unreachable: status = .failed(L("send_unreachable", t.peer))
            case .keyChanged:
                status = .failed(L("send_key_changed", t.peer))
                security = nil
                keyChanged = KeyChangedAlert(peerId: t.peerId, name: t.peer)
            case .unencrypted: status = .failed(L("send_unencrypted", t.peer))
            case .error: status = .failed(L("send_failed", message ?? "?"))
            }
        case let .sendSession(id, sec):
            guard let t = transfer, t.id == id else { return }
            security = sec
        case let .trustNotSaved(error):
            banner = L("trust_not_saved", error)
        default:
            break // send-only: no offers, no receiving
        }
    }
}

struct ShareView: View {
    @EnvironmentObject private var model: ShareModel
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let palette = Palette.current(scheme)
        NavigationView {
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if let text = model.banner {
                        BannerView(text: text, palette: palette, onClose: { model.banner = nil })
                    }
                    HStack {
                        Spacer()
                        Image(isDelivered ? "MascotHappy" : "Mascot")
                            .resizable().scaledToFit().frame(width: 96, height: 96)
                            .accessibilityLabel(L("mascot"))
                        Spacer()
                    }
                    if model.loading {
                        HStack(spacing: 10) { ProgressView(); Text(L("preparing")).foregroundColor(palette.text) }
                    } else if model.files.isEmpty {
                        Text(L("nothing_to_send")).foregroundColor(palette.text)
                    } else {
                        Text(String.localizedStringWithFormat(NSLocalizedString("files_summary", comment: ""),
                                                              model.files.count, Format.size(totalSize)))
                            .font(.subheadline).foregroundColor(palette.muted)
                    }
                    if let text = statusText {
                        Text(text).font(.subheadline.weight(.semibold)).foregroundColor(palette.text)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if case let .progress(_, percent) = model.status {
                        ProgressView(value: Double(percent), total: 100).accessibilityHidden(true)
                    }
                    if let security = model.security {
                        SecurityPanel(security: security, peerName: peerName(security), palette: palette,
                                      collapsed: model.checkHidden, note: model.note,
                                      onMatch: { model.confirmMatch() },
                                      onLater: { model.checkHidden = true })
                    }
                    Text(L("send_to")).font(.headline).foregroundColor(palette.text).padding(.top, 6)
                        .accessibilityAddTraits(.isHeader)
                    if model.peers.isEmpty {
                        HStack(spacing: 10) {
                            ProgressView()
                            Text(L("searching")).foregroundColor(palette.text)
                        }
                        Text(L("no_peers")).font(.subheadline).foregroundColor(palette.muted)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    ForEach(model.peers) { peer in
                        Button { model.send(to: peer) } label: { PeerRow(peer: peer, palette: palette) }
                            .buttonStyle(.plain)
                            .disabled(model.loading || model.files.isEmpty || model.sending)
                    }
                }
                .padding(20)
            }
            .background(palette.bg.ignoresSafeArea())
            .navigationTitle(L("share_title"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(isDelivered ? L("done") : L("cancel")) {
                        if isDelivered {
                            model.shutdown()
                            model.close(true)
                        } else {
                            model.cancel()
                        }
                    }
                }
                ToolbarItem(placement: .primaryAction) {
                    Button { model.refresh() } label: { Image(systemName: "arrow.clockwise") }
                        .accessibilityLabel(L("search_again"))
                }
            }
        }
        .navigationViewStyle(.stack)
        .tint(palette.accent)
        .alert(L("key_changed_title", model.keyChanged?.name ?? ""),
               isPresented: Binding(get: { model.keyChanged != nil }, set: { if !$0 { model.keyChanged = nil } }),
               presenting: model.keyChanged) { alert in
            Button(L("forget_device"), role: .destructive) { model.forget(alert.peerId) }
            Button(L("ok"), role: .cancel) { model.keyChanged = nil }
        } message: { alert in
            Text(L("key_changed_text", alert.name))
        }
    }

    private func peerName(_ security: HoppySecurity) -> String {
        switch model.status {
        case let .waiting(n), let .fallback(n), let .progress(n, _), let .delivered(n), let .declined(n): return n
        default: return security.peer?.name ?? "?"
        }
    }

    private var isDelivered: Bool {
        if case .delivered = model.status { return true }
        return false
    }

    private var totalSize: UInt64 {
        model.files.reduce(0) { sum, url in
            sum + UInt64(max((try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0, 0))
        }
    }

    private var statusText: String? {
        switch model.status {
        case .idle, .preparing: return nil
        case let .waiting(name): return L("waiting_accept", name)
        case let .fallback(name): return L("send_trying_callback", name)
        case let .progress(name, percent): return L("sending_progress", name, percent)
        case let .delivered(name): return L("delivered", name)
        case let .declined(name): return L("declined", name)
        case let .failed(message): return message
        }
    }
}
