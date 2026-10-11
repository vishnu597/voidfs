// SPDX-License-Identifier: Apache-2.0
//
// A drive as a read-only FSKit volume, served by the daemon through the app's launch agent
// (Shared/BridgeClient.swift). The daemon's session answers every call; its invalidation relay says
// what changed, here or on other machines, and the volume makes the kernel forget it with revokes.

import FSKit
import Foundation
import os

func posix(_ code: Int32) -> Error { fs_errorForPOSIXError(code) }

/// Mount options, from `mount -o k=v,…` or the activate options.
struct VolumeOptions {
    /// `f_iosize`. Measured on macOS 27.0: the kernel sends reads of at most 1 MiB whatever this
    /// says (256 KiB, 4 MiB and 16 MiB were tried).
    var ioSize = 1 << 20
    var uid = getuid()
    var gid = getgid()

    mutating func apply(_ args: [String]) {
        var i = 0
        while i < args.count {
            let value = i + 1 < args.count ? args[i + 1] : ""
            switch args[i] {
            case "-u": uid = uid_t(value) ?? uid; i += 1
            case "-g": gid = gid_t(value) ?? gid; i += 1
            case "-o":
                for kv in value.split(separator: ",") {
                    let p = kv.split(separator: "=", maxSplits: 1).map(String.init)
                    if p.first == "iosize", p.count > 1 { ioSize = Int(p[1]) ?? ioSize }
                }
                i += 1
            default: break
            }
            i += 1
        }
    }
}

/// Names looked up in a folder and not found, up to a bound; past it, any name.
struct Missed {
    var names: Set<String> = []
    var any = false

    mutating func insert(_ name: String) {
        if any { return }
        if names.count >= 16_384 { (names, any) = ([], true) } else { names.insert(name) }
    }

    func matches(_ changed: Set<String>) -> Bool { any || !names.isDisjoint(with: changed) }
}

/// A folder's entries as of one enumeration, at one namespace generation. Every invalidation
/// names the folders on a changed key's path, so a listing kept until one names its folder
/// answers lookups too, misses included (the spike's listing did; `ls -l` and Finder look up an
/// AppleDouble `._name` for every file).
struct Listing {
    var names: [String] = []
    var attrs: [BridgeAttr] = []
    var generation: UInt64 = 0
    /// Each name's position, by its NFC form, as the daemon matches names; -1 where two share one.
    var index: [String: Int] = [:]

    mutating func indexNames() {
        index.reserveCapacity(names.count)
        for (i, name) in names.enumerated() {
            let key = name.precomposedStringWithCanonicalMapping
            index[key] = index[key] == nil ? i : -1
        }
    }
}

final class VoidfsVolume: FSVolume, FSVolume.Handler, FSVolume.ReadWriteHandler, FSVolume.XattrHandler, FSVolume.DataCacheHandler, @unchecked Sendable {
    let client: BridgeClient
    let drive: String
    let root: UInt64
    var options = VolumeOptions()
    private let log = Logger(subsystem: "dev.voidfs", category: "fs")
    private let rootItem: VoidfsItem
    private let caseSensitive: Bool

    // Everything below is guarded by `lock`, which is never held across an await or a call into
    // FSKit (setCacheState may call back into the volume).
    private let lock = NSLock()
    private var session: BridgeSession
    private var connecting: Task<BridgeSession, Error>?
    private var lastAttempt: (ContinuousClock.Instant, Error)?
    /// The current incarnation of each inode the kernel was given.
    private var items: [UInt64: VoidfsItem] = [:]
    /// Incarnations, current or retired, with a daemon handle open.
    private var open: [ObjectIdentifier: VoidfsItem] = [:]
    private var listings: [UInt64: Listing] = [:]
    private var listed = 0
    /// Counts invalidations; a listing fetched across one isn't kept.
    private var epoch: UInt64 = 0
    /// Names in each folder the kernel was told are missing, which it may have cached.
    private var misses: [UInt64: Missed] = [:]
    private var missedNames = 0
    /// Folders whose purge waits for files open below them to close.
    private var pending: Set<UInt64> = []
    private var unmounted = false
    private var offline = false
    /// The feed's position, as the relay's events carry it.
    private var seq: UInt64 = 0
    private var revoked = (ok: 0, benign: 0, failed: 0)

