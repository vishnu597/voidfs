// SPDX-License-Identifier: Apache-2.0
//
// The launch agent: the app's own binary, which launchd runs from the bundle's
// `Contents/Library/LaunchAgents` plist once `SMAppService` registers it. It answers the bridge
// (`Shared/Bridge.swift`) on the App-Group-prefixed Mach service, accepts only the extension's code
// signature, and forwards each call to the Rust daemon's user-only socket. It owns no journal,
// publisher or cache: the daemon does.

import Foundation
import ServiceManagement
import os

enum Agent {
    static let service = SMAppService.agent(plistName: "dev.voidfs.agent.plist")
    static let log = Logger(subsystem: "dev.voidfs", category: "agent")

    static var status: String {
        switch service.status {
        case .notRegistered: "not registered"
        case .enabled: "enabled"
        case .requiresApproval: "requires approval in System Settings → General → Login Items & Extensions"
        case .notFound: "not found in the bundle"
        @unknown default: "unknown (\(service.status.rawValue))"
        }
    }

    /// The daemon's state directory: the CLI's default, `voidfs_client::default_dir()`, which the
    /// app shares, so that one daemon owns one store. Not the App Group container: macOS keeps the
    /// CLI's daemon out of it (step 5, "One daemon for the CLI and the app").
    static var stateDirectory: URL? {
        FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Application Support/voidfs")
    }

    static func serve() -> Never {
        guard let socket = stateDirectory?.appending(path: "daemon.sock").path else {
            log.error("no home directory for the daemon's state")
            exit(1)
        }
        let delegate = BridgeListener(socket: DaemonSocket(path: socket))
        let listener = NSXPCListener(machServiceName: Bridge.service)
        listener.delegate = delegate
        listener.resume()
        // launchd stops an agent with SIGTERM: release the daemon sessions first, as an
        // invalidated connection does. A kill leaves them to the daemon's next restart.
        signal(SIGTERM, SIG_IGN)
        let term = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
        term.setEventHandler {
            log.notice("SIGTERM: releasing \(delegate.sessionCount) sessions")
            Task {
                await delegate.releaseAll()
                exit(0)
            }
        }
        term.resume()
        log.notice("bridge serving \(Bridge.service, privacy: .public), daemon socket \(socket, privacy: .public)")
        withExtendedLifetime((listener, delegate, term)) { RunLoop.main.run() }
        exit(0)
    }
}

final class BridgeListener: NSObject, NSXPCListenerDelegate, @unchecked Sendable {
    let socket: DaemonSocket
    /// The code signature a peer must have. The Mach service uses the extension's; only the
    /// app's in-process self-test names its own.
    let requirement: String
    private let lock = NSLock()
    private var bridges: [ObjectIdentifier: BridgeConnection] = [:]

    init(socket: DaemonSocket, requirement: String = Bridge.peerRequirement) { (self.socket, self.requirement) = (socket, requirement) }

    var sessionCount: Int { lock.withLock { bridges.values.reduce(0) { $0 + $1.sessionCount } } }

    func listener(_ listener: NSXPCListener, shouldAcceptNewConnection connection: NSXPCConnection) -> Bool {
        guard connection.effectiveUserIdentifier == getuid() else {
            Agent.log.error("refused a connection from uid \(connection.effectiveUserIdentifier), pid \(connection.processIdentifier)")
            return false
        }
        // Messages from any other code are refused and invalidate the connection.
        connection.setCodeSigningRequirement(requirement)
        let bridge = BridgeConnection(socket: socket, connection: connection)
        let key = ObjectIdentifier(bridge)
        let pid = connection.processIdentifier
        connection.exportedInterface = Bridge.interface()
        connection.exportedObject = bridge
        connection.remoteObjectInterface = Bridge.eventsInterface()
        connection.invalidationHandler = { [weak self] in
            Agent.log.notice("connection from pid \(pid) ended after \(bridge.calls) calls; releasing \(bridge.sessionCount) sessions")
            Task { await bridge.releaseAll() }
            self?.lock.withLock { _ = self?.bridges.removeValue(forKey: key) }
        }
        lock.withLock { bridges[key] = bridge }
        connection.resume()
        return true
    }

