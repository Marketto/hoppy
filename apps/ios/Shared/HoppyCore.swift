// Swift wrapper over the Rust core (crates/hoppy-ffi, C header crates/hoppy-ffi/include/hoppy.h,
// linked as HoppyFFI.xcframework). JSON in, JSON events out; no protocol logic here: the wire
// format lives only in hoppy-core (invariant 1, ADR 0009).
import Foundation
import HoppyFFI

struct HoppyPeer: Codable, Identifiable, Hashable {
    let id: String
    let name: String
    let platform: String
    let addr: String
    /// Protocol v1 (spec §7): the device encrypts. nil from an older core.
    var encrypted: Bool?
    /// "new", "known" or "verified" (trust store, by id).
    var trust: String?
}

/// A device's identity key as the core reports it (PEER in crates/hoppy-ffi/src/lib.rs).
struct HoppyPeerKey: Codable, Hashable {
    let id: String
    let name: String
    let platform: String
    let fingerprint: String
}

/// The verification code (spec §7.6): 4 emoji or 8 digits of the same 24-bit value.
struct HoppySas: Decodable, Hashable {
    let value: UInt32
    let emoji: [Int]
    let text: String
    let digits: String
    let it: [String]
    let en: [String]

    /// The emoji one by one (`text` holds them as one string).
    var symbols: [String] { text.map { String($0) } }

    /// Names for VoiceOver, in the app's language.
    var names: [String] { Bundle.main.preferredLocalizations.first == "it" ? it : en }
}

/// SECURITY of an offer / send session (spec §7): who is on the other end and how sure we are.
struct HoppySecurity: Decodable, Hashable {
    let encrypted: Bool
    /// "new", "known", "verified" or "key_changed" (then the core blocks the transfer).
    var trust: String?
    var nameMatchesKnown: Bool?
    var peer: HoppyPeerKey?
    var sas: HoppySas?

    static let plain = HoppySecurity(encrypted: false)

    var isVerified: Bool { encrypted && trust == "verified" }
    /// Encrypted, with a code worth comparing.
    var needsCheck: Bool { encrypted && (trust == "new" || trust == "known") && sas != nil && peer != nil }

    enum CodingKeys: String, CodingKey {
        case encrypted, trust, peer, sas
        case nameMatchesKnown = "name_matches_known"
    }
}

struct HoppyOfferFile: Codable, Hashable {
    let name: String
    let size: UInt64
}

struct HoppyOffer: Identifiable, Hashable {
    let id: UInt64
    let fromName: String
    let fromPlatform: String
    let files: [HoppyOfferFile]
    let count: Int
    let total: UInt64
    var security: HoppySecurity = .plain
}

enum HoppySendOutcome: String {
    case delivered, declined, unreachable, error
    /// The receiver's key differs from the remembered one: nothing was sent (spec §7.5).
    case keyChanged = "key_changed"
    /// A device known to encrypt answered in plaintext: refused.
    case unencrypted
}

enum HoppyEvent {
    case peers([HoppyPeer])
    /// A device left the list (its bye, or 15 s of silence); `peers` already reflects it.
    case peerLost(String)
    case receiving(on: Bool, port: Int)
    case offer(HoppyOffer)
    case offerClosed(UInt64)
    case receiveProgress(done: UInt64, total: UInt64)
    case received(ok: Bool, from: String, files: [URL], error: String?)
    case sendProgress(transfer: UInt64, done: UInt64, total: UInt64)
    case sendFallback(transfer: UInt64)
    case sendDone(transfer: UInt64, outcome: HoppySendOutcome, message: String?)
    /// Before the offer goes out: show the code while waiting for the receiver.
    case sendSession(transfer: UInt64, security: HoppySecurity)
    /// An offer declined without asking ("key_changed" / "encryption_required").
    case blocked(id: String, name: String, platform: String, reason: String)
    /// A device scanned our QR code (spec §7.7).
    case verified(HoppyPeerKey)
    /// Repeated interrupted handshakes from one address (someone may guess codes, spec §7.3).
    case handshakeAborts(from: String, recent: Int)
    /// The trust store could not be written (receiver side, or `send_warning` on the sender).
    case trustNotSaved(String)
    /// Result of `qrScan`: `local` = marked verified here, `remote` = the other side too.
    case qrDone(scan: UInt64, ok: Bool, local: Bool, remote: Bool, error: String?)
}

/// Everything the Rust side may put in an event (see crates/hoppy-ffi/src/lib.rs).
private struct RawEvent: Decodable {
    struct From: Decodable { let id: String?; let name: String; let platform: String }
    let type: String
    let id: String?
    let peers: [HoppyPeer]?
    let on: Bool?
    let port: Int?
    let offer: UInt64?
    let from: FromField?
    let files: FilesField?
    let count: Int?
    let total: UInt64?
    let done: UInt64?
    let ok: Bool?
    let error: String?
    let transfer: UInt64?
    let outcome: String?
    let message: String?
    let security: HoppySecurity?
    let reason: String?
    let peer: HoppyPeerKey?
    let recent: Int?
    let scan: UInt64?
    let local: Bool?
    let remote: Bool?

