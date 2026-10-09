// SPDX-License-Identifier: Apache-2.0
//
// The extension's side of the bridge: one XPC connection to the agent, the sessions opened on
// it, and each session's metadata memo. The memo answers repeated `getattr` and `lookup` calls
// without an agent hop (about 60 µs from the sandbox, against about 17 µs per FSKit upcall). It is
// tagged by the daemon's watch: an entry is kept only while the session's invalidation relay is
// live, is dropped when an event names its inode (or its parent, for a name), and everything goes
// on a resync or when the relay or the connection ends. A reply is kept only if no event arrived
// while it was in flight, and this client's own changes drop what they touch before returning.

import Foundation

final class BridgeClient: NSObject, VoidfsBridgeEvents, @unchecked Sendable {
    private let lock = NSLock()
    private let make: () -> NSXPCConnection
    private var connection: NSXPCConnection?
    /// Counts connections: a session belongs to the one it was opened on, since the agent's
    /// session numbers mean nothing on another.
    private(set) var epoch = 0
    private var sessions: [Int64: BridgeSession] = [:]
    /// Instrumentation for the checks: while set, invalidations queue instead of applying, so a
    /// check can tell the memo's own invalidation from the relay's.
    private var held: [(Int64, BridgeEvent)]?

    /// The agent's Mach service, as the extension reaches it.
    convenience init(service: String = Bridge.service) {
        self.init { NSXPCConnection(machServiceName: service) }
    }

    /// An in-process listener, for the app's self-test.
    convenience init(endpoint: NSXPCListenerEndpoint) {
        self.init { NSXPCConnection(listenerEndpoint: endpoint) }
    }

    init(make: @escaping () -> NSXPCConnection) { self.make = make }

    deinit { connection?.invalidate() }

    private func current() -> (NSXPCConnection, Int) {
        lock.lock(); defer { lock.unlock() }
        if let connection { return (connection, epoch) }
        let c = make()
        c.remoteObjectInterface = Bridge.interface()
        c.exportedInterface = Bridge.eventsInterface()
        c.exportedObject = self
        epoch += 1
        let mine = epoch
        // Interrupted (the agent exited; launchd starts it again on the next message) or
        // invalidated: either way the agent forgot this connection's sessions.
        c.interruptionHandler = { [weak self] in self?.lost(mine) }
        c.invalidationHandler = { [weak self] in self?.lost(mine) }
        c.resume()
        connection = c
        return (c, mine)
    }

    private func lost(_ which: Int) {
        let dead: [BridgeSession] = lock.withLock {
            guard which == epoch else { return [] }
            connection?.invalidationHandler = nil
            connection?.interruptionHandler = nil
            connection?.invalidate()
            connection = nil
            defer { sessions.removeAll() }
            return Array(sessions.values)
        }
        for s in dead { s.lost() }
    }

    /// Drops the connection, as if the agent had gone: the next call makes a new one.
    func reset() { lost(lock.withLock { epoch }) }

    static func result<T>(_ value: T?, _ error: NSError?) -> Result<T, Error> {
        if let error { return .failure(error) }
        if let value { return .success(value) }
        return .failure(Bridge.error(EIO, "BadResponse", "the bridge answered nothing"))
    }

    /// One call, with exactly one of its reply and the connection's error handler answering.
    func send<T>(epoch expected: Int? = nil, _ call: (VoidfsBridgeProtocol, @escaping (Result<T, Error>) -> Void) -> Void) async throws -> T {
        let (c, now) = current()
        if let expected, expected != now { throw Bridge.error(ESTALE, "StaleSession", "the bridge connection was lost: open another session") }
        return try await withCheckedThrowingContinuation { continuation in
            let proxy = c.remoteObjectProxyWithErrorHandler { continuation.resume(throwing: $0) } as! VoidfsBridgeProtocol
            call(proxy) { continuation.resume(with: $0) }
        }
    }

