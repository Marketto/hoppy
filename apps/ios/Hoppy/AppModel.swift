// The app's state: one Rust node while the app is in the foreground (iOS gives no
// background listening without special entitlements), receive toggle, sends, the accept
// question and the last received files. Used only on the main thread (events are posted
// there with DispatchQueue.main.async, which keeps their order).
import AVFoundation
import SwiftUI
import UIKit

final class AppModel: ObservableObject {
    @Published private(set) var peers: [HoppyPeer] = []
    @Published private(set) var receiving: Bool
    @Published private(set) var receivePercent: Int?
    @Published var offer: HoppyOffer?
    @Published private(set) var sendStatus: SendStatus = .idle
    @Published private(set) var lastReceived: [URL] = []
    @Published private(set) var lastReceivedFrom = ""
    @Published var receiveError: String?
    @Published var startError: String?
    /// Files waiting for the user to pick a device ("Invia a" sheet).
    @Published var pendingFiles: [URL] = []
    @Published var deviceName: String
    @Published var soundOn: Bool {
        didSet { UserDefaults.standard.set(soundOn, forKey: Self.soundKey) }
    }

    private var node: HoppyNode?
    private var transfer: (id: UInt64, peer: String, files: [URL])?
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid
    private var inForeground = false
    private var player: AVAudioPlayer?
    private static let receiveKey = "hoppy.receive"
    private static let soundKey = "hoppy.sound"