    /// The daemon sessions the connections hold, for the self-test.
    func daemonSessions() -> [(String, UInt32)] { lock.withLock { bridges.values.flatMap { $0.daemonSessions() } } }

    /// Ends every connection, as an agent that exits does.
    func dropAll() { for b in lock.withLock({ Array(bridges.values) }) { b.connection?.invalidate() } }

    func releaseAll() async {
        let all = lock.withLock { Array(bridges.values) }
        await withTaskGroup(of: Void.self) { group in
            for bridge in all { group.addTask { await bridge.releaseAll() } }
        }
    }
}

/// One session the connection opened in the daemon. Its daemon ID stays in the agent.
private final class AgentSession: @unchecked Sendable {
    let token: Int64
    let id: String
    let generation: UInt32
    let maxIo: UInt64
    let maxEntries: Int
    var watch: Int32?

    init(token: Int64, id: String, generation: UInt32, maxIo: UInt64, maxEntries: Int) {
        (self.token, self.id, self.generation, self.maxIo, self.maxEntries) = (token, id, generation, maxIo, maxEntries)
    }

    func route(_ op: String) -> String { "/v1/fs/\(id)/\(op)" }
}

/// What one extension connection may do: its own sessions, a bounded number of calls at a
/// time, and only the daemon's filesystem calls, built here from typed arguments.
final class BridgeConnection: NSObject, VoidfsBridgeProtocol, @unchecked Sendable {
    let socket: DaemonSocket
    weak var connection: NSXPCConnection?
    private let lock = NSLock()
    private var sessions: [Int64: AgentSession] = [:]
    private var next: Int64 = 1
    private var inFlight = 0
    private(set) var calls = 0

    init(socket: DaemonSocket, connection: NSXPCConnection) { (self.socket, self.connection) = (socket, connection) }

    var sessionCount: Int { lock.withLock { sessions.count } }

    func daemonSessions() -> [(String, UInt32)] { lock.withLock { sessions.values.map { ($0.id, $0.generation) } } }

    // MARK: Admission and errors

    private func admit() -> Bool {
        lock.withLock {
            guard inFlight < Bridge.maxCalls else { return false }
            inFlight += 1
            calls += 1
            return true
        }
    }

    private func leave() { lock.withLock { inFlight -= 1 } }

    private func session(_ token: Int64) -> AgentSession? { lock.withLock { sessions[token] } }

    /// Runs one call off the connection's queue, so that calls overlap, and answers its reply
    /// exactly once: `body` returns normally only after replying. The socket call blocks a GCD
    /// thread rather than hopping through Swift concurrency's pool as well.
    private func run(_ token: Int64?, fail: @escaping (NSError) -> Void, _ body: @escaping (AgentSession?) throws -> Void) {
        guard admit() else { fail(Bridge.error(EAGAIN, "Busy", "too many bridge calls in flight")); return }
        var found: AgentSession?
        if let token {
            guard let s = session(token) else { leave(); fail(Bridge.error(ESTALE, "StaleSession", "no such bridge session: open another")); return }
            found = s
        }
        DispatchQueue.global(qos: .userInitiated).async {
            defer { self.leave() }
            do { try body(found) } catch { fail(Self.nsError(error)) }
        }
    }

    static func nsError(_ error: Error) -> NSError {
        switch error {
        case let e as NSError where e.domain == NSPOSIXErrorDomain: return e
        case DaemonSocketError.notRunning: return Bridge.error(ECONNREFUSED, "DaemonNotRunning", "the voidfs daemon isn't running")
        case let DaemonSocketError.io(e): return Bridge.error(e == 0 ? EIO : e, "Io", "the daemon socket: \(String(cString: strerror(e == 0 ? EIO : e)))")
        case DaemonSocketError.tooLarge: return Bridge.error(EIO, "TooLarge", "the daemon's answer exceeds its bound")
        case let DaemonSocketError.malformed(what): return Bridge.error(EIO, "BadResponse", "the daemon's answer: \(what)")
        case let e as DaemonCallError: return refusal(e.status, e.body)
        default: return Bridge.error(EIO, "Failed", String(describing: error))
        }
    }