    /// Revokes run in order, here and nowhere else.
    private let revokes = DispatchQueue(label: "dev.voidfs.fs.revoke")
    private static let listingLimit = 200_000
    private static let missesLimit = 200_000

    init(client: BridgeClient, session: BridgeSession, drive: String, volumeID: FSVolume.Identifier) {
        self.client = client
        self.session = session
        self.drive = drive
        root = session.info.root
        rootItem = VoidfsItem(ino: root, id: .rootDirectory, parent: 0, kind: .folder)
        let capabilities = (try? JSONSerialization.jsonObject(with: session.info.capabilities)) as? [String: Any]
        caseSensitive = capabilities?["caseSensitive"] as? Bool ?? true
        super.init(volumeID: volumeID, volumeName: FSFileName(string: session.info.drive))
        session.listen { [weak self, weak session] event in
            guard let self, let session else { return }
            self.relay(event, from: session)
        }
    }

    // MARK: - Properties FSKit reads

    var maximumLinkCount: Int { 1 }
    var maximumNameLength: Int { Bridge.maxName }
    var restrictsOwnershipChanges: Bool { true }
    var truncatesLongNames: Bool { false }
    var maximumXattrSize: Int { Bridge.maxXattr }
    var maximumFileSize: UInt64 { 5 << 40 }
    var requestedMountOptions: FSVolume.MountOptions { .readOnly }

    var supportedVolumeCapabilities: FSVolume.SupportedCapabilities {
        let c = FSVolume.SupportedCapabilities()
        c.supportsSymbolicLinks = true
        c.supportsHardLinks = false
        c.supports2TBFiles = true
        c.supports64BitObjectIDs = true
        c.supportsPersistentObjectIDs = true
        c.supportsFastStatFS = true
        c.supportsHiddenFiles = true
        c.doesNotSupportSettingFilePermissions = true
        c.caseFormat = caseSensitive ? .sensitive : .insensitive
        return c
    }

    var volumeStatistics: FSStatFSResult {
        let s = FSStatFSResult(fileSystemTypeName: "voidfs")
        s.blockSize = 4096
        s.ioSize = options.ioSize
        let total: UInt64 = 1 << 50
        s.totalBytes = total
        s.usedBytes = 0
        s.availableBytes = total
        s.freeBytes = total
        s.totalFiles = 1 << 32
        s.freeFiles = 1 << 31
        return s
    }

    // MARK: - Life cycle

    func activate(options taskOptions: FSTaskOptions) async throws -> FSActivateResult {
        options.apply(taskOptions.taskOptions)
        log.notice("activate \(taskOptions.taskOptions, privacy: .public): drive \(self.drive, privacy: .public), root inode \(self.root); wanted attributes: lookup \(String(FSLookupItemResult.requestedAttributes.wantedAttributes.rawValue, radix: 16), privacy: .public), getattr \(String(FSGetAttributesResult.requestedAttributes.wantedAttributes.rawValue, radix: 16), privacy: .public), read \(String(FSReadFileResult.requestedAttributes.wantedAttributes.rawValue, radix: 16), privacy: .public)")
        return FSActivateResult(rootItem: rootItem)!
    }

    func deactivate(options: FSDeactivateOptions = []) async throws {
        log.notice("deactivate")
    }

    // macOS 27.0 (26A428) sends `activateVolumeWithOptions:` and `deactivateVolumeWithOptions:`,
    // while the Xcode 27 beta SDK (26A5353p) declares `activateWithOptions:` and
    // `deactivateWithOptions:`. Answer to both until the SDK and the OS agree. Rechecked on
    // macOS 27.0.1 (26A434) with Xcode 27 beta 27A5194q: still needed, the first mount without
    // it crashes on an unrecognized selector.
    @objc(activateVolumeWithOptions:replyHandler:)
    func activateVolume(options: FSTaskOptions, replyHandler reply: @escaping (FSActivateResult?, Error?) -> Void) {
        log.notice("activate arrived as activateVolumeWithOptions:")
        Task {
            do { reply(try await activate(options: options), nil) } catch { reply(nil, error) }
        }
    }

    @objc(deactivateVolumeWithOptions:replyHandler:)
    func deactivateVolume(options: FSDeactivateOptions, replyHandler reply: @escaping (Error?) -> Void) {
        Task {
            do { try await deactivate(options: options); reply(nil) } catch { reply(error) }
        }
    }

