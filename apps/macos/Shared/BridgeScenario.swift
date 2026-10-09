// SPDX-License-Identifier: Apache-2.0
//
// Checks of the bridge, end to end through the daemon, with timings. The app's `bridge-selftest`
// runs them over an in-process XPC listener; the extension runs them through the launch agent
// when a mount looks up `.voidfs-bridge` (spike-only instrumentation, removed with item 3).
// Each line is logged as it happens; a failed check says FAIL.

import Foundation

final class BridgeScenario: @unchecked Sendable {
    let client: BridgeClient
    let drive: String
    let say: (String) -> Void
    /// Another connection to the same agent, as a second extension would have.
    let another: () -> BridgeClient
    private(set) var failures = 0
    private(set) var checks = 0

    init(client: BridgeClient, drive: String, another: @escaping () -> BridgeClient, say: @escaping (String) -> Void) {
        (self.client, self.drive, self.another, self.say) = (client, drive, another, say)
    }

    func check(_ name: String, _ ok: Bool, _ detail: @autoclosure () -> String = "") {
        checks += 1
        if !ok { failures += 1 }
        say("\(ok ? "ok  " : "FAIL") \(name)\(ok ? "" : ": " + detail())")
    }

    /// The errno a call fails with, or 0.
    func errno(_ body: () async throws -> Void) async -> Int32 {
        do { try await body(); return 0 } catch { return Bridge.errno(error) }
    }

    func expect(_ name: String, _ expected: Int32, _ body: () async throws -> Void) async {
        let got = await errno(body)
        check("\(name) → \(Self.name(expected))", got == expected, "got \(Self.name(got))")
    }

    /// As `expect`, and the refusal is the one `code` names: the agent's own, say, not the
    /// daemon's.
    func expect(_ name: String, _ expected: Int32, code: String, _ body: () async throws -> Void) async {
        var got: (Int32, String?) = (0, nil)
        do { try await body() } catch { got = (Bridge.errno(error), (error as NSError).userInfo["code"] as? String) }
        check("\(name) → \(Self.name(expected)), \(code)", got.0 == expected && got.1 == code, "got \(Self.name(got.0)), \(got.1 ?? "no code")")
    }

    static func name(_ e: Int32) -> String { e == 0 ? "ok" : "\(String(cString: strerror(e))) (\(e))" }

    static func micros(_ d: Duration) -> Double { Double(d.components.seconds) * 1e6 + Double(d.components.attoseconds) / 1e12 }

    func distribution(_ label: String, _ samples: [Double]) {
        let s = samples.sorted()
        guard !s.isEmpty else { return }
        let mean = s.reduce(0, +) / Double(s.count)
        say(String(format: "time %@: n=%d mean %.1f µs, p50 %.1f µs, p90 %.1f µs, p99 %.1f µs, max %.1f µs", label, s.count, mean, s[s.count / 2], s[s.count * 9 / 10], s[s.count * 99 / 100], s.last!))
    }

    func timed(_ n: Int, _ body: () async throws -> Void) async rethrows -> [Double] {
        var samples: [Double] = []
        samples.reserveCapacity(n)
        for _ in 0..<n {
            let start = ContinuousClock.now
            try await body()
            samples.append(Self.micros(ContinuousClock.now - start))
        }
        return samples
    }

    static func pattern(_ count: Int, seed: UInt8) -> Data {
        var d = Data(count: count)
        d.withUnsafeMutableBytes { raw in for i in 0..<count { raw[i] = UInt8(truncatingIfNeeded: i &* 31 &+ Int(seed)) } }
        return d
    }

    /// Reads a whole file in `maxIo` pieces.
    func readAll(_ s: BridgeSession, _ fh: UInt64, size: UInt64) async throws -> Data {
        var out = Data()
        while UInt64(out.count) < size {
            let piece = try await s.read(fh, offset: UInt64(out.count), length: min(s.info.maxIo, size - UInt64(out.count)))
            if piece.isEmpty { break }
            out.append(piece)
        }
        return out
    }

    // MARK: The main run

