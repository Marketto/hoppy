// Device identity and discovery seeds, persisted in UserDefaults, and the protocol v1 key and
// trust store (spec §7.5) in Application Support. The app and the Share extension have
// separate containers (no App Group or shared keychain with a free signature, ADR 0009/0010):
// the extension is a separate device to others, with its own stable id, key and trust store,
// named "<model> (Condividi)", so desktops never see one id with two keys (KeyChanged).
import Foundation
import UIKit
import UniformTypeIdentifiers

enum Identity {
    private static let idKey = "hoppy.device.id"
    private static let nameKey = "hoppy.device.name"
    private static let recentKey = "hoppy.recent.ips"
    private static let manualKey = "hoppy.manual.ips"
    /// Fingerprint of the key the current id is bound to.
    private static let fingerprintKey = "hoppy.key.fingerprint"
    static let maxRecent = 16

    static var deviceId: String {
        let d = UserDefaults.standard
        if let id = d.string(forKey: idKey), !id.isEmpty { return id }
        return newDeviceId()
    }

    @discardableResult
    private static func newDeviceId() -> String {
        let id = UUID().uuidString.lowercased()
        UserDefaults.standard.set(id, forKey: idKey)
        return id
    }

    /// The Share extension's name: its container cannot read the name set in the app.
    static var shareExtensionName: String { L("share_device_name", UIDevice.current.model) }

    // MARK: protocol v1 key and trust store

    /// `Library/Application Support/Hoppy` of this container: private (not in Documents, which
    /// Files shows), excluded from backups so a restored or cloned device never shares a key.
    static func keyDirectory() throws -> URL {
        let fm = FileManager.default
        var dir = try fm.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent("Hoppy", isDirectory: true)
        try fm.createDirectory(at: dir, withIntermediateDirectories: true,
                               attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication])
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? dir.setResourceValues(values)
        return dir
    }

    /// Starts a node with this container's persistent key (`identity.key`) and trust store
    /// (`trust.json`). The device id is bound to the key: when the key is new (first run, a
    /// restore from backup, a damaged key file the core replaced) the id is new too, so other
    /// devices see a New device instead of blocking a known id as KeyChanged.
    static func startNode(name: String, configure: (inout HoppyConfig) -> Void,
                          onEvent: @escaping (HoppyEvent) -> Void) throws -> HoppyNode {
        let dir = try keyDirectory()
        let keyFile = dir.appendingPathComponent("identity.key")
        let trustFile = dir.appendingPathComponent("trust.json")
        let d = UserDefaults.standard
        let bound = d.string(forKey: fingerprintKey)
        let hadKey = FileManager.default.fileExists(atPath: keyFile.path)
        if !hadKey || bound == nil { newDeviceId() }
        var base = HoppyConfig(id: "", name: name)
        base.key_file = keyFile.path
        base.trust_file = trustFile.path
        configure(&base)
        func make() throws -> HoppyNode {
            var config = base
            config.id = deviceId
            return try HoppyNode(config: config, onEvent: onEvent)
        }
        var node = try make()
        if let fp = node.info?.fingerprint {
            if hadKey, let bound = bound, bound != fp {
                node.stop()
                newDeviceId()
                node = try make()
            }
            d.set(fp, forKey: fingerprintKey)
        }
        return node
    }

    /// iOS 16+ only reveals the generic model ("iPad") without a special entitlement, so the
    /// user can rename the device in the app.
    static var deviceName: String {
        get {
            let saved = UserDefaults.standard.string(forKey: nameKey)?.trimmingCharacters(in: .whitespacesAndNewlines)
            if let saved = saved, !saved.isEmpty { return saved }
            return UIDevice.current.model
        }
        set {
            let clean = String(newValue.trimmingCharacters(in: .whitespacesAndNewlines).prefix(64))
            UserDefaults.standard.set(clean.isEmpty ? nil : clean, forKey: nameKey)
        }
    }

    /// IPs typed by the user (networks the sweep cannot cover, e.g. a /16 office LAN).
    static var manualIPs: [String] {
        get { UserDefaults.standard.stringArray(forKey: manualKey) ?? [] }
        set { UserDefaults.standard.set(Array(newValue.prefix(maxRecent)), forKey: manualKey) }
    }

    /// Recently seen peers: queried directly at start, before the sweep reaches them.
    static var recentIPs: [String] {
        get { UserDefaults.standard.stringArray(forKey: recentKey) ?? [] }
        set { UserDefaults.standard.set(Array(newValue.prefix(maxRecent)), forKey: recentKey) }
    }

    static func remember(_ peers: [HoppyPeer]) {
        var list = recentIPs.filter(isPrivateIPv4)
        for p in peers where isPrivateIPv4(p.addr) {
            list.removeAll { $0 == p.addr }
            list.insert(p.addr, at: 0)
        }
        if list != recentIPs { recentIPs = list }
    }

    /// Manual entries as typed, recent peers only from private LAN ranges.
    static var seeds: [String] {
        var out: [String] = []
        for ip in manualIPs + recentIPs.filter(isPrivateIPv4) where !out.contains(ip) { out.append(ip) }
        return out
    }

    /// 10/8, 172.16/12, 192.168/16 (where Hoppy's LAN peers live).
    static func isPrivateIPv4(_ s: String) -> Bool {
        guard isIPv4(s) else { return false }
        let o = s.split(separator: ".").compactMap { Int($0) }
        return o[0] == 10 || (o[0] == 172 && (16...31).contains(o[1])) || (o[0] == 192 && o[1] == 168)
    }

    static func isIPv4(_ s: String) -> Bool {
        let parts = s.split(separator: ".", omittingEmptySubsequences: false)
        return parts.count == 4 && parts.allSatisfy { p in
            !p.isEmpty && p.count <= 3 && p.allSatisfy(\.isNumber) && (Int(p) ?? 256) <= 255
        }
    }
}

