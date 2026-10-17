// SPDX-License-Identifier: Apache-2.0
//
// Finder's sync badges (step 5, item 8): the Finder Sync extension's XPC interface to the app's
// launch agent, its client, and the model behind the extension. The agent answers on a Mach
// service of its own, accepts only the Finder extension's code signature, and asks the daemon for
// three things only: the mounted drives (`/v1/status`, as the menu reads them), one folder's sync
// states (`POST /v1/mounts/states`, read from the daemon's state alone, never the server), and a
// mounted drive's pause or resume, as the menu has them. The extension keeps no state of its own:
// every badge comes from the daemon, and goes when the daemon doesn't answer.

import Foundation

enum Badges {
    /// The agent's service for the Finder extension: App-Group-prefixed, so the sandbox reaches it.
    static let service = "\(Bridge.appGroup).badges"
    /// Who the agent accepts on it: the Finder extension, signed by voidfs's team.
    static let peerRequirement = "anchor apple generic and certificate leaf[subject.OU] = \"HAUTK68F56\" and identifier \"dev.voidfs.app.findersync\""
    /// The daemon's bounds (`crates/voidfs-daemon/src/mounts.rs`), checked before a call crosses
    /// the socket.
    static let maxFolder = 1024
    static let maxName = 255
    static let maxDrive = 1024
    static let maxVersion = 256
    /// Per Finder extension connection.
    static let maxCalls = 16

    static func interface() -> NSXPCInterface {
        let i = NSXPCInterface(with: VoidfsBadgesProtocol.self)
        let mounts = NSSet(array: [NSArray.self, BadgeMount.self]) as! Set<AnyHashable>
        i.setClasses(mounts, for: #selector(VoidfsBadgesProtocol.mounts(reply:)), argumentIndex: 0, ofReply: true)
        return i
    }

    /// The badge for a state: none for a file kept on this Mac only, or a state not known.
    static func identifier(_ sync: BridgeAttr.SyncState?) -> String {
        switch sync {
        case .saved?: "saved"
        case .pending?: "pending"
        case .saving?: "saving"
        case .error?: "error"
        case .conflict?: "conflict"
        case .kept?: "kept"
        case .local?, nil: ""
        }
    }

    static func sync(_ text: String) -> BridgeAttr.SyncState? {
        switch text {
        case "Saved": .saved
        case "Pending": .pending
        case "Saving": .saving
        case "Conflict": .conflict
        case "Error": .error
        case "Local": .local
        case "Kept": .kept
        default: nil
        }
    }

    /// A folder's path below its drive's top as the daemon takes it: names of 1 to 255 bytes,
    /// none `.` or `..`, joined by `/`; `""` is the top.
    static func validFolder(_ folder: String) -> Bool {
        guard folder.utf8.count <= maxFolder, !folder.contains("\0") else { return false }
        if folder.isEmpty { return true }
        return folder.split(separator: "/", omittingEmptySubsequences: false).allSatisfy { $0.utf8.count >= 1 && $0.utf8.count <= maxName && $0 != "." && $0 != ".." }
    }
}

/// What the agent exports to the Finder extension.
@objc protocol VoidfsBadgesProtocol {
    /// The daemon's mounts, mounted or not, and whether each drive's uploads are paused.
    func mounts(reply: @escaping ([BadgeMount]?, NSError?) -> Void)
    /// One page of a folder's sync states. `since`, a first page's version, answers only
    /// `unchanged` while it holds.
    func states(drive: String, folder: String, after: String?, since: String?, reply: @escaping (BadgeFolder?, NSError?) -> Void)
    /// Pauses or resumes a mounted drive's uploads, as the menu does. Cancel is the menu bar's
    /// only, where it says what happens: the change stays on this Mac, not uploaded.
    func pause(drive: String, reply: @escaping (NSError?) -> Void)
    func resume(drive: String, reply: @escaping (NSError?) -> Void)
}

/// One entry of the daemon's mount table.
@objc(VoidfsBadgeMount) final class BadgeMount: NSObject, NSSecureCoding {
    let drive: String
    let mountpoint: String
    /// `mounting`, `mounted` or `failed`.
    let state: String
    let readOnly: Bool
    /// Its uploads are held: everything is paused, or this drive.
    let paused: Bool

    init(drive: String, mountpoint: String, state: String, readOnly: Bool, paused: Bool) {
        (self.drive, self.mountpoint, self.state, self.readOnly, self.paused) = (drive, mountpoint, state, readOnly, paused)
    }