    func ping(_ payload: Data) async throws -> Data {
        try await send { p, done in p.ping(payload) { done(.success($0)) } }
    }

    /// Opens a session on `drive`; with `watch`, its memo works once the initial resync arrives.
    func open(drive: String, readOnly: Bool, watch: Bool = true) async throws -> BridgeSession {
        let (_, epoch) = current()
        let info: BridgeSessionInfo = try await send(epoch: epoch) { p, done in p.openSession(drive: drive, readOnly: readOnly) { done(Self.result($0, $1)) } }
        let session = BridgeSession(client: self, info: info, epoch: epoch)
        let kept = lock.withLock { () -> Bool in
            guard self.epoch == epoch else { return false }
            sessions[info.session] = session
            return true
        }
        guard kept else { throw Bridge.error(ESTALE, "StaleSession", "the bridge connection was lost: open another session") }
        if watch { try await session.watch() }
        return session
    }

    func forget(_ session: BridgeSession) {
        lock.withLock { if sessions[session.token] === session { sessions[session.token] = nil } }
    }

    private func session(_ token: Int64) -> BridgeSession? { lock.withLock { sessions[token] } }

    // MARK: VoidfsBridgeEvents, from the agent

    func invalidated(_ session: Int64, event: BridgeEvent) {
        let queued = lock.withLock { () -> Bool in
            guard held != nil else { return false }
            held!.append((session, event))
            return true
        }
        if !queued { self.session(session)?.apply(event) }
    }

    func holdEvents() { lock.withLock { if held == nil { held = [] } } }

    func releaseEvents() {
        let queued = lock.withLock { () -> [(Int64, BridgeEvent)] in defer { held = nil }; return held ?? [] }
        for (session, event) in queued { self.session(session)?.apply(event) }
    }

    func watchEnded(_ session: Int64, error: NSError?) { self.session(session)?.unwatched() }
}

final class BridgeSession: @unchecked Sendable {
    let client: BridgeClient
    let info: BridgeSessionInfo
    let epoch: Int
    var token: Int64 { info.session }

    private struct Name: Hashable { let parent: UInt64; let name: String }

    private let lock = NSLock()
    private var attrs: [UInt64: BridgeAttr] = [:]
    private var names: [Name: UInt64] = [:]
    private var children: [UInt64: Set<String>] = [:]
    private var handles: [UInt64: UInt64] = [:]
    /// Counts invalidations and local changes; a reply in flight across one isn't kept.
    private var changes: UInt64 = 0
    private var watching = false
    private var dead = false
    private(set) var generation: UInt64 = 0
    private var waiters: [Int: (UInt64, CheckedContinuation<UInt64, Error>)] = [:]
    private var nextWaiter = 0
    private var started: CheckedContinuation<Void, Error>?
    private(set) var memoHits = 0
    /// Calls tried again after `EAGAIN`.
    private(set) var retries = 0
    private(set) var events = 0
    private static let memoLimit = 1 << 16

    init(client: BridgeClient, info: BridgeSessionInfo, epoch: Int) { (self.client, self.info, self.epoch) = (client, info, epoch) }

    var isWatching: Bool { lock.withLock { watching } }

    // MARK: Memo

    private func cached(_ ino: UInt64) -> BridgeAttr? {
        lock.withLock {
            guard watching, let a = attrs[ino] else { return nil }
            memoHits += 1
            return a
        }
    }

    private func cached(_ parent: UInt64, _ name: String) -> BridgeAttr? {
        lock.withLock {
            guard watching, let ino = names[Name(parent: parent, name: name)], let a = attrs[ino] else { return nil }
            memoHits += 1
            return a
        }
    }

    private var mark: UInt64 { lock.withLock { changes } }

    private func remember(_ a: BridgeAttr, under name: Name? = nil, since: UInt64) {
        lock.withLock {
            guard watching, changes == since else { return }
            if attrs.count >= Self.memoLimit { clearLocked() }
            attrs[a.ino] = a
            if let name {
                names[name] = a.ino
                children[name.parent, default: []].insert(name.name)
            }
        }
    }