/// Copies picked/shared files into a private outbox (APFS clones: instant, no extra space
/// until changed) so they stay readable for the whole send, keeping their names.
enum Outbox {
    static var root: URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("outbox", isDirectory: true)
    }

    /// Copies `url` (security-scoped if needed) into its own folder; returns the copy.
    static func stage(_ url: URL, name: String? = nil) throws -> URL {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let dir = root.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let fileName = (name?.isEmpty == false ? name! : url.lastPathComponent)
        let dest = dir.appendingPathComponent(safeName(fileName))
        try FileManager.default.copyItem(at: url, to: dest)
        return dest
    }

    /// A name usable as a local file name (the receiver sanitizes again, spec §4).
    static func safeName(_ name: String) -> String {
        let cleaned = name.replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: ":", with: "_")
        let trimmed = cleaned.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty || trimmed == "." || trimmed == ".." ? "file" : trimmed
    }

    static func remove(_ files: [URL]) {
        for f in files where f.path.hasPrefix(root.path) {
            try? FileManager.default.removeItem(at: f.deletingLastPathComponent())
        }
    }

    static func clear() {
        try? FileManager.default.removeItem(at: root)
    }

    /// Loads a file out of an item provider (Photos picker, Share extension) into the outbox.
    static func stage(_ provider: NSItemProvider, completion: @escaping (URL?) -> Void) {
        let fileURL = "public.file-url"
        if provider.hasItemConformingToTypeIdentifier(fileURL) {
            provider.loadItem(forTypeIdentifier: fileURL, options: nil) { item, _ in
                var url = item as? URL
                if url == nil, let data = item as? Data { url = URL(dataRepresentation: data, relativeTo: nil) }
                completion(url.flatMap { try? stage($0) })
            }
            return
        }
        // Prefer the original media (HEIC, MOV…) over derived representations.
        let types = provider.registeredTypeIdentifiers
        let wanted: [UTType] = [.movie, .image, .audio, .pdf, .data]
        let preferred = types.first { t in
            guard let ut = UTType(t) else { return false }
            return wanted.contains { ut.conforms(to: $0) }
        } ?? types.first
        guard let type = preferred else { return completion(nil) }
        provider.loadFileRepresentation(forTypeIdentifier: type) { url, _ in
            // The temporary file vanishes when this closure returns: copy it now.
            guard let url = url else { return completion(nil) }
            var name = url.lastPathComponent
            if let suggested = provider.suggestedName, !suggested.isEmpty {
                let ext = url.pathExtension
                name = ext.isEmpty || suggested.lowercased().hasSuffix("." + ext.lowercased()) ? suggested : suggested + "." + ext
            }
            completion(try? stage(url, name: name))
        }
    }

    /// Stages every provider; calls back on the main queue with the staged files in order.
    static func stage(_ providers: [NSItemProvider], completion: @escaping ([URL]) -> Void) {
        var results = [URL?](repeating: nil, count: providers.count)
        let group = DispatchGroup()
        let lock = NSLock()
        for (i, p) in providers.enumerated() {
            group.enter()
            stage(p) { url in
                lock.lock()
                results[i] = url
                lock.unlock()
                group.leave()
            }
        }
        group.notify(queue: .main) { completion(results.compactMap { $0 }) }
    }
}
