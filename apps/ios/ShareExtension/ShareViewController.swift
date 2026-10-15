// "Condividi → Hoppy": a self-contained Share extension. It stages the shared files into its
// own temporary folder (APFS clones, nothing is loaded into memory: extensions are killed
// above ~120 MB), runs its own send-only Rust node (ephemeral UDP port, unicast sweep), lets
// the user tap a device and streams the files. No App Group, no URL-scheme hand-off to the
// app (ADR 0009).
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
    var close: (Bool) -> Void = { _ in }

    private var node: HoppyNode?
    private var transfer: (id: UInt64, peer: String)?

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
        var config = HoppyConfig(id: Identity.deviceId, name: Identity.deviceName) // own defaults: own id
        config.udp_port = 0 // never take 47470 from the app
        config.receive = false
        config.seeds = Identity.seeds
        node = try? HoppyNode(config: config) { [weak self] event in
            DispatchQueue.main.async { self?.handle(event) }
        }
        if node == nil { status = .failed(L("send_failed", L("not_running"))) }
    }

    func send(to peer: HoppyPeer) {
        guard let node = node, !files.isEmpty, !sending else { return }
        let id = node.send(files, to: peer.id)
        guard id != 0 else {
            status = .failed(L("nothing_to_send"))
            return
        }
        transfer = (id, peer.name)
        status = .waiting(peer.name)
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
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { [weak self] in
                    self?.shutdown()
                    self?.close(true)
                }
            case .declined: status = .declined(t.peer)
            case .unreachable: status = .failed(L("send_unreachable", t.peer))
            case .error: status = .failed(L("send_failed", message ?? "?"))
            }
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
