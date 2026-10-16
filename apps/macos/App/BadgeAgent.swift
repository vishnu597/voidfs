// SPDX-License-Identifier: Apache-2.0
//
// The agent's side of Finder's badges (`Badges/Badges.swift`): a second Mach service, for the
// Finder extension's code signature only, that asks the daemon's socket four fixed things built
// here from typed arguments. It is not a proxy: no path, method or body crosses from the
// extension, and a drive's pause or resume applies only to a drive mounted now.

import Foundation

final class BadgeListener: NSObject, NSXPCListenerDelegate, @unchecked Sendable {
    let socket: DaemonSocket
    /// The code signature a peer must have: the Finder extension's, or the app's own for its
    /// in-process self-test.
    let requirement: String

    init(socket: DaemonSocket, requirement: String = Badges.peerRequirement) { (self.socket, self.requirement) = (socket, requirement) }

    func listener(_ listener: NSXPCListener, shouldAcceptNewConnection connection: NSXPCConnection) -> Bool {
        guard connection.effectiveUserIdentifier == getuid() else {
            Agent.log.error("badges: refused a connection from uid \(connection.effectiveUserIdentifier), pid \(connection.processIdentifier)")
            return false
        }
        connection.setCodeSigningRequirement(requirement)
        connection.exportedInterface = Badges.interface()
        connection.exportedObject = BadgeConnection(socket: socket)
        connection.resume()
        return true
    }
}

final class BadgeConnection: NSObject, VoidfsBadgesProtocol, @unchecked Sendable {
    let socket: DaemonSocket
    private let lock = NSLock()
    private var inFlight = 0

    init(socket: DaemonSocket) { self.socket = socket }

    /// Off the connection's queue, a bounded number at a time, answered exactly once.
    private func run(fail: @escaping (NSError) -> Void, _ body: @escaping () throws -> Void) {
        let admitted = lock.withLock { () -> Bool in
            guard inFlight < Badges.maxCalls else { return false }
            inFlight += 1
            return true
        }
        guard admitted else { fail(Bridge.error(EAGAIN, "Busy", "too many badge calls in flight")); return }
        DispatchQueue.global(qos: .utility).async {
            defer { self.lock.withLock { self.inFlight -= 1 } }
            do { try body() } catch { fail(Self.nsError(error)) }
        }
    }

    /// The mounts API's errors carry a code and no errno: these are the ones a badge read meets.
    static func nsError(_ error: Error) -> NSError {
        guard let e = error as? DaemonCallError else { return BridgeConnection.nsError(error) }
        let o = (try? JSONSerialization.jsonObject(with: e.body)) as? [String: Any]
        let detail = o?["error"] as? [String: Any]
        let code = detail?["code"] as? String ?? "UnexpectedResponse"
        let message = detail?["message"] as? String ?? "the daemon answered \(e.status)"
        switch (code, e.status) {
        case ("NotOpen", _), ("NotFound", _), ("NoSuchUpload", _): return Bridge.error(ENOENT, code, message)
        case ("NotDir", _): return Bridge.error(ENOTDIR, code, message)
        case (_, 400): return Bridge.error(EINVAL, code, message)
        default: return Bridge.error(EIO, code, message)
        }
    }

    private static func validDrive(_ drive: String) -> Bool {
        let n = drive.utf8.count
        return n > 0 && n <= Badges.maxDrive && !drive.contains("/") && !drive.contains("\0")
    }

    private func json(_ method: String, _ target: String, _ body: [String: Any]? = nil) throws -> [String: Any] {
        let data = try body.map { try JSONSerialization.data(withJSONObject: $0) } ?? Data()
        let response = try socket.requestSync(method, target, generation: nil, contentType: body == nil ? nil : "application/json", body: data, limit: 1 << 20)
        guard (200..<300).contains(response.status) else { throw DaemonCallError(status: response.status, body: response.body) }
        guard let o = (try? JSONSerialization.jsonObject(with: response.body)) as? [String: Any] else { throw DaemonSocketError.malformed("JSON") }
        return o
    }