    static var supportsSecureCoding: Bool { true }

    func encode(with c: NSCoder) {
        c.encode(drive, forKey: "d")
        c.encode(mountpoint, forKey: "m")
        c.encode(state, forKey: "s")
        c.encode(readOnly, forKey: "r")
        c.encode(paused, forKey: "p")
    }

    init?(coder c: NSCoder) {
        guard let drive = c.decodeObject(of: NSString.self, forKey: "d") as String?, let mountpoint = c.decodeObject(of: NSString.self, forKey: "m") as String?,
              let state = c.decodeObject(of: NSString.self, forKey: "s") as String?, mountpoint.hasPrefix("/") else { return nil }
        (self.drive, self.mountpoint, self.state, self.readOnly, self.paused) = (drive, mountpoint, state, c.decodeBool(forKey: "r"), c.decodeBool(forKey: "p"))
    }
}

/// One page of the daemon's `FolderStates`: the entries that aren't saved.
@objc(VoidfsBadgeFolder) final class BadgeFolder: NSObject, NSSecureCoding {
    let version: String
    let unchanged: Bool
    let state: BridgeAttr.SyncState
    let names: [String]
    let states: [BridgeAttr.SyncState]
    let next: String?

    init(version: String, unchanged: Bool, state: BridgeAttr.SyncState, names: [String], states: [BridgeAttr.SyncState], next: String?) {
        (self.version, self.unchanged, self.state) = (version, unchanged, state)
        (self.names, self.states, self.next) = (names, states, next)
    }

    static var supportsSecureCoding: Bool { true }

    func encode(with c: NSCoder) {
        c.encode(version, forKey: "v")
        c.encode(unchanged, forKey: "u")
        c.encode(state.rawValue, forKey: "s")
        c.encode(names as NSArray, forKey: "n")
        c.encode(states.map { NSNumber(value: $0.rawValue) } as NSArray, forKey: "y")
        c.encode(next, forKey: "x")
    }

    init?(coder c: NSCoder) {
        guard let version = c.decodeObject(of: NSString.self, forKey: "v") as String?, let state = BridgeAttr.SyncState(rawValue: c.decodeInteger(forKey: "s")),
              let names = c.decodeArrayOfObjects(ofClass: NSString.self, forKey: "n")?.map({ $0 as String }),
              let raw = c.decodeArrayOfObjects(ofClass: NSNumber.self, forKey: "y"), names.count == raw.count else { return nil }
        let states = raw.compactMap { BridgeAttr.SyncState(rawValue: $0.intValue) }
        guard states.count == raw.count else { return nil }
        (self.version, self.unchanged, self.state) = (version, c.decodeBool(forKey: "u"), state)
        (self.names, self.states, self.next) = (names, states, c.decodeObject(of: NSString.self, forKey: "x") as String?)
    }
}

/// The Finder extension's connection to the agent, made again after the agent goes.
final class BadgeClient: @unchecked Sendable {
    private let lock = NSLock()
    private let make: () -> NSXPCConnection
    private var connection: NSXPCConnection?

    convenience init(service: String = Badges.service) { self.init { NSXPCConnection(machServiceName: service) } }

    /// An in-process listener, for the app's self-test.
    convenience init(endpoint: NSXPCListenerEndpoint) { self.init { NSXPCConnection(listenerEndpoint: endpoint) } }

    init(make: @escaping () -> NSXPCConnection) { self.make = make }

    deinit { connection?.invalidate() }

    private func current() -> NSXPCConnection {
        lock.lock(); defer { lock.unlock() }
        if let connection { return connection }
        let c = make()
        c.remoteObjectInterface = Badges.interface()
        c.interruptionHandler = { [weak self, weak c] in self?.lost(c) }
        c.invalidationHandler = { [weak self, weak c] in self?.lost(c) }
        c.resume()
        connection = c
        return c
    }

    private func lost(_ c: NSXPCConnection?) {
        lock.withLock { if connection === c { connection = nil } }
        c?.invalidate()
    }

    /// One call, answered once: by its reply or by the connection's error handler.
    private func send<T>(_ call: (VoidfsBadgesProtocol, @escaping (Result<T, Error>) -> Void) -> Void) async throws -> T {
        let c = current()
        return try await withCheckedThrowingContinuation { continuation in
            let proxy = c.remoteObjectProxyWithErrorHandler { continuation.resume(throwing: $0) } as! VoidfsBadgesProtocol
            call(proxy) { continuation.resume(with: $0) }
        }
    }