    func mount(options taskOptions: FSTaskOptions) async throws {
        log.notice("mount \(taskOptions.taskOptions, privacy: .public)")
    }

    func unmount() async {
        let s: BridgeSession = lock.withLock {
            unmounted = true
            connecting?.cancel()
            return session
        }
        let stats = lock.withLock { revoked }
        log.notice("unmount: revokes \(stats.ok) ok, \(stats.benign) of items the kernel had let go, \(stats.failed) failed")
        // The daemon may be gone (it unmounts as it stops): don't hold the unmount up.
        let release = Task { try? await s.release() }
        _ = await withTaskGroup(of: Void.self) { group in
            group.addTask { await release.value }
            group.addTask { try? await Task.sleep(for: .seconds(2)) }
            await group.next()
            group.cancelAll()
        }
    }

    func synchronize(flags: FSSyncFlags) async throws {}

    // MARK: - The session

    /// The live session: the current one while its relay runs, otherwise a new one. Without its
    /// relay the volume would miss invalidations, so a session whose relay ended is replaced, and
    /// everything the kernel holds is revoked once the new one's relay is live.
    private func live() async throws -> BridgeSession {
        let (current, task, failed): (BridgeSession?, Task<BridgeSession, Error>?, Error?) = lock.withLock {
            if unmounted { return (nil, nil, posix(ENXIO)) }
            if session.isWatching { return (session, nil, nil) }
            if let connecting { return (nil, connecting, nil) }
            // Calls fail fast between attempts while the daemon or the agent is away.
            if let (at, error) = lastAttempt, ContinuousClock.now - at < .milliseconds(250) { return (nil, nil, error) }
            let t = Task { try await self.reconnect() }
            connecting = t
            return (nil, t, nil)
        }
        if let current { return current }
        if let failed { throw failed }
        return try await task!.value
    }

    private func reconnect() async throws -> BridgeSession {
        do {
            let s = try await client.open(drive: drive, readOnly: true)
            guard s.info.root == root else {
                try? await s.release()
                throw Bridge.error(ESTALE, "OtherDrive", "the drive's root changed: mount it again")
            }
            s.listen { [weak self, weak s] event in
                guard let self, let s else { return }
                self.relay(event, from: s)
            }
            let old: BridgeSession = lock.withLock {
                defer { session = s; connecting = nil; lastAttempt = nil }
                return session
            }
            log.notice("reconnected to the daemon: session generation \(s.info.generation)")
            try? await old.release()
            // What changed while the relay was down is unknown.
            resync("the relay was down")
            return s
        } catch {
            lock.withLock { connecting = nil; lastAttempt = (.now, error) }
            log.error("reconnecting to the daemon: \(error.localizedDescription, privacy: .public)")
            throw error
        }
    }

    /// Keeps reconnecting while nothing else calls, so invalidations resume.
    private func keepConnected() {
        Task.detached { [weak self] in
            var delay: Duration = .milliseconds(250)
            while let self, !self.lock.withLock({ self.unmounted }) {
                if (try? await self.live()) != nil { return }
                try? await Task.sleep(for: delay)
                delay = min(delay * 2, .seconds(5))
            }
        }
    }

    /// Runs `call` on the live session; a session lost meanwhile is replaced once.
    private func call<T>(_ body: (BridgeSession) async throws -> T) async throws -> T {
        for attempt in 0..<2 {
            let s: BridgeSession
            do { s = try await live() } catch { throw failure(error) }
            do {
                return try await body(s)
            } catch {
                let e = Bridge.errno(error)
                if e == ESTALE, attempt == 0, !s.isWatching { continue }
                if e == ENETDOWN, lock.withLock({ () -> Bool in defer { offline = true }; return !offline }) {
                    log.notice("offline: the daemon can't reach the server; serving what it has cached")
                }
                throw failure(error)
            }
        }
        throw posix(ESTALE)
    }

    /// The errno FSKit returns. The daemon or the agent being away is `EIO` to the caller.
    private func failure(_ error: Error) -> Error {
        let e = Bridge.errno(error)
        return posix(e == ECONNREFUSED ? EIO : e)
    }

    // MARK: - Invalidations

