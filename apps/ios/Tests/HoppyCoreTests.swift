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
        XCTAssertEqual(rx.setReceiving(false), 0)
        _ = txEvents.wait("bye", after: mark) { e -> Bool? in
            if case let .peers(list) = e, list.isEmpty { return true }
            return nil
        }
    }
}