    private static func done(_ e: NSError?) -> Result<Void, Error> { e.map { .failure($0) } ?? .success(()) }

    func mounts() async throws -> [BadgeMount] {
        try await send { p, done in p.mounts { done(BridgeClient.result($0, $1)) } }
    }

    func states(drive: String, folder: String, after: String?, since: String?) async throws -> BadgeFolder {
        try await send { p, done in p.states(drive: drive, folder: folder, after: after, since: since) { done(BridgeClient.result($0, $1)) } }
    }

    func pause(drive: String) async throws { try await send { p, done in p.pause(drive: drive) { done(Self.done($0)) } } }

    func resume(drive: String) async throws { try await send { p, done in p.resume(drive: drive) { done(Self.done($0)) } } }
}

/// What the Finder extension shows, all of it from the daemon through the agent. Finder asks for
/// one item's badge at a time; the model answers from the item's folder, read in one call (or a
/// page per 1,024 unsaved entries), and reads again only the folders Finder shows or asked about
/// lately, each second, with the version that makes an unchanged folder cost one short answer.
/// A folder's entries that the daemon doesn't name are saved: whatever isn't is a change made on
/// this Mac, which the daemon always names. A folder it can't find has no badges.
final class BadgeModel: @unchecked Sendable {
    struct Mount: Equatable {
        let drive: String
        let mountpoint: String
        let readOnly: Bool
        let paused: Bool
    }

    private struct Key: Hashable { let mountpoint: String; let folder: String }

    private final class Folder {
        let drive: String
        var version: String?
        /// The daemon found the folder.
        var found = false
        var state: BridgeAttr.SyncState = .saved
        var entries: [String: BridgeAttr.SyncState] = [:]
        var loaded = false
        /// What Finder asked about in this folder, with the URL it named each by.
        var asked: [String: URL] = [:]
        /// The mount's top itself, for the folder `""`.
        var top: URL?
        var observers = 0
        var used = ContinuousClock.now
        var loading = false
        var again = false

        init(drive: String) { self.drive = drive }
    }

    /// The folders' states, as read.
    enum Read { case unchanged, changed(version: String, state: BridgeAttr.SyncState, entries: [String: BridgeAttr.SyncState]), unknown, failed }

    static let every: Duration = .seconds(1)
    static let mountsEvery: Duration = .seconds(3)
    /// A folder Finder no longer shows is read while it was asked about this lately, and kept
    /// this long.
    static let recent: Duration = .seconds(30)
    static let keep: Duration = .seconds(120)

    let client: BadgeClient
    private let setDirectories: (Set<URL>) -> Void
    private let setBadge: (String, URL) -> Void
    private let queue = DispatchQueue(label: "dev.voidfs.badges")
    private var mounts: [Mount] = []
    private var folders: [Key: Folder] = [:]
    private var timers: [DispatchSourceTimer] = []
    private var mountFailures = 0
    /// The self-test's mounts, in place of the daemon's.
    private let fixed: [Mount]?

    /// Calls to the agent, and how many folder reads `since` answered at once.
    private(set) var calls = 0
    private(set) var unchanged = 0

    init(client: BadgeClient, mounts fixed: [Mount]? = nil, setDirectories: @escaping (Set<URL>) -> Void, setBadge: @escaping (String, URL) -> Void) {
        (self.client, self.fixed, self.setDirectories, self.setBadge) = (client, fixed, setDirectories, setBadge)
    }

    func start() {
        queue.async {
            self.refreshMounts()
            self.timers = [self.timer(Self.mountsEvery) { $0.refreshMounts() }, self.timer(Self.every) { $0.tick() }]
        }
    }

    func stop() { queue.sync { for t in timers { t.cancel() }; timers = [] } }

    private func timer(_ every: Duration, _ body: @escaping (BadgeModel) -> Void) -> DispatchSourceTimer {
        let t = DispatchSource.makeTimerSource(queue: queue)
        let ms = Int(every.components.seconds * 1000)
        t.schedule(deadline: .now() + .milliseconds(ms), repeating: .milliseconds(ms), leeway: .milliseconds(100))
        t.setEventHandler { [weak self] in if let self { body(self) } }
        t.resume()
        return t
    }