    /// The daemon's `{error: {code, message, errno}}`, as a POSIX error.
    static func refusal(_ status: Int, _ body: Data) -> NSError {
        if let o = try? JSONSerialization.jsonObject(with: body) as? [String: Any], let e = o["error"] as? [String: Any],
           let code = e["code"] as? String, let message = e["message"] as? String, let errno = (e["errno"] as? NSNumber)?.int32Value, errno > 0 {
            return Bridge.error(errno, code, message)
        }
        return Bridge.error(EIO, "UnexpectedResponse", "the daemon answered \(status)")
    }

    static func invalid(_ what: String) -> NSError { Bridge.error(EINVAL, "InvalidArgument", what) }

    /// A name as the daemon takes one: 1 to 255 bytes, no `/` or NUL. Never a path.
    static func checkName(_ name: String) throws {
        let n = name.utf8.count
        guard n > 0, n <= Bridge.maxName, !name.contains("/"), !name.contains("\0") else { throw invalid("invalid name") }
    }

    // MARK: Socket calls

    private func call(_ s: AgentSession, _ op: String, _ body: [String: Any]) throws -> Any {
        let data = try JSONSerialization.data(withJSONObject: body)
        let response = try socket.requestSync("POST", s.route(op), generation: s.generation, contentType: "application/json", body: data, limit: 1 << 20)
        guard (200..<300).contains(response.status) else { throw DaemonCallError(status: response.status, body: response.body) }
        do { return try JSONSerialization.jsonObject(with: response.body) } catch { throw DaemonSocketError.malformed("JSON") }
    }

    private func raw(_ s: AgentSession, _ method: String, _ target: String, body: Data = Data(), limit: Int) throws -> Data {
        let response = try socket.requestSync(method, target, generation: s.generation, contentType: method == "PUT" ? "application/octet-stream" : nil, body: body, limit: limit)
        guard (200..<300).contains(response.status) else { throw DaemonCallError(status: response.status, body: response.body) }
        return response.body
    }

    private func attr(_ s: AgentSession, _ op: String, _ body: [String: Any]) throws -> BridgeAttr {
        try Wire.attr(try call(s, op, body))
    }

    private func empty(_ s: AgentSession, _ op: String, _ body: [String: Any]) throws {
        _ = try call(s, op, body)
    }

    // MARK: VoidfsBridgeProtocol

    func ping(_ payload: Data, reply: @escaping (Data) -> Void) { reply(payload) }

    func openSession(drive: String, readOnly: Bool, reply: @escaping (BridgeSessionInfo?, NSError?) -> Void) {
        run(nil, fail: { reply(nil, $0) }) { _ in
            let n = drive.utf8.count
            guard n > 0, n <= Bridge.maxDrive, !drive.contains("/"), !drive.contains("\0") else { throw Self.invalid("invalid drive name") }
            let token: Int64? = self.lock.withLock {
                guard self.sessions.count < Bridge.maxSessions else { return nil }
                defer { self.next += 1 }
                return self.next
            }
            guard let token else { throw Bridge.error(EAGAIN, "Busy", "too many bridge sessions on this connection") }
            let body = try JSONSerialization.data(withJSONObject: ["version": 1, "drive": drive, "readOnly": readOnly])
            let response = try self.socket.requestSync("POST", "/v1/fs/sessions", generation: nil, contentType: "application/json", body: body, limit: 1 << 20)
            guard (200..<300).contains(response.status) else { throw DaemonCallError(status: response.status, body: response.body) }
            let (info, id) = try Wire.session(response.body, token: token)
            let s = AgentSession(token: token, id: id, generation: info.generation, maxIo: info.maxIo, maxEntries: info.maxEntries)
            let kept: Bool = self.lock.withLock {
                guard self.connection != nil else { return false }
                self.sessions[token] = s
                return true
            }
            if !kept { _ = try? self.call(s, "release", [:]); throw Bridge.error(ECANCELED, "Cancelled", "the connection ended") }
            reply(info, nil)
        }
    }