    private func relay(_ event: BridgeEvent?, from s: BridgeSession) {
        guard lock.withLock({ session === s && !unmounted }) else { return }
        guard let event else {
            log.notice("the invalidation relay ended; reconnecting")
            keepConnected()
            return
        }
        // The server's feed moved on (local changes don't advance it): an offline mount is current.
        let back = lock.withLock { () -> Bool in
            defer { seq = max(seq, event.seq) }
            guard offline, event.seq > seq, !event.resync else { return false }
            offline = false
            return true
        }
        if back { log.notice("the feed delivered changes again at sequence \(event.seq): the mount is current again") }
        if event.resync || event.all {
            // The feed reconnected or lost track: a missed generation means relisting everything.
            resync("resync at generation \(event.generation)")
            return
        }
        invalidate(event)
    }

    /// Makes the kernel forget what `event` names. Changed files and symbolic links are retired and
    /// revoked, so their next lookup makes a new incarnation, and each folder named is revoked
    /// after them. The kernel also caches a name it was told is missing, until a revoke of the
    /// folder takes effect, which happens only once the kernel holds nothing below the folder:
    /// a vnode pins its parent. So when a changed name was looked up and missed in a folder, the
    /// folder's whole known subtree is revoked first, deepest first. Files open below it (or a
    /// working directory in it) still hold it: its purge is tried again as they close.
    private func invalidate(_ event: BridgeEvent) {
        let changed = Set((event.objects + event.subtrees).flatMap(Self.components))
        let targets: [VoidfsItem] = lock.withLock {
            epoch += 1
            var files: [VoidfsItem] = []
            var folders: [VoidfsItem] = []
            var purged: [VoidfsItem] = []
            for ino in event.inodes {
                if let l = listings.removeValue(forKey: ino) { listed -= l.names.count }
                guard let item = ino == root ? rootItem : items[ino] else { continue }
                if item.kind == .folder {
                    folders.append(item)
                    if let missed = misses[ino], missed.matches(changed) {
                        misses[ino] = nil
                        if ino == root {
                            log.notice("a name missed in the drive's top folder now exists: it shows by path once the drive is mounted again")
                        } else {
                            purged += subtree(ino)
                            if openBelow(ino) { pending.insert(ino) }
                        }
                    }
                } else {
                    retire(item)
                    files.append(item)
                }
            }
            var seen = Set<ObjectIdentifier>()
            return (files + purged + folders).filter { seen.insert(ObjectIdentifier($0)).inserted }
        }
        revoke(targets)
    }

    /// The names along a changed key: any of them may be a name that now exists.
    static func components(_ key: String) -> [String] {
        key.split(separator: "/").map(String.init)
    }

    /// Everything the kernel may hold below `folder`, deepest first. Called with the lock held.
    private func subtree(_ folder: UInt64) -> [VoidfsItem] {
        var children: [UInt64: [VoidfsItem]] = [:]
        for item in items.values { children[item.parent, default: []].append(item) }
        for item in open.values where item.retired { children[item.parent, default: []].append(item) }
        var out: [VoidfsItem] = []
        var seen = Set<ObjectIdentifier>()
        func walk(_ ino: UInt64, _ depth: Int) {
            guard depth < 256 else { return }
            for child in children[ino] ?? [] where seen.insert(ObjectIdentifier(child)).inserted {
                if child.kind == .folder { walk(child.ino, depth + 1) }
                out.append(child)
            }
        }
        walk(folder, 0)
        return out
    }

    /// Whether a file is open below `folder`. Called with the lock held.
    private func openBelow(_ folder: UInt64) -> Bool {
        open.values.contains { ancestors($0).contains(folder) }
    }

    /// Called with the lock held.
    private func ancestors(_ item: VoidfsItem) -> [UInt64] {
        var out: [UInt64] = []
        var at = item.parent
        while out.count < 256 {
            out.append(at)
            guard at != root, let up = items[at] else { break }
            at = up.parent
        }
        return out
    }

    /// Everything the kernel holds is forgotten, deepest first. The daemon's feed resyncs when it
    /// reconnects to the server, so this is also when an offline mount becomes current again.
    private func resync(_ why: String) {
        let (targets, wasOffline): ([VoidfsItem], Bool) = lock.withLock {
            defer { offline = false }
            epoch += 1
            listings.removeAll()
            listed = 0
            misses.removeAll()
            let all = subtree(root) + [rootItem]
            for item in all where item.kind != .folder { retire(item) }
            if !open.isEmpty { pending.insert(root) }
            return (all, offline)
        }
        log.notice("\(why, privacy: .public): revoking \(targets.count) items\(wasOffline ? "; the feed resumed, and the mount is current again" : "", privacy: .public)")
        revoke(targets)
    }