    private func clearLocked() {
        attrs.removeAll()
        names.removeAll()
        children.removeAll()
    }

    private func dropLocked(_ ino: UInt64) {
        attrs[ino] = nil
        if let names = children.removeValue(forKey: ino) {
            for n in names { self.names[Name(parent: ino, name: n)] = nil }
        }
    }

    /// Before and after a change of this client's: what it touches is no longer known.
    private func touch(_ inodes: [UInt64], names gone: [Name] = []) {
        lock.withLock {
            changes += 1
            for ino in inodes { dropLocked(ino) }
            for n in gone {
                if let ino = names.removeValue(forKey: n) { attrs[ino] = nil }
                children[n.parent]?.remove(n.name)
            }
        }
    }

    func apply(_ event: BridgeEvent) {
        var ready: [CheckedContinuation<UInt64, Error>] = []
        var start: CheckedContinuation<Void, Error>?
        lock.withLock {
            changes += 1
            events += 1
            if event.all || event.resync { clearLocked() } else { for ino in event.inodes { dropLocked(ino) } }
            if event.resync && event.all && !watching && !dead {
                watching = true
                start = started
                started = nil
            }
            generation = max(generation, event.generation)
            for (id, (at, c)) in waiters where event.generation >= at {
                waiters[id] = nil
                ready.append(c)
            }
        }
        start?.resume()
        for c in ready { c.resume(returning: event.generation) }
    }

    func unwatched() {
        let (start, pending): (CheckedContinuation<Void, Error>?, [CheckedContinuation<UInt64, Error>]) = lock.withLock {
            watching = false
            changes += 1
            clearLocked()
            defer { started = nil; waiters.removeAll() }
            return (started, waiters.values.map(\.1))
        }
        let ended = Bridge.error(ESTALE, "WatchEnded", "the invalidation relay ended")
        start?.resume(throwing: ended)
        for c in pending { c.resume(throwing: ended) }
    }

    func lost() {
        lock.withLock { dead = true }
        unwatched()
    }

    /// Starts the relay and waits, at most 10 s, for its initial resync.
    func watch() async throws {
        try await withCheckedThrowingContinuation { (c: CheckedContinuation<Void, Error>) in
            lock.withLock { started = c }
            DispatchQueue.global().asyncAfter(deadline: .now() + 10) { [weak self] in
                let late = self?.lock.withLock { () -> CheckedContinuation<Void, Error>? in defer { self?.started = nil }; return self?.started }
                late?.resume(throwing: Bridge.error(ETIMEDOUT, "Timeout", "the invalidation relay sent no initial resync"))
            }
            Task {
                do {
                    try await send { p, done in p.watch(token) { done($0.map { .failure($0) } ?? .success(())) } }
                } catch {
                    let pending = lock.withLock { () -> CheckedContinuation<Void, Error>? in defer { started = nil }; return started }
                    pending?.resume(throwing: error)
                }
            }
        }
    }

    /// Returns once an invalidation of at least `generation` has arrived.
    func waitForGeneration(above previous: UInt64, timeout: Duration) async throws -> UInt64 {
        try await withCheckedThrowingContinuation { c in
            let (now, id): (UInt64?, Int) = lock.withLock {
                if generation > previous { return (generation, 0) }
                nextWaiter += 1
                waiters[nextWaiter] = (previous + 1, c)
                return (nil, nextWaiter)
            }
            if let now { c.resume(returning: now); return }
            let seconds = Double(timeout.components.seconds) + Double(timeout.components.attoseconds) / 1e18
            DispatchQueue.global().asyncAfter(deadline: .now() + seconds) { [weak self] in
                let expired = self?.lock.withLock { self?.waiters.removeValue(forKey: id)?.1 }
                expired?.resume(throwing: Bridge.error(ETIMEDOUT, "Timeout", "no generation update above \(previous)"))
            }
        }
    }