    func releaseSession(_ session: Int64, reply: @escaping (NSError?) -> Void) {
        run(session, fail: { reply($0) }) { s in
            try self.release(s!)
            reply(nil)
        }
    }

    /// Forgets the session once the daemon has released it, or no longer knows it.
    private func release(_ s: AgentSession) throws {
        do { try empty(s, "release", [:]) }
        catch let e as DaemonCallError where e.status == 409 {}
        let watch: Int32? = lock.withLock { sessions.removeValue(forKey: s.token); return s.watch }
        if let watch { DaemonSocket.cancel(watch) }
    }

    func releaseAll() async {
        let all = lock.withLock { Array(sessions.values) }
        await withTaskGroup(of: Void.self) { group in
            for s in all { group.addTask { try? self.release(s) } }
        }
    }

    func watch(_ session: Int64, reply: @escaping (NSError?) -> Void) {
        guard let s = self.session(session) else { reply(Bridge.error(ESTALE, "StaleSession", "no such bridge session: open another")); return }
        guard lock.withLock({ s.watch == nil }) else { reply(Bridge.error(EBUSY, "Busy", "the session is already watched")); return }
        let events = connection?.remoteObjectProxyWithErrorHandler { _ in } as? VoidfsBridgeEvents
        let opened = DispatchSemaphore(value: 0)
        var failure: NSError?
        let thread = Thread {
            var previous: UInt64?
            var first = true
            do {
                try self.socket.stream(s.route("watch"), generation: s.generation, limit: 1 << 20, opened: { fd in
                    self.lock.withLock { s.watch = fd }
                    opened.signal()
                }) { line in
                    guard let event = Wire.event(line), first ? (event.resync && event.all) : event.generation > (previous ?? 0) else {
                        failure = Bridge.error(EIO, "BadResponse", "the daemon's invalidation regressed or lacks its initial resync")
                        return false
                    }
                    (first, previous) = (false, event.generation)
                    events?.invalidated(s.token, event: event)
                    return true
                }
            } catch {
                failure = Self.nsError(error)
                opened.signal()
            }
            let ended = self.lock.withLock { () -> Bool in
                defer { s.watch = nil }
                return self.sessions[s.token] === s
            }
            Agent.log.debug("watch \(s.token) ended: \(failure?.localizedDescription ?? "closed", privacy: .public)")
            events?.watchEnded(s.token, error: ended ? failure ?? Bridge.error(ESTALE, "WatchEnded", "the daemon ended the watch") : nil)
        }
        thread.name = "dev.voidfs.agent.watch"
        thread.start()
        opened.wait()
        reply(lock.withLock { s.watch == nil } ? failure ?? Bridge.error(EIO, "WatchFailed", "the watch did not start") : nil)
    }

    func lookup(_ session: Int64, parent: UInt64, name: String, reply: @escaping (BridgeAttr?, NSError?) -> Void) {
        run(session, fail: { reply(nil, $0) }) { s in
            try Self.checkName(name)
            reply(try self.attr(s!, "lookup", ["parent": parent, "name": name]), nil)
        }
    }

    func getattr(_ session: Int64, ino: UInt64, reply: @escaping (BridgeAttr?, NSError?) -> Void) {
        run(session, fail: { reply(nil, $0) }) { s in reply(try self.attr(s!, "getattr", ["ino": ino]), nil) }
    }

    func readlink(_ session: Int64, ino: UInt64, reply: @escaping (String?, NSError?) -> Void) {
        run(session, fail: { reply(nil, $0) }) { s in
            guard let o = try self.call(s!, "readlink", ["ino": ino]) as? [String: Any], let target = o["target"] as? String, target.utf8.count <= 4096 else { throw DaemonSocketError.malformed("readlink") }
            reply(target, nil)
        }
    }

    func readdir(_ session: Int64, ino: UInt64, after: String?, limit: Int, reply: @escaping (BridgeDirPage?, NSError?) -> Void) {
        run(session, fail: { reply(nil, $0) }) { s in
            guard limit > 0, limit <= min(s!.maxEntries, Bridge.maxEntries), (after?.utf8.count ?? 0) <= 1024 else { throw Self.invalid("directory page out of bounds") }
            let page = try Wire.page(try self.call(s!, "readdir", ["ino": ino, "after": after.map { $0 as Any } ?? NSNull(), "limit": limit]), limit: limit)
            reply(page, nil)
        }
    }

