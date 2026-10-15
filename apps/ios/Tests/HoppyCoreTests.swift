// FFI smoke tests on the iOS simulator (CI job "iOS · Rust core + app"): the Swift wrapper
// over the Rust core, two in-process nodes on loopback. Same scenario as the Rust test
// crates/hoppy-ffi/src/tests.rs, through Swift this time (module map, C callback, JSON).
import XCTest

final class HoppyCoreTests: XCTestCase {
    /// Collects events from a node's background threads.
    final class Recorder {
        private let lock = NSLock()
        private var events: [HoppyEvent] = []

        func add(_ e: HoppyEvent) { lock.lock(); events.append(e); lock.unlock() }
        var all: [HoppyEvent] { lock.lock(); defer { lock.unlock() }; return events }

        /// First match among the events after the first `after` ones.
        func wait<T>(_ what: String, after: Int = 0, timeout: TimeInterval = 10, _ match: (HoppyEvent) -> T?) -> T? {
            let deadline = Date().addingTimeInterval(timeout)
            while Date() < deadline {
                for e in all.dropFirst(after) { if let v = match(e) { return v } }
                Thread.sleep(forTimeInterval: 0.02)
            }
            XCTFail("timed out waiting for \(what)")
            return nil
        }
    }

    private func tempDir() throws -> URL {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    func testVersion() {
        XCTAssertFalse(HoppyNode.version.isEmpty)
    }

    func testStartStopWithUnicastSweep() throws {
        var config = HoppyConfig(id: UUID().uuidString.lowercased(), name: "Smoke")
        config.udp_port = 0
        config.tcp_port = 0
        config.sweep = .hosts(["127.0.0.1:9"])
        let node = try HoppyNode(config: config) { _ in }
        XCTAssertNotNil(node.info)
        XCTAssertEqual(node.info?.tcp_port, 0, "not receiving")
        node.refresh()
        XCTAssertTrue(node.peers.isEmpty)
        node.stop()
        node.stop() // idempotent
        XCTAssertFalse(node.isRunning)
    }

    func testBadConfigThrows() {
        let config = HoppyConfig(id: "", name: "x")
        XCTAssertThrowsError(try HoppyNode(config: config) { _ in })
    }

    func testLoopbackSendAcceptAndDecline() throws {
        let inbox = try tempDir()
        let rxEvents = Recorder(), txEvents = Recorder()

        var rxConfig = HoppyConfig(id: "rx-" + UUID().uuidString.lowercased(), name: "Receiver")
        rxConfig.platform = "linux"
        rxConfig.receive_dir = inbox.path
        rxConfig.udp_port = 0
        rxConfig.tcp_port = 0
        rxConfig.receive = true
        rxConfig.sweep = .off
        let rx = try HoppyNode(config: rxConfig) { rxEvents.add($0) }
        defer { rx.stop() }
        let rxUdp = try XCTUnwrap(rx.info?.udp_port)
        XCTAssertNotEqual(rx.info?.tcp_port, 0)

        // The "iPad" also receives (as in the app), so the receiver learns it from its
        // unicast queries and can tell it "bye" at the end.
        var txConfig = HoppyConfig(id: "tx-" + UUID().uuidString.lowercased(), name: "iPad")
        txConfig.udp_port = 0
        txConfig.tcp_port = 0
        txConfig.receive_dir = try tempDir().path
        txConfig.receive = true
        txConfig.sweep = .hosts(["127.0.0.1:\(rxUdp)"])
        let tx = try HoppyNode(config: txConfig) { txEvents.add($0) }
        defer { tx.stop() }

        let peer = try XCTUnwrap(txEvents.wait("peer") { e -> HoppyPeer? in
            if case let .peers(list) = e { return list.first { $0.id == rxConfig.id } }
            return nil
        })
        XCTAssertEqual(peer.name, "Receiver")
        XCTAssertEqual(peer.addr, "127.0.0.1")

        let outbox = try tempDir()
        let file = outbox.appendingPathComponent("foto mare.jpg")
        try Data("hello hoppy".utf8).write(to: file)

        // Accepted.
        let transfer = tx.send([file], to: peer.id)
        XCTAssertNotEqual(transfer, 0)
        let offer = try XCTUnwrap(rxEvents.wait("offer") { e -> HoppyOffer? in
            if case let .offer(o) = e { return o }
            return nil
        })
        XCTAssertEqual(offer.fromName, "iPad")
        XCTAssertEqual(offer.fromPlatform, "ios")
        XCTAssertEqual(offer.files.first?.name, "foto mare.jpg")
        XCTAssertEqual(offer.total, 11)
        XCTAssertTrue(rx.decide(offer.id, accept: true))
        let saved = try XCTUnwrap(rxEvents.wait("received") { e -> [URL]? in
            if case let .received(ok, from, files, error) = e {
                XCTAssertTrue(ok, error ?? "")
                XCTAssertEqual(from, "iPad")
                return files
            }
            return nil
        })
        XCTAssertEqual(saved.first?.lastPathComponent, "foto mare.jpg")
        XCTAssertEqual(try Data(contentsOf: saved[0]), Data("hello hoppy".utf8))
        let outcome = txEvents.wait("delivered") { e -> HoppySendOutcome? in
            if case let .sendDone(id, outcome, _) = e, id == transfer { return outcome }
            return nil
        }
        XCTAssertEqual(outcome, .delivered)

        // Declined: nothing new is saved.
        let second = tx.send([file], to: peer.id)
        let offer2 = try XCTUnwrap(rxEvents.wait("second offer") { e -> HoppyOffer? in
            if case let .offer(o) = e, o.id != offer.id { return o }
            return nil
        })
        XCTAssertTrue(rx.decide(offer2.id, accept: false))
        let outcome2 = txEvents.wait("declined") { e -> HoppySendOutcome? in
            if case let .sendDone(id, outcome, _) = e, id == second { return outcome }
            return nil
        }
        XCTAssertEqual(outcome2, .declined)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: inbox.path).count, 1)