    var counts: (calls: Int, unchanged: Int) { queue.sync { (calls, unchanged) } }

    var currentMounts: [Mount] { queue.sync { mounts } }

    /// The folders read each second now.
    var polledFolders: Int {
        queue.sync {
            let now = ContinuousClock.now
            return folders.values.filter { $0.observers > 0 || now - $0.used < Self.recent }.count
        }
    }

    // MARK: Finder's calls

    /// Finder asks for one item's badge; it gets it now if its folder was read, or once it is.
    func request(_ url: URL) {
        queue.async {
            guard let (mount, folder, name) = self.locate(url.path) else { return }
            let f = self.folder(mount, folder)
            f.used = .now
            if let name { f.asked[name] = url } else { f.top = url }
            if f.loaded { self.setBadge(Self.badge(f, name), url) } else { self.load(Key(mountpoint: mount.mountpoint, folder: folder)) }
        }
    }

    /// Finder shows a folder's items: read it each second while it does.
    func observe(_ url: URL) {
        queue.async {
            guard let (mount, folder) = self.folderAt(url.path) else { return }
            let f = self.folder(mount, folder)
            f.observers += 1
            f.used = .now
            self.load(Key(mountpoint: mount.mountpoint, folder: folder))
        }
    }

    func unobserve(_ url: URL) {
        queue.async {
            guard let (mount, folder) = self.folderAt(url.path), let f = self.folders[Key(mountpoint: mount.mountpoint, folder: folder)] else { return }
            f.observers = max(0, f.observers - 1)
            f.used = .now
        }
    }

    /// The mount an item is on, for the context menu.
    func mount(of url: URL) -> Mount? { queue.sync { locate(url.path)?.0 } }

    func pause(_ mount: Mount) async throws { try await client.pause(drive: mount.drive); queue.async { self.refreshMounts() } }

    func resume(_ mount: Mount) async throws { try await client.resume(drive: mount.drive); queue.async { self.refreshMounts() } }

    /// The badge an item has now, as Finder was last told, for the checks.
    func badge(_ url: URL) -> String? {
        queue.sync {
            guard let (mount, folder, name) = locate(url.path), let f = folders[Key(mountpoint: mount.mountpoint, folder: folder)], f.loaded else { return nil }
            return Self.badge(f, name)
        }
    }

    // MARK: Paths

    /// A mountpoint's spellings: `/tmp` and `/var` are `/private`'s, and Finder may use either.
    static func spellings(_ path: String) -> [String] {
        if path.hasPrefix("/private/") { return [path, String(path.dropFirst("/private".count))] }
        if ["/tmp", "/var", "/etc"].contains(where: { path == $0 || path.hasPrefix($0 + "/") }) { return [path, "/private" + path] }
        return [path]
    }

    /// Where a path is: its mount, the folder that holds it (below the mount's top), and its name
    /// there. The mount's top itself has no name.
    private func locate(_ path: String) -> (Mount, String, String?)? {
        let path = path.count > 1 && path.hasSuffix("/") ? String(path.dropLast()) : path
        for mount in mounts.sorted(by: { $0.mountpoint.count > $1.mountpoint.count }) {
            for root in Self.spellings(mount.mountpoint) {
                if path == root { return (mount, "", nil) }
                guard path.hasPrefix(root + "/") else { continue }
                var names = path.dropFirst(root.count + 1).split(separator: "/").map(String.init)
                guard let name = names.popLast() else { return (mount, "", nil) }
                let folder = names.joined(separator: "/")
                return Badges.validFolder(folder) ? (mount, folder, name) : nil
            }
        }
        return nil
    }

    /// A folder Finder shows: its mount, and its path below the mount's top.
    private func folderAt(_ path: String) -> (Mount, String)? {
        guard let (mount, parent, name) = locate(path) else { return nil }
        guard let name else { return (mount, "") }
        let folder = parent.isEmpty ? name : parent + "/" + name
        return Badges.validFolder(folder) ? (mount, folder) : nil
    }

    private func folder(_ mount: Mount, _ folder: String) -> Folder {
        let key = Key(mountpoint: mount.mountpoint, folder: folder)
        if let f = folders[key] { return f }
        let f = Folder(drive: mount.drive)
        folders[key] = f
        return f
    }

    private static func badge(_ f: Folder, _ name: String?) -> String {
        guard f.found else { return Badges.identifier(nil) }
        guard let name else { return Badges.identifier(f.state) }
        return Badges.identifier(f.entries[name] ?? .saved)
    }