    func open(_ session: Int64, ino: UInt64, write: Bool, reply: @escaping (UInt64, BridgeAttr?, NSError?) -> Void) {
        run(session, fail: { reply(0, nil, $0) }) { s in
            guard let o = try self.call(s!, "open", ["ino": ino, "write": write]) as? [String: Any], let fh = (o["fh"] as? NSNumber)?.uint64Value, let a = o["attr"] else { throw DaemonSocketError.malformed("open") }
            reply(fh, try Wire.attr(a), nil)
        }
    }

    func handleAttr(_ session: Int64, fh: UInt64, reply: @escaping (BridgeAttr?, NSError?) -> Void) {
        run(session, fail: { reply(nil, $0) }) { s in reply(try self.attr(s!, "handle_attr", ["fh": fh]), nil) }
    }

    func read(_ session: Int64, fh: UInt64, offset: UInt64, length: UInt64, reply: @escaping (Data?, NSError?) -> Void) {
        run(session, fail: { reply(nil, $0) }) { s in
            guard length <= s!.maxIo, offset <= UInt64(Int64.max) - length else { throw Self.invalid("read out of bounds") }
            reply(try self.raw(s!, "GET", "\(s!.route("read"))?fh=\(fh)&offset=\(offset)&length=\(length)", limit: Int(length)), nil)
        }
    }

    func write(_ session: Int64, fh: UInt64, offset: UInt64, data: Data, reply: @escaping (Int, NSError?) -> Void) {
        run(session, fail: { reply(0, $0) }) { s in
            guard UInt64(data.count) <= s!.maxIo, offset <= UInt64(Int64.max) - UInt64(data.count) else { throw Self.invalid("write out of bounds") }
            let body = try self.raw(s!, "PUT", "\(s!.route("write"))?fh=\(fh)&offset=\(offset)", body: data, limit: 4096)
            guard let o = try? JSONSerialization.jsonObject(with: body) as? [String: Any], let n = (o["written"] as? NSNumber)?.intValue, n >= 0, n <= data.count else { throw DaemonSocketError.malformed("write") }
            reply(n, nil)
        }
    }

    func truncate(_ session: Int64, fh: UInt64, size: UInt64, reply: @escaping (NSError?) -> Void) {
        run(session, fail: { reply($0) }) { s in try self.empty(s!, "truncate", ["fh": fh, "size": size]); reply(nil) }
    }

    func fsync(_ session: Int64, fh: UInt64, reply: @escaping (NSError?) -> Void) {
        run(session, fail: { reply($0) }) { s in try self.empty(s!, "fsync", ["fh": fh]); reply(nil) }
    }

    func close(_ session: Int64, fh: UInt64, reply: @escaping (NSError?) -> Void) {
        run(session, fail: { reply($0) }) { s in try self.empty(s!, "close", ["fh": fh]); reply(nil) }
    }

    func create(_ session: Int64, parent: UInt64, name: String, mode: UInt32, directory: Bool, reply: @escaping (BridgeAttr?, NSError?) -> Void) {
        run(session, fail: { reply(nil, $0) }) { s in
            try Self.checkName(name)
            reply(try self.attr(s!, directory ? "mkdir" : "create", ["parent": parent, "name": name, "mode": mode]), nil)
        }
    }

    func remove(_ session: Int64, parent: UInt64, name: String, directory: Bool, reply: @escaping (NSError?) -> Void) {
        run(session, fail: { reply($0) }) { s in
            try Self.checkName(name)
            try self.empty(s!, directory ? "rmdir" : "unlink", ["parent": parent, "name": name])
            reply(nil)
        }
    }