    /// Called with the lock held.
    private func retire(_ item: VoidfsItem) {
        guard item !== rootItem else { return }
        item.retired = true
        if items[item.ino] === item { items[item.ino] = nil }
    }

    /// The kernel refuses every cache action on the volume's root item (`kIOReturnBadArgument`,
    /// measured on macOS 27.0.1), so the top folder is never revoked: its listing stays fresh
    /// because folders are opened without kernel caching, but a name the kernel was told is
    /// missing there stays missing to path lookups until the drive is mounted again.
    private func revoke(_ targets: [VoidfsItem]) {
        let targets = targets.filter { $0 !== rootItem }
        guard !targets.isEmpty else { return }
        revokes.async { [weak self] in
            guard let self else { return }
            for item in targets {
                let error = self.setCacheState(for: item, cacheMode: .none, coherencyType: .noCache, action: .revoke)
                self.note(error, item)
            }
        }
    }

    /// The kernel refuses a revoke of an item it has already let go (reclaimed, or never held):
    /// `kIOReturnBadArgument`, expected and harmless. Anything else means a cache may be stale.
    private func note(_ error: Error?, _ item: VoidfsItem) {
        guard let error else {
            lock.withLock { revoked.ok += 1 }
            log.debug("revoke \(item.ino) ok")
            return
        }
        let e = error as NSError
        if e.domain == NSOSStatusErrorDomain, e.code == -536870206 {
            lock.withLock { revoked.benign += 1 }
            log.debug("revoke \(item.ino): the kernel holds no such item")
        } else {
            lock.withLock { revoked.failed += 1 }
            log.error("revoke \(item.ino) failed, its cache may be stale: \(e.domain, privacy: .public) \(e.code)")
        }
    }

    /// After a file below a pending folder closes or goes: the purges that waited for it.
    private func released(_ item: VoidfsItem) {
        let targets: [VoidfsItem] = lock.withLock {
            var out: [VoidfsItem] = []
            for folder in ancestors(item) where pending.contains(folder) && !openBelow(folder) {
                pending.remove(folder)
                guard let f = folder == root ? rootItem : items[folder] else { continue }
                out += subtree(folder) + [f]
            }
            return out
        }
        if !targets.isEmpty {
            log.debug("purging again now the files open below have closed: \(targets.count) items")
            revoke(targets)
        }
    }

    // MARK: - Names and attributes

    private func fid(_ ino: UInt64) -> FSItem.Identifier { VoidfsItem.identifier(ino, root: root) }

    func attributes(_ a: BridgeAttr, parent: UInt64) -> FSItem.Attributes {
        let r = FSItem.Attributes()
        switch a.kind {
        case .folder: r.type = .directory
        case .symlink: r.type = .symlink
        default: r.type = .file
        }
        r.mode = a.mode & 0o7777
        r.linkCount = a.kind == .folder ? 2 : 1
        r.uid = options.uid
        r.gid = options.gid
        r.flags = 0
        r.size = a.kind == .symlink ? UInt64(a.target?.utf8.count ?? 0) : a.size
        r.allocSize = (r.size + 4095) & ~4095
        r.fileID = fid(a.ino)
        r.parentID = a.ino == root ? .parentOfRoot : fid(parent)
        let t = timespec(tv_sec: Int(a.mtimeSeconds), tv_nsec: Int(a.mtimeNanoseconds))
        r.modifyTime = t
        r.changeTime = t
        r.accessTime = t
        r.birthTime = t
        r.addedTime = t
        r.backupTime = timespec()
        r.supportsLimitedXAttrs = false
        r.inhibitKernelOffloadedIO = false
        return r
    }

    /// The current incarnation of what a lookup found.
    private func incarnation(_ a: BridgeAttr, parent: UInt64) -> VoidfsItem {
        if a.ino == root { return rootItem }
        return lock.withLock {
            if let item = items[a.ino] {
                item.parent = parent
                item.kind = a.kind
                return item
            }
            let item = VoidfsItem(ino: a.ino, id: fid(a.ino), parent: parent, kind: a.kind)
            items[a.ino] = item
            return item
        }
    }