    /// `from` is an object in `offer`, a plain name in `received`.
    enum FromField: Decodable {
        case device(From), name(String)
        init(from decoder: Decoder) throws {
            let c = try decoder.singleValueContainer()
            if let s = try? c.decode(String.self) { self = .name(s) } else { self = .device(try c.decode(From.self)) }
        }
    }

    /// `files` is a list of {name,size} in `offer`, of paths in `received`.
    enum FilesField: Decodable {
        case offered([HoppyOfferFile]), paths([String])
        init(from decoder: Decoder) throws {
            let c = try decoder.singleValueContainer()
            if let p = try? c.decode([String].self) { self = .paths(p) } else { self = .offered(try c.decode([HoppyOfferFile].self)) }
        }
    }

    var event: HoppyEvent? {
        switch type {
        case "peers":
            return .peers(peers ?? [])
        case "peer_lost":
            return id.map { .peerLost($0) }
        case "receiving":
            return .receiving(on: on ?? false, port: port ?? 0)
        case "offer":
            guard let id = offer, case let .device(sender)? = from else { return nil }
            var list: [HoppyOfferFile] = []
            if case let .offered(f)? = files { list = f }
            return .offer(HoppyOffer(id: id, fromName: sender.name, fromPlatform: sender.platform,
                                     files: list, count: count ?? list.count, total: total ?? 0,
                                     security: security ?? .plain))
        case "offer_closed":
            return offer.map { .offerClosed($0) }
        case "receive_progress":
            return .receiveProgress(done: done ?? 0, total: total ?? 0)
        case "received":
            var name = "?"
            if case let .name(n)? = from { name = n }
            var urls: [URL] = []
            if case let .paths(p)? = files { urls = p.map { URL(fileURLWithPath: $0) } }
            return .received(ok: ok ?? false, from: name, files: urls, error: error)
        case "send_progress":
            guard let t = transfer else { return nil }
            return .sendProgress(transfer: t, done: done ?? 0, total: total ?? 0)
        case "send_fallback":
            return transfer.map { .sendFallback(transfer: $0) }
        case "send_done":
            guard let t = transfer else { return nil }
            return .sendDone(transfer: t, outcome: HoppySendOutcome(rawValue: outcome ?? "") ?? .error, message: message)
        case "send_session":
            guard let t = transfer else { return nil }
            return .sendSession(transfer: t, security: security ?? .plain)
        case "blocked":
            guard case let .device(d)? = from else { return nil }
            return .blocked(id: d.id ?? "", name: d.name, platform: d.platform, reason: reason ?? "")
        case "verified":
            return peer.map { .verified($0) }
        case "handshake_aborts":
            var ip = "?"
            if case let .name(n)? = from { ip = n }
            return .handshakeAborts(from: ip, recent: recent ?? 0)
        case "trust_not_saved", "send_warning":
            return .trustNotSaved(error ?? "?")
        case "qr_done":
            guard let s = scan else { return nil }
            return .qrDone(scan: s, ok: ok ?? false, local: local ?? false, remote: remote ?? false, error: error)
        default:
            return nil // newer core, older app: ignore
        }
    }
}

struct HoppyConfig: Encodable {
    var id: String
    var name: String
    var platform = "ios"
    var receive_dir: String?
    var udp_port: UInt16 = 47470
    var tcp_port: UInt16 = 47471
    var receive = false
    var multicast = false
    /// "subnets", "off", or explicit "ip:port" targets (tests): encoded as a JSON value.
    var sweep: Sweep = .subnets
    var seeds: [String] = []
    var decision_timeout_secs: UInt64 = 110
    /// Protocol v1 device key and trust store (spec §7.5), in this container's private
    /// Application Support folder (`Identity.startNode`). nil = throw-away key (tests only).
    var key_file: String?
    var trust_file: String?

    enum Sweep: Encodable {
        case subnets, off, hosts([String])
        func encode(to encoder: Encoder) throws {
            var c = encoder.singleValueContainer()
            switch self {
            case .subnets: try c.encode("subnets")
            case .off: try c.encode("off")
            case let .hosts(h): try c.encode(h)
            }
        }
    }
}

struct HoppyError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// Receives the C callback on Rust threads and forwards decoded events.
private final class EventSink {
    let handler: (HoppyEvent) -> Void
    init(_ handler: @escaping (HoppyEvent) -> Void) { self.handler = handler }
}

/// One running Rust node. Not thread-safe: use it from one thread (the main one in the app).
/// Events arrive on background threads; the handler must hop to the main queue itself.
final class HoppyNode {
    private var raw: OpaquePointer?
    private let sink: Unmanaged<EventSink>

    static var version: String { String(cString: hoppy_version()) }

