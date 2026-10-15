// The app's state: one Rust node while the app is in the foreground (iOS gives no
// background listening without special entitlements), receive toggle, sends, the accept
// question, the last received files, device verification (spec §7: code, QR, KeyChanged)
// and the update notice. Used only on the main thread (events are posted there with
// DispatchQueue.main.async, which keeps their order).
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

    // Verification (spec §7, ADR 0010)
    /// Which screen a verification action belongs to: the incoming offer or the running send.
    enum Session { case offer, send }
    /// Security of the running send (from `send_session`).
    @Published private(set) var sendSecurity: HoppySecurity?
    @Published var sendCheckHidden = false
    @Published var offerCheckHidden = false
    /// Result of "Coincide" / a QR scan, shown in the panel of `noteFor`.
    @Published private(set) var securityNote: String?
    @Published private(set) var noteFor: Session?
    /// The camera scanner is open for this session.
    @Published var scanningFor: Session?
    /// "Mostra il mio QR" is open (payload) over this session's screen.
    @Published private(set) var myQR: (payload: String, over: Session)?
    @Published var keyChanged: KeyChangedAlert?
    @Published var banner: String?
    /// A newer release to announce (UpdateCheck).
    @Published private(set) var update: String?

    private var node: HoppyNode?
    private var transfer: (id: UInt64, peerId: String, peer: String, files: [URL])?
    private var pendingScan: (id: UInt64, session: Session, name: String)?
    /// Receiving was turned on only so a scan of "my QR" can reach us.
    private var receivingForQR = false
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
        if !isTesting {
            UpdateCheck.runIfDue { [weak self] version in self?.update = version }
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
        let dir = receiveDir.path, receive = receiving, seeds = Identity.seeds
        do {
            // Persistent key + trust store (Application Support), id bound to the key.
            node = try Identity.startNode(name: deviceName, configure: { config in
                config.receive_dir = dir
                config.receive = receive
                config.seeds = seeds
            }, onEvent: { [weak self] event in
                DispatchQueue.main.async { self?.handle(event) }
            })
            startError = nil
            if let warning = node?.info?.trust_warning { banner = L("trust_problem", warning) }
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
        sendSecurity = nil
        pendingScan = nil
        scanningFor = nil
        myQR = nil
        receivingForQR = false
        securityNote = nil
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
        transfer = (id, peer.id, peer.name, files)
        sendSecurity = nil
        sendCheckHidden = false
        clearNote(.send)
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

    // MARK: verification (spec §7)

    private func security(of session: Session) -> HoppySecurity? {
        session == .offer ? offer?.security : sendSecurity
    }

    /// The session on screen is now verified (✓) if it is this key.
    private func markVerified(_ key: HoppyPeerKey) {
        if var o = offer, o.security.peer?.id == key.id, o.security.peer?.fingerprint == key.fingerprint {
            o.security.trust = "verified"
            offer = o
        }
        if var s = sendSecurity, s.peer?.id == key.id, s.peer?.fingerprint == key.fingerprint {
            s.trust = "verified"
            sendSecurity = s
        }
    }

    private func note(_ text: String, _ session: Session) {
        securityNote = text
        noteFor = session
    }

    private func clearNote(_ session: Session) {
        if noteFor == session { securityNote = nil; noteFor = nil }
    }

    /// "Coincide": both screens show the same code.
    func confirmMatch(_ session: Session) {
        guard let node = node, let sec = security(of: session), let peer = sec.peer else { return }
        if node.verify(peer) {
            markVerified(peer)
            note(L("sec_verified_now", peer.name), session)
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        } else {
            note(L("sec_verify_failed", peer.name), session)
        }
    }

    func later(_ session: Session) {
        if session == .offer { offerCheckHidden = true } else { sendCheckHidden = true }
    }

    /// "Verifica con QR": camera if allowed, otherwise compare the code.
    func startScan(_ session: Session) {
        CameraAccess.request { [weak self] ok in
            guard let self = self else { return }
            if ok {
                self.scanningFor = session
            } else {
                self.note(L(CameraAccess.hasCamera ? "camera_denied" : "camera_unavailable"), session)
            }
        }
    }

    func cancelScan() { scanningFor = nil }

    func scanned(_ payload: String) {
        guard let session = scanningFor else { return }
        scanningFor = nil
        guard let node = node else { return }
        let live = security(of: session)?.peer
        let id = node.qrScan(payload, live: live)
        if id == 0 {
            note(L("qr_invalid"), session)
        } else {
            pendingScan = (id, session, live?.name ?? "?")
            note(L("qr_checking"), session)
        }
    }

    /// "Mostra il mio QR": the other device's scan reaches us through the receiver, so it is
    /// turned on while the code is shown.
    func showMyQR(over session: Session) {
        guard let node = node else { return }
        if !receiving {
            receivingForQR = node.setReceiving(true) != nil
        }
        if let payload = node.qrBegin() { myQR = (payload, session) }
    }

    func closeMyQR() {
        guard myQR != nil else { return }
        myQR = nil
        node?.qrEnd()
        if receivingForQR {
            receivingForQR = false
            if !(UserDefaults.standard.object(forKey: Self.receiveKey) as? Bool ?? true) {
                node?.setReceiving(false)
            }
        }
    }

    /// "Dimentica dispositivo": its next contact is New (trust on first use again).
    func forget(_ peerId: String) {
        node?.forget(peerId)
        keyChanged = nil
    }

    func dismissUpdate() {
        if let v = update { UpdateCheck.dismiss(v) }
        update = nil
    }

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
            offerCheckHidden = false
            clearNote(.offer)
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
            case .keyChanged:
                sendStatus = .failed(L("send_key_changed", t.peer))
                sendSecurity = nil
                keyChanged = KeyChangedAlert(peerId: t.peerId, name: t.peer)
                UINotificationFeedbackGenerator().notificationOccurred(.error)
            case .unencrypted:
                sendStatus = .failed(L("send_unencrypted", t.peer))
            case .error:
                sendStatus = .failed(L("send_failed", message ?? "?"))
            }
            // The send panel stays (the code can still be compared) until the next send.
            transferEnded()
        case let .sendSession(id, security):
            guard let t = transfer, t.id == id else { return }
            sendSecurity = security
        case let .blocked(id, name, _, reason):
            if reason == "key_changed" {
                keyChanged = KeyChangedAlert(peerId: id, name: name)
                UINotificationFeedbackGenerator().notificationOccurred(.error)
            } else {
                banner = L("plain_refused", name)
            }
        case let .verified(peer):
            markVerified(peer)
            banner = L("sec_verified_now", peer.name)
            closeMyQR()
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        case let .handshakeAborts(from, recent):
            banner = L("aborts_banner", from, recent)
        case let .trustNotSaved(error):
            banner = L("trust_not_saved", error)
        case let .qrDone(scan, ok, local, _, error):
            guard let p = pendingScan, p.id == scan else { return }
            pendingScan = nil
            if ok {
                note(L("sec_verified_now", p.name), p.session)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
            } else if local {
                note(L("qr_local_only", p.name, error ?? "?"), p.session)
            } else {
                note(L("qr_mismatch", error ?? "?"), p.session)
            }
            if local, let key = security(of: p.session)?.peer { markVerified(key) }
            if noteFor == .send && sendSecurity == nil, let text = securityNote { banner = text }
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