    /// `seeded` names a file of the drive's root to time cache-hit reads on (the socket
    /// benchmark used a 4 KiB object).
    func main(iterations: Int = 1000, seeded: String = "data") async {
        let started = ContinuousClock.now
        do {
            let payload = Data(count: 4096)
            distribution("XPC ping 4 KiB", try await timed(500) { _ = try await client.ping(payload) })

            let s = try await client.open(drive: drive, readOnly: false)
            check("session on \(drive): generation \(s.info.generation), root \(s.info.root), maxIo \(s.info.maxIo)", s.info.generation > 0 && s.info.maxIo == Bridge.maxIo)
            check("watch relays the initial resync", s.isWatching && s.generation >= s.info.metadataGeneration, "watching \(s.isWatching), generation \(s.generation)")
            let caps = (try? JSONSerialization.jsonObject(with: s.info.capabilities) as? [String: Any]) ?? [:]
            check("capabilities arrive: no hard links, exchange or cloning; xattrs 65536", caps["hardLinks"] as? Bool == false && caps["exchange"] as? Bool == false && caps["clone"] as? Bool == false && caps["maxXattrBytes"] as? Int == 65536, "\(caps)")
            let root = s.info.root

            // Warm metadata and cache-hit reads, as the socket benchmark measured them.
            if let data = try? await s.lookup(root, seeded) {
                _ = try await s.getattrUncached(data.ino)
                distribution("warm getattr through the bridge", try await timed(iterations) { _ = try await s.getattrUncached(data.ino) })
                distribution("warm getattr from the memo", try await timed(iterations) { _ = try await s.getattr(data.ino) })
                distribution("warm lookup from the memo", try await timed(iterations) { _ = try await s.lookup(root, seeded) })
                let (fh, _) = try await s.open(data.ino, write: false)
                let length = min(UInt64(4096), data.size)
                let first = try await s.read(fh, offset: 0, length: length)
                check("\(seeded): read \(first.count) bytes", UInt64(first.count) == length)
                distribution("cache-hit \(length) B read through the bridge", try await timed(iterations) {
                    let again = try await s.read(fh, offset: 0, length: length)
                    if again != first { throw Bridge.error(EIO, "Mismatch", "a cache-hit read changed") }
                })
                try await s.close(fh)
            } else {
                say("note: no \(seeded) in the drive's root; cache-hit and warm metadata timings skipped")
            }

            // More calls at once than the agent admits (32 a connection): what exceeds it, when
            // calls arrive faster than they finish, is refused with EAGAIN, untouched, and the
            // client tries it again.
            let retried = s.retries
            let together = await withTaskGroup(of: Bool.self) { group in
                for _ in 0..<256 { group.addTask { (try? await s.getattrUncached(root).ino) == root } }
                return await group.reduce(into: 0) { n, ok in if ok { n += 1 } }
            }
            check("256 calls at once all answer, \(s.retries - retried) after EAGAIN", together == 256, "\(together) answered, \(s.retries - retried) retried")

            // Bounded binary writes and reads, and a generation update for them.
            let name = "bridge-\(Int(Date().timeIntervalSince1970 * 1000)).bin"
            let before = s.generation
            let file = try await s.create(root, name, mode: 0o644)
            check("create \(name)", file.kind == .file && file.size == 0 && file.mode == 0o644)
            let (wh, _) = try await s.open(file.ino, write: true)
            var offset: UInt64 = 0
            var written = Data()
            for (size, seed) in [(4096, UInt8(1)), (1 << 20, 2), (Int(s.info.maxIo), 3)] {
                let piece = Self.pattern(size, seed: seed)
                let t0 = ContinuousClock.now
                let n = try await s.write(wh, offset: offset, data: piece)
                let elapsed = Self.micros(ContinuousClock.now - t0)
                check(String(format: "write %d bytes at %llu (%.0f µs)", size, offset, elapsed), n == size, "wrote \(n)")
                offset += UInt64(n)
                written.append(piece)
            }
            await expect("a write over maxIo is refused by the agent", EINVAL, code: "InvalidArgument") { _ = try await s.write(wh, offset: offset, data: Data(count: Int(s.info.maxIo) + 1)) }
            await expect("a read over maxIo is refused by the agent", EINVAL, code: "InvalidArgument") { _ = try await s.read(wh, offset: 0, length: s.info.maxIo + 1) }
            let t0 = ContinuousClock.now
            let back = try await readAll(s, wh, size: offset)
            check(String(format: "read back %d bytes byte for byte (%.0f µs)", back.count, Self.micros(ContinuousClock.now - t0)), back == written)
            check("handle size \(offset)", try await s.handleAttr(wh).size == offset)
            try await s.fsync(wh)
            try await s.close(wh)
            let t1 = ContinuousClock.now
            let g = try await s.waitForGeneration(above: before, timeout: .seconds(10))
            check(String(format: "generation update %llu → %llu after the writes (%.0f µs after close)", before, g, Self.micros(ContinuousClock.now - t1)), g > before)
            let after = try await s.getattr(file.ino)
            check("getattr after close sees \(offset) bytes", after.size == offset, "size \(after.size)")

            // Namespace, attributes and xattrs, with the memo following this client's changes.
            let dir = try await s.create(root, name + ".d", mode: 0o755, directory: true)
            check("mkdir", dir.kind == .folder && dir.mode == 0o755)
            try await s.rename(root, name, dir.ino, "moved")
            check("rename keeps the inode", try await s.lookup(dir.ino, "moved").ino == file.ino)
            await expect("lookup of the old name", ENOENT) { _ = try await s.lookup(root, name) }
            let other = try await s.create(dir.ino, "other", mode: 0o600)
            await expect("exclusive rename onto a name", EEXIST) { try await s.rename(dir.ino, "moved", dir.ino, "other", how: 1) }
            await expect("swap rename", ENOTSUP) { try await s.rename(dir.ino, "moved", dir.ino, "other", how: 2) }
            await expect("hard link", ENOTSUP) { _ = try await s.link(file.ino, dir.ino, "twin") }
            await expect("clone", ENOTSUP) { _ = try await s.link(file.ino, dir.ino, "copy", clone: true) }
            await expect("a name with a slash", EINVAL) { _ = try await s.create(dir.ino, "a/b", mode: 0o644) }
            // With the relay held back, only the client's own invalidation can update the memo.
            client.holdEvents()
            let memoHits = s.memoHits
            for _ in 0..<50 where s.memoHits == memoHits { _ = try await s.getattr(file.ino) }
            let changed = try await s.setattr(file.ino, mode: 0o600, mtime: (-1_500_000, 123_456_000))
            check("setattr mode and a pre-1970 time", changed.mode == 0o600 && changed.mtimeSeconds == -1_500_000 && changed.mtimeNanoseconds == 123_456_000, "\(changed.mode) \(changed.mtimeSeconds).\(changed.mtimeNanoseconds)")
            let held = try await s.getattr(file.ino).mode
            client.releaseEvents()
            check("the memo follows this client's setattr before any event", s.memoHits > memoHits && held == 0o600, "mode \(String(held, radix: 8)), memo hits \(s.memoHits - memoHits)")
            // Another connection's change reaches this one's memo through the daemon's relay.
            let peer = try await another().open(drive: drive, readOnly: false, watch: false)
            // A reply isn't kept if an invalidation (of this run's own publications, say) arrived
            // while it was in flight, so this asks until one sticks.
            let hits = s.memoHits
            for _ in 0..<50 where s.memoHits == hits {
                _ = try await s.getattr(file.ino)
                if s.memoHits == hits { try await Task.sleep(for: .milliseconds(20)) }
            }
            check("a repeated getattr comes from the memo", s.memoHits > hits)
            _ = try await peer.setattr(file.ino, mode: 0o640)
            var seen = try await s.getattr(file.ino).mode
            let until = ContinuousClock.now + .seconds(10)
            while seen != 0o640, ContinuousClock.now < until {
                _ = try? await s.waitForGeneration(above: s.generation, timeout: .seconds(1))
                seen = try await s.getattr(file.ino).mode
            }
            check("another connection's setattr reaches the memo through the relay", seen == 0o640, String(seen, radix: 8))
            try await peer.release()
            let value = Data([0, 0xff, 0x80, 0x62, 0x69, 0x6e, 0])
            let xname = "user.bridge é&=%"
            await expect("setxattr under a name needing escapes", 0) { try await s.setxattr(file.ino, xname, value, how: 1) }
            try await s.setxattr(file.ino, "empty", Data())
            check("getxattr returns the raw bytes", try await s.getxattr(file.ino, xname) == value)
            check("an empty value", try await s.getxattr(file.ino, "empty").isEmpty)
            check("listxattr", try await s.listxattr(file.ino) == ["empty", xname])
            await expect("create an existing xattr", EEXIST) { try await s.setxattr(file.ino, xname, value, how: 1) }
            await expect("replace a missing xattr", ENOATTR) { try await s.setxattr(file.ino, "absent", value, how: 2) }
            await expect("an xattr over 64 KiB", E2BIG) { try await s.setxattr(file.ino, "big", Data(count: Bridge.maxXattr + 1)) }
            try await s.removexattr(file.ino, "empty")
            await expect("getxattr after remove", ENOATTR) { _ = try await s.getxattr(file.ino, "empty") }
            let page = try await s.readdir(dir.ino, after: nil, limit: 16)
            check("readdir", page.names == ["moved", "other"], "\(page.names)")
            await expect("a page of 0", EINVAL) { _ = try await s.readdir(dir.ino, after: nil, limit: 0) }
            await expect("rmdir of a full folder", ENOTEMPTY) { try await s.remove(root, name + ".d", directory: true) }
            try await s.remove(dir.ino, "moved")
            try await s.remove(dir.ino, "other")
            _ = other
            try await s.remove(root, name + ".d", directory: true)
            await expect("lookup after rmdir", ENOENT) { _ = try await s.lookup(root, name + ".d") }

            // A read-only session of the same connection.
            let ro = try await client.open(drive: drive, readOnly: true, watch: false)
            await expect("read-only create", EROFS) { _ = try await ro.create(root, "nope", mode: 0o644) }
            await expect("read-only setxattr", EROFS) { try await ro.setxattr(root, "user.x", Data()) }
            try await ro.release()
            await expect("a released session", ESTALE) { _ = try await ro.getattr(root) }

            say("memo hits \(s.memoHits), events \(s.events), generation \(s.generation), EAGAIN retries \(s.retries)")
            try await s.release()
            await expect("calls after release", ESTALE) { _ = try await s.getattrUncached(root) }
        } catch {
            check("the run", false, "\(error)")
        }
        say(String(format: "bridge checks: %d, failures: %d, %.1f s", checks, failures, Self.micros(ContinuousClock.now - started) / 1e6))
    }