    /// An open incarnation's attributes are those bound at open; others are current.
    private func attr(_ item: VoidfsItem) async throws -> BridgeAttr {
        if let h = lock.withLock({ item.handle }) { return h.attr }
        return try await call { try await $0.getattr(item.ino) }
    }

    func lookupItem(named name: FSFileName, in directory: FSItem, context: FSContext) async throws -> FSLookupItemResult {
        guard let dir = directory as? VoidfsItem else { throw posix(EINVAL) }
        // voidfs names are UTF-8.
        guard let n = name.string else { throw posix(ENOENT) }
        var found: BridgeAttr?
        if let l = lock.withLock({ listings[dir.ino] }) {
            switch l.index[n.precomposedStringWithCanonicalMapping] {
            case nil:
                missed(n, in: dir.ino)
                throw posix(ENOENT)
            case let i? where i >= 0: found = l.attrs[i]
            default: break // two names alike: the daemon says EILSEQ
            }
        }
        let a: BridgeAttr
        do {
            if let found { a = found } else { a = try await call { try await $0.lookup(dir.ino, n) } }
        } catch {
            if (error as NSError).code == Int(ENOENT) { missed(n, in: dir.ino) }
            throw error
        }
        let item = incarnation(a, parent: dir.ino)
        return FSLookupItemResult(foundItem: item, itemName: name, itemAttributes: attributes(a, parent: dir.ino))!
    }

    /// Remembers a miss the kernel may cache. Past the bound, every folder matches any name.
    private func missed(_ name: String, in folder: UInt64) {
        lock.withLock {
            misses[folder, default: Missed()].insert(name)
            missedNames += 1
            if missedNames > Self.missesLimit {
                for key in misses.keys { misses[key] = Missed(any: true) }
                missedNames = 0
            }
        }
    }

    func reclaimItem(_ item: FSItem) async throws {
        guard let item = item as? VoidfsItem, item !== rootItem else { return }
        let handle: OpenFile? = lock.withLock {
            if items[item.ino] === item { items[item.ino] = nil }
            open[ObjectIdentifier(item)] = nil
            defer { item.handle = nil }
            return item.handle
        }
        if let handle { try? await handle.session.close(handle.fh) }
        released(item)
    }

