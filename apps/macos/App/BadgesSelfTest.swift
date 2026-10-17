// SPDX-License-Identifier: Apache-2.0
//
// `voidfs badges-selftest <drive> [main|daemon-restart|refused] [--socket <path>] [--peer <path>]
// [--metrics <url>] [--files <n>] [--png <path>]`: Finder's badges end to end without Finder or
// FSKit. The Finder extension's model (`Badges/Badges.swift`) runs here against the agent's badge
// listener in process, which accepts this app's own signature (`refused` shows the Finder
// extension's requirement turning it away); a bridge session, as a mount's would, makes the
// edits. Each badge state is made on a fresh folder of the drive, and the check is what the
// model told "Finder" for each item.
//
// A conflict needs a change on the drive while this Mac's edit waits: `--peer` names the SDK's
// `mount_peer` example, run with the environment's `VOIDFS_ENDPOINT` and key. `--metrics` names
// the server's admin `/metrics`, to count the server's requests while the badges are read.

import Foundation

final class BadgesSelfTest: @unchecked Sendable {
    let scenario: BridgeScenario
    let model: BadgeModel
    let daemon: DaemonSocket
    let drive: String
    let mountpoint: String
    let say: (String) -> Void
    private let lock = NSLock()
    private var told: [String: String] = [:]
    private var tells = 0

    init(scenario: BridgeScenario, badges: BadgeClient, daemon: DaemonSocket, drive: String, say: @escaping (String) -> Void) {
        (self.scenario, self.daemon, self.drive, self.say) = (scenario, daemon, drive, say)
        mountpoint = "/voidfs-badges-selftest/\(drive)"
        var record: ((String, URL) -> Void)?
        model = BadgeModel(client: badges, mounts: [BadgeModel.Mount(drive: drive, mountpoint: mountpoint, readOnly: false, paused: false)],
                           setDirectories: { _ in }, setBadge: { record?($0, $1) })
        record = { [unowned self] identifier, url in lock.withLock { told[url.path] = identifier; tells += 1 } }
    }

    static func run(_ args: [String]) async -> Int32 {
        var rest = args
        func option(_ name: String) -> String? {
            guard let i = rest.firstIndex(of: name), i + 1 < rest.count else { return nil }
            defer { rest.removeSubrange(i...(i + 1)) }
            return rest[i + 1]
        }
        let socket = option("--socket") ?? Agent.stateDirectory?.appending(path: "daemon.sock").path
        let peer = option("--peer")
        let metrics = option("--metrics").flatMap(URL.init(string:))
        let files = option("--files").flatMap(Int.init) ?? 200
        let png = option("--png")
        guard let drive = rest.first, let socket else {
            print("usage: badges-selftest <drive> [main|daemon-restart|refused] [--socket <path>] [--peer <path>] [--metrics <url>] [--files <n>] [--png <path>]")
            return 2
        }
        let mode = rest.count > 1 ? rest[1] : "main"
        let say: (String) -> Void = { print($0); fflush(stdout) }
        say("badges self-test: drive \(drive), daemon socket \(socket), \(mode)")
        if let png, let sheet = BadgeArt.sheet() {
            do { try sheet.write(to: URL(filePath: png)); say("badge pictures: \(png)") } catch { say("badge pictures: \(error)") }
        }
        let bridge = NSXPCListener.anonymous()
        let bridgeDelegate = BridgeListener(socket: DaemonSocket(path: socket), requirement: SelfTest.ownRequirement)
        bridge.delegate = bridgeDelegate
        bridge.resume()
        let badges = NSXPCListener.anonymous()
        let badgeDelegate = BadgeListener(socket: DaemonSocket(path: socket), requirement: mode == "refused" ? Badges.peerRequirement : SelfTest.ownRequirement)
        badges.delegate = badgeDelegate
        badges.resume()
        defer { bridge.invalidate(); badges.invalidate() }
        let scenario = BridgeScenario(client: BridgeClient(endpoint: bridge.endpoint), drive: drive, another: { BridgeClient(endpoint: bridge.endpoint) }, say: say)
        let test = BadgesSelfTest(scenario: scenario, badges: BadgeClient(endpoint: badges.endpoint), daemon: DaemonSocket(path: socket), drive: drive, say: say)
        switch mode {
        case "main", "daemon-restart": await test.main(peer: peer, metrics: metrics, files: files, restart: mode == "daemon-restart")
        case "refused": await test.refused()
        default:
            say("unknown mode \(mode)")
            return 2
        }
        say("badge checks: \(scenario.checks), failures: \(scenario.failures)")
        return scenario.failures == 0 ? 0 : 1
    }