    // MARK: Restarts

    /// Writes and fsyncs a file, says `ready`, then waits for the session to go stale: the
    /// caller restarts the daemon (or the agent) meanwhile. Then it opens a new session and
    /// checks what survived.
    func restart(_ what: String, wait: Duration = .seconds(90), trigger: (() async -> Void)? = nil) async {
        do {
            let s = try await client.open(drive: drive, readOnly: false)
            let root = s.info.root
            let name = "restart-\(what)-\(Int(Date().timeIntervalSince1970 * 1000)).txt"
            let file = try await s.create(root, name, mode: 0o644)
            let (fh, _) = try await s.open(file.ino, write: true)
            let bytes = Data("acknowledged before the \(what) restart\n".utf8)
            _ = try await s.write(fh, offset: 0, data: bytes)
            try await s.fsync(fh)
            say("ready: restart the \(what) now (session generation \(s.info.generation), file \(name), handle \(fh))")
            if let trigger { await trigger() }
            var seen: [Int32] = []
            let deadline = ContinuousClock.now + wait
            var stale = false
            while ContinuousClock.now < deadline {
                let e = await errno { _ = try await s.getattrUncached(root) }
                if e != 0, seen.last != e { seen.append(e); say("old session: \(Self.name(e))") }
                if e == ESTALE { stale = true; break }
                try await Task.sleep(for: .milliseconds(100))
            }
            check("the old session goes stale after the \(what) restart", stale, "saw \(seen.map(Self.name))")
            check("its relay ended", !s.isWatching)
            var fresh: BridgeSession?
            var lastError: Error?
            while fresh == nil, ContinuousClock.now < deadline {
                do { fresh = try await client.open(drive: drive, readOnly: false) } catch { lastError = error; try await Task.sleep(for: .milliseconds(200)) }
            }
            guard let fresh else { check("a new session", false, "\(String(describing: lastError))"); return }
            check("a new session (generation \(fresh.info.generation), was \(s.info.generation))", what == "daemon" ? fresh.info.generation > s.info.generation : fresh.info.generation >= s.info.generation)
            let found = try await fresh.lookup(fresh.info.root, name)
            check("the inode survives (\(found.ino))", found.ino == file.ino)
            let (rh, _) = try await fresh.open(found.ino, write: false)
            check("acknowledged bytes survive", try await fresh.read(rh, offset: 0, length: 4096) == bytes)
            try await fresh.close(rh)
            var published = false
            let publishBy = ContinuousClock.now + .seconds(30)
            while !published, ContinuousClock.now < publishBy {
                published = try await fresh.getattrUncached(found.ino).sync == .saved
                if !published { try await Task.sleep(for: .milliseconds(100)) }
            }
            check("the journal publishes it after the \(what) restart", published)
            if what == "daemon" { await expect("the old handle on the new session", ESTALE) { _ = try await fresh.read(fh, offset: 0, length: 1) } }
            await expect("the old session", ESTALE) { _ = try await s.read(fh, offset: 0, length: 1) }
            try await fresh.remove(fresh.info.root, name)
            try await fresh.release()
        } catch {
            check("the \(what) restart run", false, "\(error)")
        }
        say("restart checks: \(checks), failures: \(failures)")
    }

    /// Waits for an outside change to `key` (an upload with the CLI, say) to reach this
    /// connection as an invalidation.
    func remote(key: String, wait: Duration = .seconds(60)) async {
        do {
            let s = try await client.open(drive: drive, readOnly: true)
            let initial = try? await s.lookup(s.info.root, key)
            say("ready: change \(key) from outside now (generation \(s.generation), version \(initial?.versionId ?? "none"))")
            let start = ContinuousClock.now
            var g = s.generation
            var found = false
            while !found, ContinuousClock.now - start < wait {
                g = try await s.waitForGeneration(above: g, timeout: wait)
                if let a = try? await s.lookup(s.info.root, key), a.versionId != initial?.versionId {
                    found = true
                    say("\(key): inode \(a.ino), size \(a.size), version \(a.versionId ?? "-")")
                }
            }
            check(String(format: "an outside change arrives as generation %llu (%.1f s after ready)", g, Self.micros(ContinuousClock.now - start) / 1e6), found)
            try await s.release()
        } catch {
            check("the remote run", false, "\(error)")
        }
    }
}