    func attributes(_ desiredAttributes: FSItem.GetAttributesRequest, of item: FSItem, context: FSContext) async throws -> FSGetAttributesResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        return FSGetAttributesResult(attributes: attributes(try await attr(item), parent: lock.withLock { item.parent }))!
    }

    /// The folder's entries: a listing kept until an invalidation names the folder, fetched in
    /// pages at one namespace generation (a page that raced a change answers `EAGAIN`, which the
    /// session retries; pages of different generations start again).
    private func listing(_ dir: UInt64) async throws -> Listing {
        if let l = lock.withLock({ listings[dir] }) { return l }
        for _ in 0..<4 {
            let mark = lock.withLock { epoch }
            var (l, complete): (Listing, Bool) = try await call { s in
                var l = Listing()
                var after: String?
                while true {
                    let page = try await s.readdir(dir, after: after, limit: s.info.maxEntries)
                    if l.generation != 0, page.generation != l.generation { return (l, false) }
                    l.generation = max(page.generation, 1)
                    l.names += page.names
                    l.attrs += page.attrs
                    if page.names.count < s.info.maxEntries { return (l, true) }
                    after = page.names.last
                }
            }
            guard complete else { continue }
            l.indexNames()
            lock.withLock {
                guard epoch == mark else { return }
                if listed + l.names.count > Self.listingLimit { listings.removeAll(); listed = 0 }
                listings[dir] = l
                listed += l.names.count
            }
            return l
        }
        throw posix(EAGAIN)
    }

    func enumerateDirectory(_ directory: FSItem, startingAt cookie: FSDirectoryCookie, verifier: FSDirectoryVerifier, attributes wanted: FSItem.GetAttributesRequest?, packer: FSDirectoryEntryPacker, context: FSContext) async throws -> FSEnumerateDirectoryResult {
        guard let dir = directory as? VoidfsItem else { throw posix(ENOTDIR) }
        let l = try await listing(dir.ino)
        let done = FSEnumerateDirectoryResult(verifier: l.generation)!
        // Positions: with no attributes wanted, 0 is ".", 1 is "..", and child k is k + 2.
        let dots = wanted == nil ? 2 : 0
        var pos = Int(cookie.rawValue)
        if pos < dots {
            if pos == 0 {
                guard packer.packEntry(name: FSFileName(string: "."), itemType: .directory, itemID: dir.id, nextCookie: FSDirectoryCookie(rawValue: 1), attributes: nil) else { return done }
                pos = 1
            }
            let up = dir === rootItem ? FSItem.Identifier.parentOfRoot : fid(lock.withLock { dir.parent })
            guard packer.packEntry(name: FSFileName(string: ".."), itemType: .directory, itemID: up, nextCookie: FSDirectoryCookie(rawValue: 2), attributes: nil) else { return done }
            pos = 2
        }
        var index = pos - dots
        while index < l.names.count {
            let a = l.attrs[index]
            let type: FSItem.ItemType = a.kind == .folder ? .directory : a.kind == .symlink ? .symlink : .file
            pos += 1
            if !packer.packEntry(name: FSFileName(string: l.names[index]), itemType: type, itemID: fid(a.ino), nextCookie: FSDirectoryCookie(rawValue: UInt64(pos)), attributes: wanted == nil ? nil : attributes(a, parent: dir.ino)) {
                break
            }
            index += 1
        }
        return done
    }

    func readSymbolicLink(_ item: FSItem, context: FSContext) async throws -> FSReadSymlinkResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        let a = try await attr(item)
        var target = a.target
        if target == nil { target = try await call { try await $0.readlink(item.ino) } }
        return FSReadSymlinkResult(contents: FSFileName(string: target ?? ""), symlinkAttributes: attributes(a, parent: lock.withLock { item.parent }))!
    }

    // MARK: - Opens and reads

    /// The incarnation's daemon handle: opened at its first open (or read), bound to the version
    /// current then, and kept until the kernel closes or reclaims it.
    private func handle(_ item: VoidfsItem) async throws -> OpenFile {
        let task: Task<OpenFile, Error> = lock.withLock {
            if let h = item.handle { return Task { h } }
            if let t = item.opening { return t }
            let t = Task {
                do {
                    let f = try await self.call { s in
                        let (fh, a) = try await s.open(item.ino, write: false)
                        return OpenFile(fh: fh, attr: a, session: s)
                    }
                    self.lock.withLock {
                        item.handle = f
                        item.opening = nil
                        self.open[ObjectIdentifier(item)] = item
                    }
                    return f
                } catch {
                    self.lock.withLock { item.opening = nil }
                    throw error
                }
            }
            item.opening = t
            return t
        }
        return try await task.value
    }

    func open(_ item: FSItem, modes: FSVolume.OpenModes, cacheMode: FSVolume.DataCacheMode, context: FSContext) async throws -> FSOpenItemResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        if modes.contains(.write) { throw posix(EROFS) }
        if item.kind == .file { _ = try await handle(item) }
        // A folder's listing is the extension's to cache (`listings`, kept by the relay): the
        // kernel's, once granted, outlives every revoke of the top folder (see `revoke`).
        if item.kind == .folder { return FSOpenItemResult(grantedCoherency: .noCache) }
        return FSOpenItemResult(grantedCoherency: cacheMode == .none ? .noCache : .readCache)
    }

    func close(_ item: FSItem, context: FSContext) async {
        guard let item = item as? VoidfsItem else { return }
        let handle: OpenFile? = lock.withLock {
            open[ObjectIdentifier(item)] = nil
            defer { item.handle = nil }
            return item.handle
        }
        if let handle { try? await handle.session.close(handle.fh) }
        released(item)
    }

    func upgrade(_ item: FSItem, cacheMode: FSVolume.DataCacheMode, context: FSContext) async throws -> FSUpgradeItemResult {
        FSUpgradeItemResult(grantedCoherency: cacheMode == .none ? .noCache : .readCache)
    }

    func read(from item: FSItem, at offset: off_t, length: Int, into buffer: FSMutableFileDataBuffer) async throws -> FSReadFileResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        var f = try await handle(item)
        let parent = lock.withLock { item.parent }
        guard offset >= 0, UInt64(offset) < f.attr.size else { return FSReadFileResult(bytesRead: 0, itemAttributes: attributes(f.attr, parent: parent))! }
        let n = UInt64(min(length, buffer.length, Int(f.attr.size - UInt64(offset))))
        var data: Data
        do {
            // A handle goes with its session (the daemon or the agent restarted): reopen, and go
            // on only if the same version is still current.
            if !f.session.isWatching { f = try await reopen(item, f) }
            do {
                data = try await f.session.read(f.fh, offset: UInt64(offset), length: n)
            } catch let error where Bridge.errno(error) == ESTALE && !f.session.isWatching {
                f = try await reopen(item, f)
                data = try await f.session.read(f.fh, offset: UInt64(offset), length: n)
            }
        } catch {
            log.error("read \(item.ino) @\(offset)+\(n): \(error.localizedDescription, privacy: .public)")
            throw failure(error)
        }
        let copied = buffer.withUnsafeMutableBytes { dst in data.copyBytes(to: dst) }
        return FSReadFileResult(bytesRead: copied, itemAttributes: attributes(f.attr, parent: parent))!
    }

    private func reopen(_ item: VoidfsItem, _ old: OpenFile) async throws -> OpenFile {
        let f = try await call { s in
            let (fh, a) = try await s.open(item.ino, write: false)
            return OpenFile(fh: fh, attr: a, session: s)
        }
        guard f.attr.objectId == old.attr.objectId, f.attr.versionId == old.attr.versionId, f.attr.size == old.attr.size else {
            try? await f.session.close(f.fh)
            log.notice("read \(item.ino): its version changed while the daemon was away; the open file is stale")
            throw posix(ESTALE)
        }
        let kept = lock.withLock { () -> Bool in
            guard item.handle?.fh == old.fh, item.handle?.session === old.session else { return false }
            item.handle = f
            return true
        }
        if !kept { try? await f.session.close(f.fh) }
        return lock.withLock { item.handle } ?? f
    }

    // MARK: - Extended attributes

    func xattrs(of item: FSItem, context: FSContext) async throws -> FSListXattrsResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        guard try await attr(item).hasXattrs else { return FSListXattrsResult(xattrNames: [])! }
        let names = try await call { try await $0.listxattr(item.ino) }
        return FSListXattrsResult(xattrNames: names.map { FSFileName(string: $0) })!
    }

    func xattr(named name: FSFileName, of item: FSItem, context: FSContext) async throws -> FSGetXattrResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        guard let n = name.string, try await attr(item).hasXattrs else { throw posix(ENOATTR) }
        return FSGetXattrResult(xattrValue: try await call { try await $0.getxattr(item.ino, n) })!
    }

    func setXattr(named name: FSFileName, to value: Data?, on item: FSItem, policy: FSVolume.SetXattrPolicy, context: FSContext) async throws -> FSSetXattrResult {
        throw posix(EROFS)
    }

    // MARK: - Everything that would write (item 4)

    func createItem(named name: FSFileName, type: FSItem.ItemType, in directory: FSItem, attributes newAttributes: FSItem.SetAttributesRequest, context: FSContext) async throws -> FSCreateItemResult {
        throw posix(EROFS)
    }

    func createSymbolicLink(named name: FSFileName, in directory: FSItem, attributes newAttributes: FSItem.SetAttributesRequest, linkContents contents: FSFileName, context: FSContext) async throws -> FSCreateSymlinkResult {
        throw posix(EROFS)
    }

    func createLink(to item: FSItem, named name: FSFileName, in directory: FSItem, context: FSContext) async throws -> FSCreateLinkResult {
        throw posix(EROFS)
    }

    func renameItem(_ item: FSItem, inDirectory sourceDirectory: FSItem, named sourceName: FSFileName, to destinationName: FSFileName, inDirectory destinationDirectory: FSItem, overItem: FSItem?, context: FSContext) async throws -> FSRenameItemResult {
        throw posix(EROFS)
    }

    func removeItem(_ item: FSItem, named name: FSFileName, from directory: FSItem, context: FSContext) async throws -> FSRemoveItemResult {
        throw posix(EROFS)
    }

    func setAttributes(_ newAttributes: FSItem.SetAttributesRequest, on item: FSItem, context: FSContext) async throws -> FSSetAttributesResult {
        throw posix(EROFS)
    }

    func write(contents: Data, to item: FSItem, at offset: off_t) async throws -> FSWriteFileResult {
        throw posix(EROFS)
    }
}