    /// Under XCTest the app host must not start a node (tests run their own).
    private let isTesting = ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] != nil

    init() {
        let d = UserDefaults.standard
        receiving = d.object(forKey: Self.receiveKey) as? Bool ?? true
        soundOn = d.object(forKey: Self.soundKey) as? Bool ?? true
        deviceName = Identity.deviceName
        Outbox.clear()
    }

    var receiveDir: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    }

    var isBusy: Bool {
        if receivePercent != nil || offer != nil { return true }
        switch sendStatus {
        case .preparing, .waiting, .fallback, .progress: return true
        default: return false
        }
    }

    // MARK: lifecycle

    func enterForeground() {
        inForeground = true
        endBackgroundTask()
        if node == nil {
            startNode()
        } else {
            // Back from a system prompt (Local Network) or a short trip away: sweep again.
            node?.refresh()
        }
    }

    /// Stops the node at once unless a transfer runs: then iOS gives us ~30 s to finish.
    func enterBackground() {
        inForeground = false
        guard node != nil else { return }
        if isBusy {
            backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "hoppy-transfer") { [weak self] in
                DispatchQueue.main.async {
                    self?.stopNode()
                    self?.endBackgroundTask()
                }
            }
        } else {
            stopNode()
        }
    }

    private func endBackgroundTask() {
        if backgroundTask != .invalid {
            UIApplication.shared.endBackgroundTask(backgroundTask)
            backgroundTask = .invalid
        }
    }

    /// Called when a transfer ends: finish a pending background stop.
    private func transferEnded() {
        UIApplication.shared.isIdleTimerDisabled = false
        if !inForeground && !isBusy {
            stopNode()
            endBackgroundTask()
        }
    }

    func startNode() {
        guard node == nil, !isTesting else { return }
        removeStaleParts()
        var config = HoppyConfig(id: Identity.deviceId, name: deviceName)
        config.receive_dir = receiveDir.path
        config.receive = receiving
        config.seeds = Identity.seeds
        do {
            node = try HoppyNode(config: config) { [weak self] event in
                DispatchQueue.main.async { self?.handle(event) }
            }
            startError = nil
        } catch {
            startError = error.localizedDescription
        }
    }

    /// A transfer cut off by iOS (app killed while suspended) can leave `*.hoppy-part` files
    /// in Documents (visible in Files). No transfer runs while the node is down: delete them.
    private func removeStaleParts() {
        let fm = FileManager.default
        guard let items = try? fm.contentsOfDirectory(at: receiveDir, includingPropertiesForKeys: nil) else { return }
        for item in items where item.lastPathComponent.hasSuffix(".hoppy-part") {
            try? fm.removeItem(at: item)
        }
    }

    func stopNode() {
        node?.stop()
        node = nil
        peers = []
        offer = nil
        receivePercent = nil
        if case .progress = sendStatus { sendStatus = .idle }
        if case .waiting = sendStatus { sendStatus = .idle }
        if case .fallback = sendStatus { sendStatus = .idle }
        if case .preparing = sendStatus { sendStatus = .idle }
        if let t = transfer { Outbox.remove(t.files) }
        transfer = nil
    }

    private func restartNode() {
        stopNode()
        if inForeground { startNode() }
    }

    func refresh() { node?.refresh() }

    // MARK: settings

    func setReceiving(_ on: Bool) {
        receiving = on
        UserDefaults.standard.set(on, forKey: Self.receiveKey)
        if node?.setReceiving(on) == nil, node != nil {
            receiving = false
            receiveError = L("receive_port_error")
        }
    }

    func rename(_ name: String) {
        Identity.deviceName = name
        deviceName = Identity.deviceName
        restartNode() // the name travels in every datagram and offer
    }

    func addAddress(_ ip: String) -> Bool {
        let ip = ip.trimmingCharacters(in: .whitespacesAndNewlines)
        guard Identity.isIPv4(ip) else { return false }
        Identity.manualIPs = [ip] + Identity.manualIPs.filter { $0 != ip }
        restartNode()
        return true
    }

    // MARK: sending

    /// Files picked or opened in Hoppy: ask where to send them.
    func offerToSend(_ files: [URL]) {
        guard !files.isEmpty else { return }
        pendingFiles = files
        refresh()
    }

    func cancelPending() {
        Outbox.remove(pendingFiles)
        pendingFiles = []
    }

    func send(_ files: [URL], to peer: HoppyPeer) {
        pendingFiles = []
        guard let node = node else {
            sendStatus = .failed(L("send_failed", L("not_running")))
            Outbox.remove(files)
            return
        }
        let id = node.send(files, to: peer.id)
        guard id != 0 else {
            sendStatus = .failed(L("nothing_to_send"))
            Outbox.remove(files)
            return
        }
        transfer = (id, peer.name, files)
        sendStatus = .waiting(peer.name)
        UIApplication.shared.isIdleTimerDisabled = true
    }

    func setPreparing() { sendStatus = .preparing }
    func clearSendStatus() { sendStatus = .idle }

    // MARK: receiving

    func decide(accept: Bool) {
        guard let offer = offer else { return }
        node?.decide(offer.id, accept: accept)
        self.offer = nil
        if accept {
            receivePercent = 0
            UIApplication.shared.isIdleTimerDisabled = true
        }
    }

    func clearReceived() { lastReceived = [] }

    // MARK: events

    private func handle(_ event: HoppyEvent) {
        guard node != nil else { return }
        switch event {
        case let .peers(list):
            peers = list
            Identity.remember(list)
        case let .receiving(on, _):
            receiving = on
        case let .offer(o):
            offer = o
            UINotificationFeedbackGenerator().notificationOccurred(.warning)
        case let .offerClosed(id):
            if offer?.id == id { offer = nil }
        case let .receiveProgress(done, total):
            receivePercent = Format.percent(done, total)
        case let .received(ok, from, files, error):
            receivePercent = nil
            if ok {
                lastReceived = files
                lastReceivedFrom = from
                UINotificationFeedbackGenerator().notificationOccurred(.success)
            } else {
                receiveError = L("receive_failed", from) + (error.map { "\n" + $0 } ?? "")
            }
            transferEnded()
        case let .sendProgress(id, done, total):
            guard let t = transfer, t.id == id else { return }
            sendStatus = .progress(t.peer, Format.percent(done, total))
        case let .sendFallback(id):
            guard let t = transfer, t.id == id else { return }
            sendStatus = .fallback(t.peer)
        case let .sendDone(id, outcome, message):
            guard let t = transfer, t.id == id else { return }
            transfer = nil
            Outbox.remove(t.files)
            switch outcome {
            case .delivered:
                sendStatus = .delivered(t.peer)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                playJingle()
            case .declined:
                sendStatus = .declined(t.peer)
            case .unreachable:
                sendStatus = .failed(L("send_unreachable", t.peer))
            case .error:
                sendStatus = .failed(L("send_failed", message ?? "?"))
            }
            transferEnded()
        }
    }

    /// The delivery jingle (assets/music/desktop_done.wav, generated): ambient category, so
    /// the silent switch mutes it and other audio keeps playing.
    private func playJingle() {
        guard soundOn, let url = Bundle.main.url(forResource: "desktop_done", withExtension: "wav") else { return }
        try? AVAudioSession.sharedInstance().setCategory(.ambient, options: [.mixWithOthers])
        player = try? AVAudioPlayer(contentsOf: url)
        player?.volume = 0.6
        player?.play()
    }
}