    func rename(_ session: Int64, fromParent: UInt64, fromName: String, toParent: UInt64, toName: String, how: Int, reply: @escaping (NSError?) -> Void) {
        run(session, fail: { reply($0) }) { s in
            try Self.checkName(fromName)
            try Self.checkName(toName)
            guard let how = ["replace", "exclusive", "swap"].indices.contains(how) ? ["replace", "exclusive", "swap"][how] : nil else { throw Self.invalid("rename mode") }
            try self.empty(s!, "rename", ["fromParent": fromParent, "fromName": fromName, "toParent": toParent, "toName": toName, "how": how])
            reply(nil)
        }
    }

    func link(_ session: Int64, ino: UInt64, parent: UInt64, name: String, clone: Bool, reply: @escaping (BridgeAttr?, NSError?) -> Void) {
        run(session, fail: { reply(nil, $0) }) { s in
            try Self.checkName(name)
            reply(try self.attr(s!, clone ? "clone_file" : "link", ["ino": ino, "parent": parent, "name": name]), nil)
        }
    }

    func setattr(_ session: Int64, ino: UInt64, mode: NSNumber?, mtimeSeconds: NSNumber?, mtimeNanoseconds: Int32, reply: @escaping (BridgeAttr?, NSError?) -> Void) {
        run(session, fail: { reply(nil, $0) }) { s in
            var body: [String: Any] = ["ino": ino]
            if let mode {
                guard mode.int64Value >= 0, mode.int64Value <= 0o7777 else { throw Self.invalid("mode") }
                body["mode"] = mode.uint32Value
            }
            if let seconds = mtimeSeconds {
                guard let text = Wire.timestamp(seconds.int64Value, mtimeNanoseconds) else { throw Self.invalid("modification time") }
                body["mtime"] = text
            }
            reply(try self.attr(s!, "setattr", body), nil)
        }
    }

    func getxattr(_ session: Int64, ino: UInt64, name: String, reply: @escaping (Data?, NSError?) -> Void) {
        run(session, fail: { reply(nil, $0) }) { s in
            try Self.checkXattr(name)
            reply(try self.raw(s!, "GET", "\(s!.route("getxattr"))?ino=\(ino)&name=\(Wire.encode(name))", limit: Bridge.maxXattr), nil)
        }
    }

    func listxattr(_ session: Int64, ino: UInt64, reply: @escaping ([String]?, NSError?) -> Void) {
        run(session, fail: { reply(nil, $0) }) { s in
            guard let o = try self.call(s!, "listxattr", ["ino": ino]) as? [String: Any], let names = o["names"] as? [String],
                  names.reduce(0, { $0 + $1.utf8.count }) <= Bridge.maxXattr else { throw DaemonSocketError.malformed("listxattr") }
            reply(names, nil)
        }
    }

    func setxattr(_ session: Int64, ino: UInt64, name: String, value: Data, how: Int, reply: @escaping (NSError?) -> Void) {
        run(session, fail: { reply($0) }) { s in
            try Self.checkXattr(name)
            guard value.count <= Bridge.maxXattr else { throw Bridge.error(E2BIG, "TooLarge", "extended attributes exceed the size limit") }
            guard let how = ["set", "create", "replace"].indices.contains(how) ? ["set", "create", "replace"][how] : nil else { throw Self.invalid("xattr mode") }
            _ = try self.raw(s!, "PUT", "\(s!.route("setxattr"))?ino=\(ino)&name=\(Wire.encode(name))&how=\(how)", body: value, limit: 4096)
            reply(nil)
        }
    }

    func removexattr(_ session: Int64, ino: UInt64, name: String, reply: @escaping (NSError?) -> Void) {
        run(session, fail: { reply($0) }) { s in
            try Self.checkXattr(name)
            try self.empty(s!, "removexattr", ["ino": ino, "name": name])
            reply(nil)
        }
    }

    static func checkXattr(_ name: String) throws {
        let n = name.utf8.count
        guard n > 0, n <= Bridge.maxName, !name.contains("\0") else { throw invalid("invalid extended attribute name") }
    }
}

/// The daemon's JSON, checked and turned into the bridge's types.
enum Wire {
    static func encode(_ value: String) -> String {
        var out = ""
        for byte in value.utf8 {
            if (byte >= 0x30 && byte <= 0x39) || (byte >= 0x41 && byte <= 0x5a) || (byte >= 0x61 && byte <= 0x7a) || byte == 0x2d || byte == 0x2e || byte == 0x5f || byte == 0x7e {
                out.unicodeScalars.append(UnicodeScalar(byte))
            } else {
                out += String(format: "%%%02X", byte)
            }
        }
        return out
    }