    /// The daemon's mount table (`/v1/mounts`), with each drive's pause from `/v1/status`.
    private func table() throws -> [BadgeMount] {
        let listed = try json("GET", "/v1/mounts")
        guard let mounts = listed["mounts"] as? [[String: Any]] else { throw DaemonSocketError.malformed("mounts") }
        let entries: [(String, String, String, Bool)] = try mounts.map {
            guard let drive = $0["drive"] as? String, let mountpoint = $0["mountpoint"] as? String, let state = $0["state"] as? String else { throw DaemonSocketError.malformed("mount") }
            return (drive, mountpoint, state, $0["readOnly"] as? Bool ?? false)
        }
        var all = false
        var drives: Set<String> = []
        if !entries.isEmpty {
            let uploads = try json("GET", "/v1/status")["uploads"] as? [String: Any]
            all = uploads?["paused"] as? Bool ?? false
            drives = Set(uploads?["pausedDrives"] as? [String] ?? [])
        }
        return entries.filter { $0.1.hasPrefix("/") }.map { BadgeMount(drive: $0.0, mountpoint: $0.1, state: $0.2, readOnly: $0.3, paused: all || drives.contains($0.0)) }
    }

    func mounts(reply: @escaping ([BadgeMount]?, NSError?) -> Void) {
        run(fail: { reply(nil, $0) }) { reply(try self.table(), nil) }
    }

    func states(drive: String, folder: String, after: String?, since: String?, reply: @escaping (BadgeFolder?, NSError?) -> Void) {
        run(fail: { reply(nil, $0) }) {
            guard Self.validDrive(drive) else { throw BridgeConnection.invalid("invalid drive name") }
            guard Badges.validFolder(folder) else { throw BridgeConnection.invalid("invalid folder") }
            if let after { try BridgeConnection.checkName(after) }
            if let since, since.utf8.count > Badges.maxVersion || since.contains("\0") { throw BridgeConnection.invalid("invalid version") }
            var body: [String: Any] = ["drive": drive, "folder": folder]
            if let after { body["after"] = after }
            if let since { body["since"] = since }
            let o = try self.json("POST", "/v1/mounts/states", body)
            guard let version = o["version"] as? String, let state = (o["state"] as? String).flatMap(Badges.sync),
                  let entries = o["entries"] as? [[Any]] else { throw DaemonSocketError.malformed("states") }
            var names: [String] = []
            var states: [BridgeAttr.SyncState] = []
            for entry in entries {
                guard entry.count == 2, let name = entry[0] as? String, let sync = (entry[1] as? String).flatMap(Badges.sync) else { throw DaemonSocketError.malformed("state") }
                names.append(name)
                states.append(sync)
            }
            reply(BadgeFolder(version: version, unchanged: o["unchanged"] as? Bool ?? false, state: state, names: names, states: states, next: o["next"] as? String), nil)
        }
    }

    func pause(drive: String, reply: @escaping (NSError?) -> Void) { control("pause", drive, reply) }

    func resume(drive: String, reply: @escaping (NSError?) -> Void) { control("resume", drive, reply) }

    /// A mounted drive's uploads only: the extension can't hold or release anything else.
    private func control(_ what: String, _ drive: String, _ reply: @escaping (NSError?) -> Void) {
        run(fail: { reply($0) }) {
            guard Self.validDrive(drive) else { throw BridgeConnection.invalid("invalid drive name") }
            guard try self.table().contains(where: { $0.drive == drive && $0.state == "mounted" }) else { throw Bridge.error(ENOENT, "NotMounted", "\(drive) isn't mounted") }
            _ = try self.json("POST", "/v1/uploads/\(what)", ["drive": drive])
            reply(nil)
        }
    }
}