    // MARK: Reading

    private func tick() {
        let now = ContinuousClock.now
        for (key, f) in folders {
            if f.observers == 0, now - f.used > Self.keep { folders[key] = nil; continue }
            if f.observers > 0 || now - f.used < Self.recent { load(key) }
        }
    }

    private func load(_ key: Key) {
        guard let f = folders[key] else { return }
        if f.loading { f.again = true; return }
        f.loading = true
        let (drive, since) = (f.drive, f.loaded ? f.version : nil)
        Task {
            let read = await self.read(drive: drive, folder: key.folder, since: since)
            self.queue.async { self.apply(key, read) }
        }
    }

    /// Every page of a folder, or `unchanged`. Pages read at another version start again.
    func read(drive: String, folder: String, since: String?) async -> Read {
        do {
            for _ in 0..<3 {
                let first = try await client.states(drive: drive, folder: folder, after: nil, since: since)
                let same = first.unchanged
                queue.async { self.calls += 1; if same { self.unchanged += 1 } }
                if first.unchanged { return .unchanged }
                var entries = Dictionary(zip(first.names, first.states), uniquingKeysWith: { a, _ in a })
                var next = first.next
                var consistent = true
                while let after = next {
                    let page = try await client.states(drive: drive, folder: folder, after: after, since: nil)
                    queue.async { self.calls += 1 }
                    if page.version != first.version { consistent = false; break }
                    for (name, state) in zip(page.names, page.states) { entries[name] = state }
                    next = page.next
                }
                if consistent { return .changed(version: first.version, state: first.state, entries: entries) }
            }
            return .failed
        } catch {
            let e = error as NSError
            // The folder is gone, the drive isn't open, or no daemon answers: nothing is known.
            if e.domain == NSPOSIXErrorDomain, [ENOENT, ENOTDIR, ECONNREFUSED].contains(Int32(e.code)) { return .unknown }
            return .failed
        }
    }

    private func apply(_ key: Key, _ read: Read) {
        guard let f = folders[key] else { return }
        f.loading = false
        // What Finder was told, so that only a change is told again.
        let before = f.loaded ? Dictionary(uniqueKeysWithValues: f.asked.keys.map { ($0, Self.badge(f, $0)) }) : nil
        let topBefore = f.loaded ? Self.badge(f, nil) : nil
        switch read {
        case .unchanged, .failed: break
        case let .changed(version, state, entries):
            (f.version, f.found, f.state, f.entries, f.loaded) = (version, true, state, entries, true)
        case .unknown:
            (f.version, f.found, f.state, f.entries, f.loaded) = (nil, false, .saved, [:], true)
        }
        if f.loaded {
            for (name, url) in f.asked {
                let now = Self.badge(f, name)
                if before?[name] != now { setBadge(now, url) }
            }
            if let top = f.top {
                let now = Self.badge(f, nil)
                if topBefore != now { setBadge(now, top) }
            }
        }
        if f.again { f.again = false; load(key) }
    }

    private func refreshMounts() {
        if let fixed { publish(fixed); return }
        Task {
            do {
                let all = try await client.mounts()
                let mounted = all.filter { $0.state == "mounted" }.map { Mount(drive: $0.drive, mountpoint: $0.mountpoint, readOnly: $0.readOnly, paused: $0.paused) }
                queue.async { self.mountFailures = 0; self.publish(mounted) }
            } catch {
                let e = error as NSError
                // No daemon: no mount is served. Anything else may pass: forget them on the second.
                queue.async {
                    self.mountFailures += 1
                    if (e.domain == NSPOSIXErrorDomain && Int32(e.code) == ECONNREFUSED) || self.mountFailures >= 2 { self.publish([]) }
                }
            }
        }
    }

    private func publish(_ now: [Mount]) {
        let gone = Set(mounts.map(\.mountpoint)).subtracting(now.map(\.mountpoint))
        let changed = Set(mounts.map(\.mountpoint)) != Set(now.map(\.mountpoint))
        mounts = now
        for (key, f) in folders where gone.contains(key.mountpoint) {
            for url in f.asked.values { setBadge("", url) }
            if let top = f.top { setBadge("", top) }
            folders[key] = nil
        }
        if changed { setDirectories(Set(now.flatMap { Self.spellings($0.mountpoint) }.map { URL(filePath: $0, directoryHint: .isDirectory) })) }
    }
}