    static func number(_ o: [String: Any], _ key: String) throws -> NSNumber {
        guard let n = o[key] as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID() else { throw DaemonSocketError.malformed(key) }
        return n
    }

    static func string(_ o: [String: Any], _ key: String) throws -> String? {
        switch o[key] {
        case nil, is NSNull: return nil
        case let s as String where s.utf8.count <= 4096: return s
        default: throw DaemonSocketError.malformed(key)
        }
    }

    static func attr(_ any: Any) throws -> BridgeAttr {
        guard let o = any as? [String: Any], let kind = o["kind"] as? String, let sync = o["sync"] as? String, let mtime = o["mtime"] as? String,
              let (seconds, nanos) = time(mtime), let hasXattrs = o["has_xattrs"] as? Bool else { throw DaemonSocketError.malformed("attr") }
        let mode = try number(o, "mode").int64Value
        guard mode >= 0, mode <= 0o7777 else { throw DaemonSocketError.malformed("mode") }
        let k: BridgeAttr.Kind = switch kind { case "file": .file; case "folder": .folder; case "symlink": .symlink; default: .unknown }
        let y: BridgeAttr.SyncState
        switch sync {
        case "Saved": y = .saved
        case "Pending": y = .pending
        case "Saving": y = .saving
        case "Conflict": y = .conflict
        case "Error": y = .error
        default: throw DaemonSocketError.malformed("sync")
        }
        return BridgeAttr(ino: try number(o, "ino").uint64Value, kind: k, size: try number(o, "size").uint64Value, mtimeSeconds: seconds, mtimeNanoseconds: nanos,
                          mode: UInt32(mode), generation: try number(o, "generation").uint64Value, sync: y, hasXattrs: hasXattrs,
                          target: try string(o, "target"), objectId: try string(o, "object_id"), versionId: try string(o, "version_id"))
    }

    static func page(_ any: Any, limit: Int) throws -> BridgeDirPage {
        guard let o = any as? [String: Any], let entries = o["entries"] as? [[Any]], entries.count <= limit else { throw DaemonSocketError.malformed("readdir") }
        var names: [String] = []
        var attrs: [BridgeAttr] = []
        for e in entries {
            guard e.count == 2, let name = e[0] as? String else { throw DaemonSocketError.malformed("entry") }
            names.append(name)
            attrs.append(try attr(e[1]))
        }
        return BridgeDirPage(names: names, attrs: attrs, generation: try number(o, "generation").uint64Value)
    }

    static func session(_ body: Data, token: Int64) throws -> (BridgeSessionInfo, String) {
        guard let o = try? JSONSerialization.jsonObject(with: body) as? [String: Any], let id = o["id"] as? String, let drive = o["drive"] as? String,
              let readOnly = o["readOnly"] as? Bool, let caps = o["capabilities"] as? [String: Any],
              !id.isEmpty, id.utf8.count <= 256, id.utf8.allSatisfy({ ($0 >= 0x30 && $0 <= 0x39) || ($0 >= 0x41 && $0 <= 0x5a) || ($0 >= 0x61 && $0 <= 0x7a) || $0 == 0x2d || $0 == 0x5f }) else {
            throw DaemonSocketError.malformed("session")
        }
        let version = try number(o, "version").intValue
        let generation = try number(o, "generation").int64Value
        let maxIo = try number(o, "maxIo").uint64Value
        let maxEntries = try number(o, "maxEntries").intValue
        guard version == 1, generation > 0, generation <= Int64(UInt32.max), maxIo > 0, maxIo <= Bridge.maxIo, maxEntries > 0, maxEntries <= Bridge.maxEntries else {
            throw DaemonSocketError.malformed("session limits or identity")
        }
        let info = BridgeSessionInfo(session: token, drive: drive, root: try number(o, "root").uint64Value, generation: UInt32(generation),
                                     metadataGeneration: try number(o, "metadataGeneration").uint64Value, readOnly: readOnly, maxIo: maxIo,
                                     maxEntries: maxEntries, capabilities: try JSONSerialization.data(withJSONObject: caps))
        return (info, id)
    }