        // Receiving off: the sender's list empties (bye).
        let mark = txEvents.all.count
        let before = tx.peers.map { "\($0.name)@\($0.addr)" }
        XCTAssertEqual(rx.setReceiving(false), 0)
        let gone = txEvents.wait("bye", after: mark) { e -> Bool? in
            if case let .peers(list) = e, list.isEmpty { return true }
            return nil
        }
        if gone == nil {
            // Diagnostics for CI (logs are only read through annotations).
            XCTFail("bye: tx peers before \(before), now \(tx.peers.map { "\($0.name)@\($0.addr)" }); "
                    + "rx info \(String(describing: rx.info)); events after mark: "
                    + "\(txEvents.all.dropFirst(mark).map { "\($0)" }.prefix(5))")
        }
    }

    /// Protocol v1 through Swift (twin of security_events_trust_and_qr in
    /// crates/hoppy-ffi/src/tests.rs): the SECURITY JSON decodes, both screens get the same
    /// code, "Coincide", QR scan, forget, and the key persists in key_file.
    func testSecurityVerificationAndPersistentKey() throws {
        let data = try tempDir()
        let inbox = try tempDir()
        let rxEvents = Recorder(), txEvents = Recorder()

        var rxConfig = HoppyConfig(id: "rx-" + UUID().uuidString.lowercased(), name: "Desk")
        rxConfig.platform = "linux"
        rxConfig.receive_dir = inbox.path
        rxConfig.udp_port = 0
        rxConfig.tcp_port = 0
        rxConfig.receive = true
        rxConfig.sweep = .off
        rxConfig.key_file = data.appendingPathComponent("rx.key").path
        rxConfig.trust_file = data.appendingPathComponent("rx-trust.json").path
        var rx: HoppyNode? = try HoppyNode(config: rxConfig) { rxEvents.add($0) }
        let rxInfo = try XCTUnwrap(rx?.info)
        let rxFingerprint = try XCTUnwrap(rxInfo.fingerprint)
        XCTAssertNil(rxInfo.trust_warning)

        var txConfig = HoppyConfig(id: "tx-" + UUID().uuidString.lowercased(), name: "iPad")
        txConfig.udp_port = 0
        txConfig.tcp_port = 0
        txConfig.sweep = .hosts(["127.0.0.1:\(rxInfo.udp_port)"])
        txConfig.key_file = data.appendingPathComponent("tx.key").path
        txConfig.trust_file = data.appendingPathComponent("tx-trust.json").path
        let tx = try HoppyNode(config: txConfig) { txEvents.add($0) }
        defer { tx.stop() }

        let peer = try XCTUnwrap(txEvents.wait("peer") { e -> HoppyPeer? in
            if case let .peers(list) = e { return list.first { $0.id == rxConfig.id } }
            return nil
        })
        XCTAssertEqual(peer.encrypted, true)
        XCTAssertEqual(peer.trust, "new")

        let file = try tempDir().appendingPathComponent("a.txt")
        try Data("hi".utf8).write(to: file)
        let transfer = tx.send([file], to: peer.id)
        let offer = try XCTUnwrap(rxEvents.wait("offer") { e -> HoppyOffer? in
            if case let .offer(o) = e { return o }
            return nil
        })
        XCTAssertTrue(offer.security.encrypted)
        XCTAssertEqual(offer.security.trust, "new")
        XCTAssertTrue(offer.security.needsCheck)
        XCTAssertEqual(offer.security.nameMatchesKnown, false)
        let sas = try XCTUnwrap(offer.security.sas)
        XCTAssertEqual(sas.symbols.count, 4)
        XCTAssertEqual(sas.it.count, 4)
        XCTAssertEqual(sas.en.count, 4)
        XCTAssertEqual(sas.digits.count, 9) // "1234 5678"
        let session = try XCTUnwrap(txEvents.wait("send_session") { e -> HoppySecurity? in
            if case let .sendSession(id, sec) = e, id == transfer { return sec }
            return nil
        })
        XCTAssertEqual(session.sas, sas, "same code on both screens")
        XCTAssertEqual(session.peer?.fingerprint, rxFingerprint)
        XCTAssertTrue(rx?.decide(offer.id, accept: true) ?? false)
        XCTAssertEqual(txEvents.wait("delivered") { e -> HoppySendOutcome? in
            if case let .sendDone(id, outcome, _) = e, id == transfer { return outcome }
            return nil
        }, .delivered)

        // "Coincide" on the receiver.
        let txKey = try XCTUnwrap(offer.security.peer)
        XCTAssertTrue(rx?.verify(txKey) ?? false)
        XCTAssertEqual(rx?.trustList?.peers.first?.verified, true)
        XCTAssertEqual(rx?.trustList?.me.fingerprint, rxFingerprint)

        // The iPad scans the desk's QR code: both sides verified.
        let payload = try XCTUnwrap(rx?.qrBegin())
        XCTAssertTrue(payload.hasPrefix("hoppy:verify:1:\(rxConfig.id):"))
        let scan = tx.qrScan(payload, live: nil)
        XCTAssertNotEqual(scan, 0)
        let done = txEvents.wait("qr_done") { e -> Bool? in
            if case let .qrDone(id, ok, _, _, error) = e, id == scan {
                XCTAssertTrue(ok, error ?? "")
                return ok
            }
            return nil
        }
        XCTAssertEqual(done, true)
        let verified = rxEvents.wait("verified") { e -> HoppyPeerKey? in
            if case let .verified(key) = e { return key }
            return nil
        }
        XCTAssertEqual(verified?.id, txConfig.id)
        XCTAssertEqual(tx.qrScan("not a code", live: nil), 0)
        rx?.qrEnd()

        // Forget, then the key file survives a restart.
        XCTAssertTrue(tx.forget(rxConfig.id))
        XCTAssertFalse(tx.forget(rxConfig.id))
        rx?.stop()
        rx = nil
        let again = try HoppyNode(config: rxConfig) { _ in }
        defer { again.stop() }
        XCTAssertEqual(again.info?.fingerprint, rxFingerprint)
    }
}