    init(config: HoppyConfig, onEvent: @escaping (HoppyEvent) -> Void) throws {
        let json = String(decoding: try JSONEncoder().encode(config), as: UTF8.self)
        let sink = Unmanaged.passRetained(EventSink(onEvent))
        var err: UnsafeMutablePointer<CChar>?
        let callback: hoppy_event_cb = { ctx, text in
            guard let ctx = ctx, let text = text else { return }
            let sink = Unmanaged<EventSink>.fromOpaque(ctx).takeUnretainedValue()
            let data = Data(bytes: text, count: strlen(text))
            if let raw = try? JSONDecoder().decode(RawEvent.self, from: data), let event = raw.event {
                sink.handler(event)
            }
        }
        let node = json.withCString { hoppy_node_start($0, callback, sink.toOpaque(), &err) }
        guard let node = node else {
            sink.release()
            let message = err.map { String(cString: $0) } ?? "unknown error"
            if let err = err { hoppy_string_free(err) }
            throw HoppyError(message: message)
        }
        self.raw = node
        self.sink = sink
    }

    deinit { stop() }

    /// Sends `bye`, closes the receiver, declines a pending question. Idempotent.
    func stop() {
        guard let raw = raw else { return }
        self.raw = nil
        hoppy_node_stop(raw) // no callback runs after this returns
        sink.release()
    }

    var isRunning: Bool { raw != nil }

    private func takeString(_ p: UnsafeMutablePointer<CChar>?) -> Data? {
        guard let p = p else { return nil }
        defer { hoppy_string_free(p) }
        return Data(bytes: p, count: strlen(p))
    }

    var peers: [HoppyPeer] {
        guard let raw = raw, let data = takeString(hoppy_node_peers(raw)) else { return [] }
        return (try? JSONDecoder().decode([HoppyPeer].self, from: data)) ?? []
    }

    struct Info: Decodable {
        let id: String
        let name: String
        let udp_port: UInt16
        let tcp_port: UInt16
        /// This device's key fingerprint (spec §7.5).
        let fingerprint: String?
        /// A damaged trust store was set aside at start.
        let trust_warning: String?
    }

    var info: Info? {
        guard let raw = raw, let data = takeString(hoppy_node_info(raw)) else { return nil }
        return try? JSONDecoder().decode(Info.self, from: data)
    }

    func refresh() {
        if let raw = raw { hoppy_node_refresh(raw) }
    }

    /// Returns the TCP port (0 = off) or nil on error.
    @discardableResult
    func setReceiving(_ on: Bool) -> Int? {
        guard let raw = raw else { return nil }
        let r = hoppy_node_set_receiving(raw, on)
        return r < 0 ? nil : Int(r)
    }

    /// Starts sending `files` (local file URLs) to `peerId`; returns the transfer id (0 = invalid).
    func send(_ files: [URL], to peerId: String) -> UInt64 {
        guard let raw = raw,
              let data = try? JSONEncoder().encode(files.map { $0.path }) else { return 0 }
        let paths = String(decoding: data, as: UTF8.self)
        return peerId.withCString { id in paths.withCString { hoppy_node_send(raw, id, $0) } }
    }

    @discardableResult
    func decide(_ offer: UInt64, accept: Bool) -> Bool {
        guard let raw = raw else { return false }
        return hoppy_node_decide(raw, offer, accept)
    }

    // MARK: trust (spec §7)

    struct TrustList: Decodable {
        struct Me: Decodable { let fingerprint: String }
        struct Entry: Decodable { let id: String; let name: String; let fingerprint: String; let verified: Bool }
        let me: Me
        let peers: [Entry]
    }

    var trustList: TrustList? {
        guard let raw = raw, let data = takeString(hoppy_node_trust_list(raw)) else { return nil }
        return try? JSONDecoder().decode(TrustList.self, from: data)
    }

    /// "Dimentica dispositivo": its next contact is New (needed after a KeyChanged block).
    @discardableResult
    func forget(_ peerId: String) -> Bool {
        guard let raw = raw else { return false }
        return peerId.withCString { hoppy_node_trust_forget(raw, $0) }
    }

    /// "Coincide": false if another key is remembered for that id.
    @discardableResult
    func verify(_ peer: HoppyPeerKey) -> Bool {
        guard let raw = raw, let json = Self.json(peer) else { return false }
        return json.withCString { hoppy_node_trust_verify(raw, $0) }
    }

    /// Payload to render as a QR code (valid while shown, at most 10 minutes).
    func qrBegin() -> String? {
        guard let raw = raw, let data = takeString(hoppy_node_qr_begin(raw)) else { return nil }
        return String(decoding: data, as: UTF8.self)
    }

    func qrEnd() {
        if let raw = raw { hoppy_node_qr_end(raw) }
    }

    /// Checks a scanned payload against `live` (the device on screen) or the remembered key,
    /// then tells that device; the result arrives as `.qrDone`. 0 = not a Hoppy QR code.
    func qrScan(_ payload: String, live: HoppyPeerKey?) -> UInt64 {
        guard let raw = raw else { return 0 }
        let liveJson = live.flatMap { Self.json($0) }
        return payload.withCString { (p: UnsafePointer<CChar>) -> UInt64 in
            if let liveJson = liveJson {
                return liveJson.withCString { hoppy_node_qr_scan(raw, p, $0) }
            }
            return hoppy_node_qr_scan(raw, p, nil)
        }
    }

    private static func json<T: Encodable>(_ value: T) -> String? {
        (try? JSONEncoder().encode(value)).map { String(decoding: $0, as: UTF8.self) }
    }
}