    static func event(_ line: Data) -> BridgeEvent? {
        guard let o = try? JSONSerialization.jsonObject(with: line) as? [String: Any], let resync = o["resync"] as? Bool,
              let list = o["invalidations"] as? [[String: Any]], let inodes = o["inodes"] as? [NSNumber],
              let generation = try? number(o, "generation"), let seq = try? number(o, "seq") else { return nil }
        var (all, objects, subtrees) = (false, [String](), [String]())
        for i in list {
            switch (i["kind"] as? String, i["key"] as? String) {
            case ("all", _): all = true
            case let ("object", key?): objects.append(key)
            case let ("subtree", key?): subtrees.append(key)
            default: return nil
            }
        }
        return BridgeEvent(generation: generation.uint64Value, seq: seq.uint64Value, resync: resync, all: all, objects: objects, subtrees: subtrees, inodes: inodes.map(\.uint64Value))
    }

    /// RFC 3339 in UTC, as the daemon writes it (`Z`, up to nine fraction digits).
    static func time(_ text: String) -> (Int64, Int32)? {
        let u = Array(text.utf8)
        func digits(_ at: Int, _ n: Int) -> Int64? {
            guard at + n <= u.count else { return nil }
            var v: Int64 = 0
            for b in u[at..<at + n] { guard b >= 0x30, b <= 0x39 else { return nil }; v = v * 10 + Int64(b - 0x30) }
            return v
        }
        guard u.count >= 20, u.last == 0x5a, u[4] == 0x2d, u[7] == 0x2d, u[10] == 0x54, u[13] == 0x3a, u[16] == 0x3a,
              let y = digits(0, 4), let mo = digits(5, 2), let d = digits(8, 2), let h = digits(11, 2), let mi = digits(14, 2), let s = digits(17, 2),
              (1...12).contains(mo), (1...31).contains(d), h < 24, mi < 60, s < 61 else { return nil }
        var nanos: Int64 = 0
        if u.count > 20 {
            let n = u.count - 21
            guard u[19] == 0x2e, n >= 1, n <= 9, let f = digits(20, n) else { return nil }
            nanos = f
            for _ in n..<9 { nanos *= 10 }
        } else if u[19] != 0x5a { return nil }
        return (days(y, mo, d) * 86400 + h * 3600 + mi * 60 + s, Int32(nanos))
    }

    static func timestamp(_ seconds: Int64, _ nanos: Int32) -> String? {
        guard nanos >= 0, nanos < 1_000_000_000 else { return nil }
        let (day, rest) = (seconds >= 0 ? seconds / 86400 : (seconds - 86399) / 86400, ((seconds % 86400) + 86400) % 86400)
        let (y, m, d) = civil(day)
        guard (0...9999).contains(y) else { return nil }
        return String(format: "%04lld-%02lld-%02lldT%02lld:%02lld:%02lld.%09dZ", y, m, d, rest / 3600, rest / 60 % 60, rest % 60, nanos)
    }

    /// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm).
    static func days(_ y0: Int64, _ m: Int64, _ d: Int64) -> Int64 {
        let y = m <= 2 ? y0 - 1 : y0
        let era = (y >= 0 ? y : y - 399) / 400
        let yoe = y - era * 400
        let doy = (153 * (m > 2 ? m - 3 : m + 9) + 2) / 5 + d - 1
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
        return era * 146097 + doe - 719468
    }

    static func civil(_ z0: Int64) -> (Int64, Int64, Int64) {
        let z = z0 + 719468
        let era = (z >= 0 ? z : z - 146096) / 146097
        let doe = z - era * 146097
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100)
        let mp = (5 * doy + 2) / 153
        let d = doy - (153 * mp + 2) / 5 + 1
        let m = mp < 10 ? mp + 3 : mp - 9
        return (yoe + era * 400 + (m <= 2 ? 1 : 0), m, d)
    }
}