    // MARK: Helpers

    func check(_ name: String, _ ok: Bool, _ detail: @autoclosure () -> String = "") { scenario.check(name, ok, detail()) }

    func url(_ path: String) -> URL { URL(filePath: path.isEmpty ? mountpoint : "\(mountpoint)/\(path)") }

    func badgeTold(_ path: String) -> String? { lock.withLock { told[url(path).path] } }

    var tellCount: Int { lock.withLock { tells } }

    /// Waits until the model told Finder each item's expected badge. With `summary`, one check
    /// stands for them all.
    func expectBadges(_ phase: String, _ expected: [(String, String)], within: Duration = .seconds(30), summary: String? = nil) async {
        for (path, _) in expected { model.request(url(path)) }
        let deadline = ContinuousClock.now + within
        var wrong = expected
        while ContinuousClock.now < deadline {
            wrong = expected.filter { badgeTold($0.0) != $0.1 }
            if wrong.isEmpty { break }
            try? await Task.sleep(for: .milliseconds(100))
        }
        if let summary {
            check("\(phase): \(summary)", wrong.isEmpty, "\(wrong.count) not, as \(wrong.prefix(3).map { "\($0.0): \(badgeTold($0.0) ?? "nothing yet")" })")
            return
        }
        for (path, badge) in expected {
            let got = badgeTold(path)
            check("\(phase): \(path.isEmpty ? "the drive's top" : path) is badged \(badge.isEmpty ? "nothing" : badge)", got == badge, "told \(got.map { $0.isEmpty ? "nothing" : $0 } ?? "nothing yet")")
        }
    }

    func json(_ method: String, _ target: String, _ body: [String: Any]? = nil) throws -> Any {
        let data = try body.map { try JSONSerialization.data(withJSONObject: $0) } ?? Data()
        let r = try daemon.requestSync(method, target, generation: nil, contentType: body == nil ? nil : "application/json", body: data, limit: 16 << 20)
        guard (200..<300).contains(r.status) else { throw NSError(domain: "dev.voidfs", code: r.status, userInfo: [NSLocalizedDescriptionKey: String(decoding: r.body, as: UTF8.self)]) }
        return try JSONSerialization.jsonObject(with: r.body)
    }

    func write(_ s: BridgeSession, _ ino: UInt64, _ text: String) async throws {
        let (fh, _) = try await again { try await s.open(ino, write: true) }
        try await s.truncate(fh, size: 0)
        _ = try await s.write(fh, offset: 0, data: Data(text.utf8))
        try await s.close(fh)
    }

    /// As FSKit does, a call the core answers `EAGAIN` (a listing raced a change) is made again.
    func again<T>(_ body: () async throws -> T) async throws -> T {
        for _ in 0..<20 {
            do { return try await body() } catch where Bridge.errno(error) == EAGAIN { try await Task.sleep(for: .milliseconds(50)) }
        }
        return try await body()
    }

    func file(_ s: BridgeSession, _ parent: UInt64, _ name: String, _ text: String?) async throws -> UInt64 {
        let a = try await again { try await s.create(parent, name, mode: 0o644) }
        if let text { try await write(s, a.ino, text) }
        return a.ino
    }

    func waitSaved(_ s: BridgeSession, _ inodes: [UInt64], within: Duration = .seconds(60)) async throws -> Bool {
        let deadline = ContinuousClock.now + within
        while ContinuousClock.now < deadline {
            var all = true
            for ino in inodes where try await s.getattrUncached(ino).sync != .saved { all = false; break }
            if all { return true }
            try await Task.sleep(for: .milliseconds(100))
        }
        return false
    }

    /// The server's requests so far, by operation.
    func serverRequests(_ metrics: URL) async -> [String: Int]? {
        guard let (data, _) = try? await URLSession.shared.data(from: metrics) else { return nil }
        var by: [String: Int] = [:]
        for line in String(decoding: data, as: UTF8.self).split(separator: "\n") where line.hasPrefix("voidfs_s3_requests_total{") {
            guard let op = line.split(separator: "\"").dropFirst().first, let value = line.split(separator: " ").last.flatMap({ Double($0) }) else { continue }
            by[String(op), default: 0] += Int(value)
        }
        return by
    }

    static func difference(_ after: [String: Int], _ before: [String: Int]) -> [String: Int] {
        after.reduce(into: [:]) { out, kv in let d = kv.value - before[kv.key, default: 0]; if d != 0 { out[kv.key] = d } }
    }

    static func describe(_ by: [String: Int]) -> String {
        let total = by.values.reduce(0, +)
        return by.isEmpty ? "0" : "\(total) (" + by.sorted { $0.key < $1.key }.map { "\($0.key) \($0.value)" }.joined(separator: ", ") + ")"
    }

    // MARK: Scenarios

    func main(peer: String?, metrics: URL?, files: Int, restart: Bool) async {
        model.start()
        defer { model.stop() }
        do {
            var s = try await scenario.client.open(drive: drive, readOnly: false)
            let root = s.info.root
            let dir = "badges-\(Int(Date().timeIntervalSince1970 * 1000))"
            say("folder \(dir)")
            let folder = try await again { try await s.create(root, dir, mode: 0o755, directory: true) }.ino
            let saved = try await file(s, folder, "saved.txt", "saved\n")
            let conflict = try await file(s, folder, "conflict.txt", "base\n")
            check("the first files are saved", try await waitSaved(s, [folder, saved, conflict]))

            // Nothing in the folder waits: every item is saved, and only Finder's asking costs a call.
            await expectBadges("saved", [(dir, "saved"), ("\(dir)/saved.txt", "saved"), ("\(dir)/conflict.txt", "saved")])

            // Held: the drive's uploads are paused, so this Mac's edits wait.
            _ = try json("POST", "/v1/uploads/pause", ["drive": drive])
            try await write(s, conflict, "mine\n")
            _ = try await file(s, folder, "saving.txt", "saving\n")
            _ = try await file(s, folder, "pending.txt", nil)
            _ = try await file(s, folder, "kept.txt", "kept\n")
            let nested = try await again { try await s.create(folder, "nested", mode: 0o755, directory: true) }.ino
            _ = try await file(s, nested, "inner.txt", nil)
            // A cancelled entry is kept: the local copy keeps it, not uploaded, until a later save.
            let uploads = try json("GET", "/v1/uploads") as? [String: Any]
            let entries = (uploads?["items"] as? [[String: Any]] ?? []).filter { $0["key"] as? String == "\(dir)/kept.txt" && ["queued", "uploading"].contains($0["state"] as? String ?? "") }
            for e in entries { _ = try json("POST", "/v1/uploads/cancel", ["entry": e["id"] as Any]) }
            let ids = Set(entries.compactMap { $0["id"] as? Int })
            let after = (try json("GET", "/v1/uploads") as? [String: Any])?["items"] as? [[String: Any]] ?? []
            let states = Set(after.filter { ids.contains($0["id"] as? Int ?? -1) }.compactMap { $0["state"] as? String })
            check("kept.txt's \(entries.count) entries are cancelled, and kept on this Mac", !entries.isEmpty && states == ["kept"], "\(states)")
            // Someone else saves conflict.txt meanwhile: this Mac's guarded save will be refused.
            var theirs = false
            if let peer {
                let p = Process()
                p.executableURL = URL(filePath: peer)
                p.arguments = [drive, "put", "\(dir)/conflict.txt", "theirs\n"]
                try p.run()
                p.waitUntilExit()
                theirs = p.terminationStatus == 0
                check("the peer saved conflict.txt on the drive", theirs, "exit \(p.terminationStatus)")
            } else {
                say("no --peer: conflict.txt can't conflict, and stays saving until the resume saves it")
            }
            await expectBadges("paused", [
                (dir, "kept"), ("\(dir)/saved.txt", "saved"), ("\(dir)/saving.txt", "saving"), ("\(dir)/pending.txt", "pending"),
                ("\(dir)/kept.txt", "kept"), ("\(dir)/conflict.txt", "saving"), ("\(dir)/nested", "pending"), ("\(dir)/nested/inner.txt", "pending"),
            ])
            model.observe(url(dir))
            model.observe(url("\(dir)/nested"))

            // Released: everything saves but the kept file, and the drive refuses conflict.txt.
            _ = try json("POST", "/v1/uploads/resume", ["drive": drive])
            await expectBadges("resumed", [
                (dir, theirs ? "conflict" : "kept"), ("\(dir)/saved.txt", "saved"), ("\(dir)/saving.txt", "saved"), ("\(dir)/pending.txt", "saved"),
                ("\(dir)/kept.txt", "kept"), ("\(dir)/conflict.txt", theirs ? "conflict" : "saved"), ("\(dir)/nested", "saved"), ("\(dir)/nested/inner.txt", "saved"),
            ], within: .seconds(60))

            // Unknown: a folder that isn't there.
            await expectBadges("unknown", [("\(dir)/missing/a.txt", "")])

            // Many files: what reading their badges costs, against the same time without it.
            let many = try await again { try await s.create(folder, "many", mode: 0o755, directory: true) }.ino
            var inodes: [UInt64] = []
            for i in 0..<files { inodes.append(try await file(s, many, String(format: "f%04d.txt", i), "\(i)\n")) }
            check("\(files) files are saved", try await waitSaved(s, [many] + inodes, within: .seconds(180)))
            let manyPaths = (0..<files).map { String(format: "\(dir)/many/f%04d.txt", $0) }
            await expectBadges("many", [("\(dir)/many", "saved")])
            await expectBadges("many", manyPaths.map { ($0, "saved") }, summary: "all \(files) files are badged saved")
            if let metrics {
                let window: Duration = .seconds(10)
                model.stop()
                try await Task.sleep(for: .seconds(2))
                guard let b0 = await serverRequests(metrics) else { check("the server's metrics", false, "\(metrics) didn't answer"); return }
                try await Task.sleep(for: window)
                let b1 = await serverRequests(metrics) ?? [:]
                let baseline = Self.difference(b1, b0)
                model.start()
                model.observe(url("\(dir)/many"))
                let (c0, u0) = model.counts
                let t0 = tellCount
                for path in manyPaths { model.request(url(path)) }
                try await Task.sleep(for: window)
                let b2 = await serverRequests(metrics) ?? [:]
                let polled = Self.difference(b2, b1)
                let (c1, u1) = model.counts
                let folders = model.polledFolders
                let seconds = Int(window.components.seconds)
                say("server requests in \(window) idle: \(Self.describe(baseline)); in \(window) reading badges: \(Self.describe(polled))")
                say("badge calls in that time: \(c1 - c0) for \(folders) folders read each second, \(u1 - u0) of them answered unchanged; \(files) items asked about, \(tellCount - t0) badges told")
                // The feed's long polls and the link probe run either way; a badge read adds none.
                check("reading \(files) files' badges adds no server request", polled.values.reduce(0, +) <= baseline.values.reduce(0, +) + 2, "\(Self.describe(polled)) against \(Self.describe(baseline))")
                check("the calls are per folder and second, not per file", c1 - c0 <= folders * (seconds + 2), "\(c1 - c0) calls for \(folders) folders")
                check("an unchanged folder answers unchanged", u1 - u0 >= (c1 - c0) * 3 / 4, "\(u1 - u0) of \(c1 - c0)")
            }

            // The agent's own checks: a pause only for a drive mounted now, and well-formed arguments.
            await scenario.expect("a pause from Finder for a drive that isn't mounted", ENOENT, code: "NotMounted") { try await self.model.client.pause(drive: self.drive) }
            await scenario.expect("a drive name with a slash", EINVAL, code: "InvalidArgument") { _ = try await self.model.client.states(drive: "a/b", folder: "", after: nil, since: nil) }
            await scenario.expect("a folder with ..", EINVAL, code: "InvalidArgument") { _ = try await self.model.client.states(drive: self.drive, folder: "\(dir)/../x", after: nil, since: nil) }
            await scenario.expect("a cursor that isn't a name", EINVAL, code: "InvalidArgument") { _ = try await self.model.client.states(drive: self.drive, folder: dir, after: "a/b", since: nil) }
            await scenario.expect("a file as the folder", ENOTDIR, code: "NotDir") { _ = try await self.model.client.states(drive: self.drive, folder: "\(dir)/saved.txt", after: nil, since: nil) }
            await scenario.expect("a drive not open here", ENOENT, code: "NotOpen") { _ = try await self.model.client.states(drive: "no-such-drive-\(dir)", folder: "", after: nil, since: nil) }

            if restart {
                // The daemon restarts: its badges go while no daemon answers, and come back from its
                // journal once the drive is open again.
                say("ready: restart the daemon now (session generation \(s.info.generation))")
                let deadline = ContinuousClock.now + .seconds(90)
                var gone = false
                while ContinuousClock.now < deadline, !gone {
                    gone = badgeTold("\(dir)/saved.txt") == ""
                    if !gone { try await Task.sleep(for: .milliseconds(100)) }
                }
                check("the badges go while no daemon answers", gone)
                var fresh: BridgeSession?
                while fresh == nil, ContinuousClock.now < deadline {
                    if let o = try? await scenario.client.open(drive: drive, readOnly: false), o.info.generation > s.info.generation { fresh = o } else { try await Task.sleep(for: .milliseconds(200)) }
                }
                guard let fresh else { check("a new session after the restart", false); return }
                s = fresh
                await expectBadges("restarted", [
                    (dir, theirs ? "conflict" : "kept"), ("\(dir)/saved.txt", "saved"), ("\(dir)/saving.txt", "saved"),
                    ("\(dir)/kept.txt", "kept"), ("\(dir)/conflict.txt", theirs ? "conflict" : "saved"), ("\(dir)/nested", "saved"),
                ], within: .seconds(60))
            }
            say("left on the drive: \(dir) (its kept and conflict files keep their states until the next save or until someone resolves them)")
            try await s.release()
        } catch {
            check("the badge run", false, "\(error)")
        }
    }

    func refused() async {
        let mounts = await scenario.errno { _ = try await self.model.client.mounts() }
        check("a peer without the Finder extension's signature can't list the mounts", mounts != 0, "it answered")
        let states = await scenario.errno { _ = try await self.model.client.states(drive: self.drive, folder: "", after: nil, since: nil) }
        check("nor read a folder's states", states != 0, "it answered")
        let pause = await scenario.errno { try await self.model.client.pause(drive: self.drive) }
        check("nor pause a drive", pause != 0, "it answered")
        say("refused with \(BridgeScenario.name(mounts)), \(BridgeScenario.name(states)), \(BridgeScenario.name(pause))")
    }
}