    // MARK: Calls

    /// `EAGAIN` means the call took no effect: the core's check raced a namespace change (a
    /// directory page, or a mutation after its own retries), or admission was full. It is tried
    /// again, eight times over about 200 ms, before the caller sees it.
    private func send<T>(_ call: (VoidfsBridgeProtocol, @escaping (Result<T, Error>) -> Void) -> Void) async throws -> T {
        var delay = 1
        for attempt in 0... {
            if lock.withLock({ dead }) { throw Bridge.error(ESTALE, "StaleSession", "the bridge connection was lost: open another session") }
            do { return try await client.send(epoch: epoch, call) }
            catch let e as NSError where e.domain == NSPOSIXErrorDomain && e.code == Int(EAGAIN) && attempt < 8 {
                lock.withLock { retries += 1 }
                try await Task.sleep(for: .milliseconds(delay))
                delay = min(delay * 2, 50)
            }
        }
        fatalError("unreachable")
    }

    private func void(_ e: NSError?) -> Result<Void, Error> { e.map { .failure($0) } ?? .success(()) }

    func release() async throws {
        defer {
            lock.withLock { dead = true }
            unwatched()
            client.forget(self)
        }
        try await send { p, done in p.releaseSession(token) { done(self.void($0)) } }
    }

    func getattr(_ ino: UInt64) async throws -> BridgeAttr {
        if let a = cached(ino) { return a }
        return try await getattrUncached(ino)
    }

    /// Always through the agent; what a reply finds is remembered.
    func getattrUncached(_ ino: UInt64) async throws -> BridgeAttr {
        let since = mark
        let a: BridgeAttr = try await send { p, done in p.getattr(token, ino: ino) { done(BridgeClient.result($0, $1)) } }
        remember(a, since: since)
        return a
    }

    func lookup(_ parent: UInt64, _ name: String) async throws -> BridgeAttr {
        if let a = cached(parent, name) { return a }
        let since = mark
        let a: BridgeAttr = try await send { p, done in p.lookup(token, parent: parent, name: name) { done(BridgeClient.result($0, $1)) } }
        remember(a, under: Name(parent: parent, name: name), since: since)
        return a
    }

    func readlink(_ ino: UInt64) async throws -> String {
        try await send { p, done in p.readlink(token, ino: ino) { done(BridgeClient.result($0, $1)) } }
    }

    func readdir(_ ino: UInt64, after: String?, limit: Int) async throws -> BridgeDirPage {
        try await send { p, done in p.readdir(token, ino: ino, after: after, limit: limit) { done(BridgeClient.result($0, $1)) } }
    }

    func open(_ ino: UInt64, write: Bool) async throws -> (UInt64, BridgeAttr) {
        let (fh, a): (UInt64, BridgeAttr) = try await send { p, done in
            p.open(token, ino: ino, write: write) { fh, a, e in done(BridgeClient.result(a, e).map { (fh, $0) }) }
        }
        lock.withLock { handles[fh] = ino }
        return (fh, a)
    }

    func handleAttr(_ fh: UInt64) async throws -> BridgeAttr {
        try await send { p, done in p.handleAttr(token, fh: fh) { done(BridgeClient.result($0, $1)) } }
    }

    func read(_ fh: UInt64, offset: UInt64, length: UInt64) async throws -> Data {
        try await send { p, done in p.read(token, fh: fh, offset: offset, length: length) { done(BridgeClient.result($0, $1)) } }
    }

    private func changing<T>(_ inodes: [UInt64], names: [Name] = [], _ body: () async throws -> T) async throws -> T {
        touch(inodes, names: names)
        defer { touch(inodes, names: names) }
        return try await body()
    }

    private func inode(_ fh: UInt64) -> [UInt64] { lock.withLock { handles[fh].map { [$0] } ?? [] } }

    func write(_ fh: UInt64, offset: UInt64, data: Data) async throws -> Int {
        try await changing(inode(fh)) {
            try await send { p, done in p.write(token, fh: fh, offset: offset, data: data) { n, e in done(e.map { .failure($0) } ?? .success(n)) } }
        }
    }

    func truncate(_ fh: UInt64, size: UInt64) async throws {
        try await changing(inode(fh)) { try await send { p, done in p.truncate(token, fh: fh, size: size) { done(self.void($0)) } } }
    }

    func fsync(_ fh: UInt64) async throws {
        try await send { p, done in p.fsync(token, fh: fh) { done(self.void($0)) } }
    }

    func close(_ fh: UInt64) async throws {
        try await changing(inode(fh)) { try await send { p, done in p.close(token, fh: fh) { done(self.void($0)) } } }
        lock.withLock { handles[fh] = nil }
    }

    func create(_ parent: UInt64, _ name: String, mode: UInt32, directory: Bool = false) async throws -> BridgeAttr {
        try await changing([parent], names: [Name(parent: parent, name: name)]) {
            try await send { p, done in p.create(token, parent: parent, name: name, mode: mode, directory: directory) { done(BridgeClient.result($0, $1)) } }
        }
    }

    func remove(_ parent: UInt64, _ name: String, directory: Bool = false) async throws {
        try await changing([parent], names: [Name(parent: parent, name: name)]) {
            try await send { p, done in p.remove(token, parent: parent, name: name, directory: directory) { done(self.void($0)) } }
        }
    }

    /// `how`: 0 replace, 1 exclusive, 2 swap.
    func rename(_ fromParent: UInt64, _ fromName: String, _ toParent: UInt64, _ toName: String, how: Int = 0) async throws {
        try await changing([fromParent, toParent], names: [Name(parent: fromParent, name: fromName), Name(parent: toParent, name: toName)]) {
            try await send { p, done in p.rename(token, fromParent: fromParent, fromName: fromName, toParent: toParent, toName: toName, how: how) { done(self.void($0)) } }
        }
    }

    func link(_ ino: UInt64, _ parent: UInt64, _ name: String, clone: Bool = false) async throws -> BridgeAttr {
        try await changing([ino, parent]) {
            try await send { p, done in p.link(token, ino: ino, parent: parent, name: name, clone: clone) { done(BridgeClient.result($0, $1)) } }
        }
    }

    func setattr(_ ino: UInt64, mode: UInt32? = nil, mtime: (Int64, Int32)? = nil) async throws -> BridgeAttr {
        try await changing([ino]) {
            try await send { p, done in
                p.setattr(token, ino: ino, mode: mode.map { NSNumber(value: $0) }, mtimeSeconds: mtime.map { NSNumber(value: $0.0) }, mtimeNanoseconds: mtime?.1 ?? 0) { done(BridgeClient.result($0, $1)) }
            }
        }
    }

    func getxattr(_ ino: UInt64, _ name: String) async throws -> Data {
        try await send { p, done in p.getxattr(token, ino: ino, name: name) { done(BridgeClient.result($0, $1)) } }
    }

    func listxattr(_ ino: UInt64) async throws -> [String] {
        try await send { p, done in p.listxattr(token, ino: ino) { done(BridgeClient.result($0, $1)) } }
    }

    /// `how`: 0 set, 1 create, 2 replace.
    func setxattr(_ ino: UInt64, _ name: String, _ value: Data, how: Int = 0) async throws {
        try await changing([ino]) { try await send { p, done in p.setxattr(token, ino: ino, name: name, value: value, how: how) { done(self.void($0)) } } }
    }

    func removexattr(_ ino: UInt64, _ name: String) async throws {
        try await changing([ino]) { try await send { p, done in p.removexattr(token, ino: ino, name: name) { done(self.void($0)) } } }
    }
}
