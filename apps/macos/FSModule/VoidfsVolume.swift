// SPDX-License-Identifier: Apache-2.0
//
// A read-only voidfs drive as an FSKit volume. Metadata comes from folder listings (§4.9), bytes
// from ranged GETs, extended attributes from §4.8, and the change feed (§5.6) invalidates what
// other machines change.

import FSKit
import Foundation
import os

func posix(_ code: Int32) -> Error { fs_errorForPOSIXError(code) }

func posix(_ error: Error) -> Error {
    if let e = error as? VoidfsError { return posix(e.errno) }
    return error
}

/// Mount options, from `mount -o k=v,…` or the activate options.
struct VolumeOptions {
    /// How long a folder listing is trusted without a change-feed notice.
    var dirTTL: Duration = .seconds(5)
    /// `f_iosize`. Measured on macOS 27.0: the kernel sends reads of at most 1 MiB whatever this
    /// says (256 KiB, 4 MiB and 16 MiB were tried).
    var ioSize = 1 << 20
    var changeFeed = true
    var uid = getuid()
    var gid = getgid()

    mutating func apply(_ args: [String]) {
        var i = 0
        while i < args.count {
            let a = args[i]
            let value = i + 1 < args.count ? args[i + 1] : ""
            switch a {
            case "-u": uid = uid_t(value) ?? uid; i += 1
            case "-g": gid = gid_t(value) ?? gid; i += 1
            case "-o":
                for kv in value.split(separator: ",") {
                    let p = kv.split(separator: "=", maxSplits: 1).map(String.init)
                    switch (p.first, p.count > 1 ? p[1] : nil) {
                    case ("ttl", let v?): dirTTL = .milliseconds(Int(v) ?? 5000)
                    case ("iosize", let v?): ioSize = Int(v) ?? ioSize
                    case ("nofeed", _): changeFeed = false
                    default: break
                    }
                }
                i += 1
            default: break
            }
            i += 1
        }
    }
}

/// Counts file system calls, so the spike can relate `ls` or `cp` to requests.
final class OpStats: @unchecked Sendable {
    private let lock = OSAllocatedUnfairLock(initialState: (ops: [String: Int](), readSizes: [Int: Int]()))

    func count(_ op: String) { lock.withLock { $0.ops[op, default: 0] += 1 } }

    func read(_ length: Int) {
        lock.withLock { s in
            s.ops["read", default: 0] += 1
            var bucket = 4096
            while bucket < length { bucket <<= 1 }
            s.readSizes[bucket, default: 0] += 1
        }
    }

    func dump() -> String {
        lock.withLock { s in
            let ops = s.ops.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" }.joined(separator: " ")
            let sizes = s.readSizes.sorted { $0.key < $1.key }.map { "≤\($0.key / 1024)K:\($0.value)" }.joined(separator: " ")
            return "ops: \(ops) | read sizes: \(sizes)"
        }
    }

    func reset() { lock.withLock { $0 = ([:], [:]) } }
}

/// A folder's children as of one listing.
struct DirCache {
    var children: [VoidfsItem]
    var byName: [String: VoidfsItem]
    var fetched: ContinuousClock.Instant
    var verifier: UInt64
    var stale = false
}

final class VoidfsVolume: FSVolume, FSVolume.Handler, FSVolume.ReadWriteHandler, FSVolume.XattrHandler, FSVolume.DataCacheHandler, @unchecked Sendable {
    let client: VoidfsClient
    let drive: DriveInfo
    var options = VolumeOptions()
    private let log = Logger(subsystem: "dev.voidfs", category: "fs")
    private let ops = OpStats()
    private let root: VoidfsItem

    // Everything below is guarded by `lock`, which is never held across an await or a call into
    // FSKit.
    private let lock = NSLock()
    private var items: [String: VoidfsItem] = [:]
    private var dirs: [FSItem.Identifier: DirCache] = [:]
    private var inflight: [FSItem.Identifier: Task<DirCache, Error>] = [:]
    private var xattrCache: [String: [String: Data]] = [:]
    private var nextVerifier: UInt64 = 1
    private var seq: UInt64
    private var feed: Task<Void, Never>?
    /// The bridge to the daemon through the app's agent: spike-only hooks use it until item 3
    /// moves the volume onto it.
    private let bridge = BridgeClient()

    init(client: VoidfsClient, drive: DriveInfo, volumeID: FSVolume.Identifier) {
        self.client = client
        self.drive = drive
        self.seq = drive.seq
        root = VoidfsItem(objectId: "", id: .rootDirectory, info: ItemInfo(root: parseTimestamp(drive.createdAt)), parentID: .parentOfRoot)
        super.init(volumeID: volumeID, volumeName: FSFileName(string: drive.displayName ?? drive.alias ?? client.target.drive))
    }

    // MARK: - Properties FSKit reads

    var maximumLinkCount: Int { 1 }
    var maximumNameLength: Int { 255 }
    var restrictsOwnershipChanges: Bool { true }
    var truncatesLongNames: Bool { false }
    var maximumXattrSize: Int { 64 << 10 }
    var maximumFileSize: UInt64 { 5 << 40 }
    var requestedMountOptions: FSVolume.MountOptions { .readOnly }

    var supportedVolumeCapabilities: FSVolume.SupportedCapabilities {
        let c = FSVolume.SupportedCapabilities()
        c.supportsSymbolicLinks = true
        c.supportsHardLinks = false
        c.supports2TBFiles = true
        c.supports64BitObjectIDs = true
        c.supportsFastStatFS = true
        c.supportsHiddenFiles = true
        c.doesNotSupportSettingFilePermissions = true
        // Keys are case-sensitive byte strings (spec/format.md §7.6).
        c.caseFormat = .sensitive
        return c
    }

    var volumeStatistics: FSStatFSResult {
        let s = FSStatFSResult(fileSystemTypeName: "voidfs")
        s.blockSize = 4096
        s.ioSize = options.ioSize
        let total: UInt64 = 1 << 50
        let used = min(drive.usageBytes ?? 0, total)
        s.totalBytes = total
        s.usedBytes = used
        s.availableBytes = total - used
        s.freeBytes = total - used
        s.totalFiles = 1 << 32
        s.freeFiles = 1 << 31
        return s
    }

    // MARK: - Life cycle

    func activate(options taskOptions: FSTaskOptions) async throws -> FSActivateResult {
        options.apply(taskOptions.taskOptions)
        log.notice("activate \(taskOptions.taskOptions, privacy: .public) uid=\(getuid()) euid=\(geteuid()) home=\(NSHomeDirectory(), privacy: .public) requested lookup attrs=\(String(FSLookupItemResult.requestedAttributes.wantedAttributes.rawValue, radix: 16), privacy: .public) getattr attrs=\(String(FSGetAttributesResult.requestedAttributes.wantedAttributes.rawValue, radix: 16), privacy: .public) read attrs=\(String(FSReadFileResult.requestedAttributes.wantedAttributes.rawValue, radix: 16), privacy: .public)")
        return FSActivateResult(rootItem: root)!
    }

    func deactivate(options: FSDeactivateOptions = []) async throws {
        log.notice("deactivate")
    }

    // macOS 27.0 (26A428) sends `activateVolumeWithOptions:` and `deactivateVolumeWithOptions:`,
    // while the Xcode 27 beta SDK (26A5353p) declares `activateWithOptions:` and
    // `deactivateWithOptions:`. Answer to both until the SDK and the OS agree.
    @objc(activateVolumeWithOptions:replyHandler:)
    func activateVolume(options: FSTaskOptions, replyHandler reply: @escaping (FSActivateResult?, Error?) -> Void) {
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
        options.apply(taskOptions.taskOptions)
        log.notice("mount \(taskOptions.taskOptions, privacy: .public) ttl=\(self.options.dirTTL, privacy: .public) iosize=\(self.options.ioSize) feed=\(self.options.changeFeed)")
        if options.changeFeed {
            feed = Task.detached { [weak self] in await self?.runChangeFeed() }
        }
    }

    func unmount() async {
        feed?.cancel()
        log.notice("unmount; \(self.statsLine(), privacy: .public)")
    }

    func synchronize(flags: FSSyncFlags) async throws {}

    // MARK: - Names and attributes

    func attributes(of item: VoidfsItem, _ info: ItemInfo) -> FSItem.Attributes {
        let a = FSItem.Attributes()
        switch info.kind {
        case .file: a.type = .file
        case .folder: a.type = .directory
        case .symlink: a.type = .symlink
        }
        a.mode = info.mode & 0o7777
        a.linkCount = info.kind == .folder ? 2 : 1
        a.uid = options.uid
        a.gid = options.gid
        a.flags = 0
        a.size = info.kind == .symlink ? UInt64(info.target?.utf8.count ?? 0) : info.size
        a.allocSize = (a.size + 4095) & ~4095
        a.fileID = item.id
        a.parentID = item.parentID
        a.modifyTime = info.mtime
        a.changeTime = info.mtime
        a.accessTime = info.mtime
        a.birthTime = info.mtime
        a.addedTime = info.mtime
        a.backupTime = timespec()
        a.supportsLimitedXAttrs = false
        a.inhibitKernelOffloadedIO = false
        return a
    }

    private func snapshot(_ item: VoidfsItem) -> ItemInfo { lock.withLock { item.info } }

    private func attributes(of item: VoidfsItem) -> FSItem.Attributes { attributes(of: item, snapshot(item)) }

    /// The children of `dir`, from the cache while it is fresh. When the server cannot be reached,
    /// a stale listing is better than an error, so browsing keeps working offline.
    private func listing(_ dir: VoidfsItem) async throws -> DirCache {
        let ttl = options.dirTTL
        let (cached, task): (DirCache?, Task<DirCache, Error>?) = lock.withLock {
            let cached = dirs[dir.id]
            if let c = cached, !c.stale, ContinuousClock.now - c.fetched < ttl { return (c, nil) }
            if let t = inflight[dir.id] { return (cached, t) }
            let key = dir.info.key
            let t = Task { try await self.fetch(dir, key: key) }
            inflight[dir.id] = t
            return (cached, t)
        }
        guard let task else { return cached! }
        do {
            return try await task.value
        } catch let e as VoidfsError {
            if case .network = e, let c = cached {
                log.error("listing \(dir.info.key, privacy: .public) failed (\(e.description, privacy: .public)); serving the cached one")
                return c
            }
            throw e
        }
    }

    private func fetch(_ dir: VoidfsItem, key: String) async throws -> DirCache {
        let listing: Listing
        do {
            listing = try await client.list(prefix: key)
        } catch {
            lock.withLock { inflight[dir.id] = nil }
            throw error
        }
        let (cache, changed) = lock.withLock { install(listing, in: dir, key: key) }
        revokeInKernel(changed)
        return cache
    }

    /// Records a fresh listing of `dir`, reusing the items already known. Returns the items whose
    /// content changed. Called with the lock held.
    private func install(_ listing: Listing, in dir: VoidfsItem, key: String) -> (DirCache, [VoidfsItem]) {
        inflight[dir.id] = nil
        var changed: [VoidfsItem] = []
        var children: [VoidfsItem] = []
        var byName: [String: VoidfsItem] = [:]
        children.reserveCapacity(listing.entries.count)
        for e in listing.entries {
            let info = ItemInfo(key: key + e.name, entry: e)
            let item: VoidfsItem
            if let existing = items[e.objectId] {
                if existing.info.etag != info.etag { changed.append(existing) }
                existing.info = info
                existing.parentID = dir.id
                item = existing
            } else {
                item = VoidfsItem(objectId: e.objectId, id: VoidfsItem.fileID(for: e.objectId), info: info, parentID: dir.id)
                items[e.objectId] = item
            }
            children.append(item)
            let name = info.name
            byName[name] = item
            let nfc = name.precomposedStringWithCanonicalMapping
            if nfc != name { byName[nfc] = item }
        }
        let cache = DirCache(children: children, byName: byName, fetched: .now, verifier: nextVerifier)
        nextVerifier += 1
        dirs[dir.id] = cache
        seq = max(seq, listing.seq)
        return (cache, changed)
    }

    func lookupItem(named name: FSFileName, in directory: FSItem, context: FSContext) async throws -> FSLookupItemResult {
        guard let dir = directory as? VoidfsItem else { throw posix(EINVAL) }
        guard let n = name.string else { throw posix(ENOENT) }
        ops.count("lookup")
        if n == ".voidfs-bench" {
            await selfBenchmark()
            throw posix(ENOENT)
        }
        // The kernel caches a name's ENOENT, so a hook takes a `~<anything>` suffix to run again.
        let hook = n.split(separator: "~", maxSplits: 1).first.map(String.init) ?? n
        if hook == ".voidfs-xpc" {
            // Spike instrumentation: the bare hop to the app's agent, 500 x 4 KiB.
            let scenario = BridgeScenario(client: bridge, drive: client.target.drive, another: { BridgeClient() }) { [log] line in log.notice("xpc \(line, privacy: .public)") }
            do { scenario.distribution("XPC ping 4 KiB to \(Bridge.service)", try await scenario.timed(500) { _ = try await bridge.ping(Data(count: 4096)) }) }
            catch { log.notice("xpc \(Bridge.service, privacy: .public): \(error.localizedDescription, privacy: .public)") }
            throw posix(ENOENT)
        }
        if hook == ".voidfs-bridge" || hook.hasPrefix(".voidfs-bridge-") {
            // Spike instrumentation: the bridge to the daemon, end to end from this sandbox
            // (Shared/BridgeScenario.swift). It runs detached; its lines go to the log.
            let scenario = BridgeScenario(client: bridge, drive: client.target.drive, another: { BridgeClient() }) { [log] line in log.notice("bridge \(line, privacy: .public)") }
            let mode = String(hook.dropFirst(".voidfs-bridge".count))
            Task.detached {
                switch mode {
                case "": await scenario.main()
                case "-daemon-restart": await scenario.restart("daemon")
                case "-agent-restart": await scenario.restart("agent")
                case let m where m.hasPrefix("-remote:"): await scenario.remote(key: String(m.dropFirst("-remote:".count)))
                default: scenario.say("unknown bridge hook \(mode)")
                }
            }
            throw posix(ENOENT)
        }
        if n.hasPrefix(".voidfs-cache:") {
            // Spike instrumentation: `.voidfs-cache:<action>:<mode>:<coherency>:<key with + for />`
            // calls setCacheState on a known item, to find what makes the kernel forget.
            let p = n.split(separator: ":", maxSplits: 4).map(String.init)
            if p.count == 5, let a = Int(p[1]), let m = Int(p[2]), let c = Int(p[3]),
               let action = FSVolume.KernelCacheCoherencyAction(rawValue: a),
               let mode = FSVolume.DataCacheMode(rawValue: m), let coherency = FSVolume.KernelCacheCoherencyType(rawValue: c) {
                let key = p[4].replacingOccurrences(of: "+", with: "/")
                let target: VoidfsItem? = key.isEmpty ? root : lock.withLock { items.values.first { $0.info.key == key } }
                if let target {
                    let err = await Task.detached { self.setCacheState(for: target, cacheMode: mode, coherencyType: coherency, action: action) }.value
                    log.notice("setCacheState \(key, privacy: .public) action=\(a) mode=\(m) coherency=\(c): \(err.map { String(describing: $0) } ?? "ok", privacy: .public)")
                } else {
                    log.notice("setCacheState: no item \(key, privacy: .public)")
                }
            }
            throw posix(ENOENT)
        }
        if n.hasPrefix(".voidfs-stats") {
            log.notice("stats: \(self.statsLine(), privacy: .public)")
            if n == ".voidfs-stats-reset" { ops.reset(); client.stats.reset() }
            throw posix(ENOENT)
        }
        let c: DirCache
        do { c = try await listing(dir) } catch { throw posix(error) }
        guard let item = c.byName[n] ?? c.byName[n.precomposedStringWithCanonicalMapping] else {
            ops.count("lookup-miss")
            log.debug("lookup miss \(dir.info.key, privacy: .public)\(n, privacy: .public)")
            throw posix(ENOENT)
        }
        let info = snapshot(item)
        log.debug("lookup hit \(info.key, privacy: .public) \(UInt(bitPattern: ObjectIdentifier(item).hashValue), format: .hex)")
        return FSLookupItemResult(foundItem: item, itemName: FSFileName(string: info.name), itemAttributes: attributes(of: item, info))!
    }

    func reclaimItem(_ item: FSItem) async throws {
        guard let item = item as? VoidfsItem, item !== root else { return }
        ops.count("reclaim")
        let reclaimed = lock.withLock {
            item.tryReclaim {
                self.items[item.objectId] = nil
                self.dirs[item.id] = nil
                self.xattrCache[item.objectId] = nil
                // The parent's cached listing still holds this instance. Re-list before the next
                // lookup, so the kernel never gets back an item the volume no longer tracks (a
                // revoke aimed at the tracked twin would then silently miss).
                self.dirs[item.parentID]?.stale = true
            }
        }
        log.debug("reclaim \(item.info.key, privacy: .public): \(reclaimed ? "released" : "kept, still referenced")")
    }

    func attributes(_ desiredAttributes: FSItem.GetAttributesRequest, of item: FSItem, context: FSContext) async throws -> FSGetAttributesResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        ops.count("getattr")
        return FSGetAttributesResult(attributes: attributes(of: item))!
    }

    func enumerateDirectory(_ directory: FSItem, startingAt cookie: FSDirectoryCookie, verifier: FSDirectoryVerifier, attributes wanted: FSItem.GetAttributesRequest?, packer: FSDirectoryEntryPacker, context: FSContext) async throws -> FSEnumerateDirectoryResult {
        guard let dir = directory as? VoidfsItem else { throw posix(ENOTDIR) }
        ops.count(wanted == nil ? "readdir" : "readdir+attrs")
        let c: DirCache
        do { c = try await listing(dir) } catch { throw posix(error) }
        // Positions: with no attributes wanted, 0 is ".", 1 is "..", and child k is k + 2.
        let dots = wanted == nil ? 2 : 0
        var pos = Int(cookie.rawValue)
        if pos < dots {
            if pos == 0 {
                guard packer.packEntry(name: FSFileName(string: "."), itemType: .directory, itemID: dir.id, nextCookie: FSDirectoryCookie(rawValue: 1), attributes: nil) else {
                    return FSEnumerateDirectoryResult(verifier: c.verifier)!
                }
                pos = 1
            }
            guard packer.packEntry(name: FSFileName(string: ".."), itemType: .directory, itemID: dir.parentID, nextCookie: FSDirectoryCookie(rawValue: 2), attributes: nil) else {
                return FSEnumerateDirectoryResult(verifier: c.verifier)!
            }
            pos = 2
        }
        let skip = pos - dots
        let infos = lock.withLock { c.children.dropFirst(skip).map { ($0, $0.info) } }
        for (item, info) in infos {
            pos += 1
            let type: FSItem.ItemType = info.kind == .folder ? .directory : info.kind == .symlink ? .symlink : .file
            if !packer.packEntry(name: FSFileName(string: info.name), itemType: type, itemID: item.id, nextCookie: FSDirectoryCookie(rawValue: UInt64(pos)), attributes: wanted == nil ? nil : attributes(of: item, info)) {
                break
            }
        }
        return FSEnumerateDirectoryResult(verifier: c.verifier)!
    }

    func readSymbolicLink(_ item: FSItem, context: FSContext) async throws -> FSReadSymlinkResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        let info = snapshot(item)
        return FSReadSymlinkResult(contents: FSFileName(string: info.target ?? ""), symlinkAttributes: attributes(of: item, info))!
    }

    // MARK: - Reads

    func read(from item: FSItem, at offset: off_t, length: Int, into buffer: FSMutableFileDataBuffer) async throws -> FSReadFileResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        let info = snapshot(item)
        ops.read(length)
        let attrs = attributes(of: item, info)
        guard offset >= 0, UInt64(offset) < info.size else { return FSReadFileResult(bytesRead: 0, itemAttributes: attrs)! }
        let n = min(length, buffer.length, Int(info.size - UInt64(offset)))
        let data: Data
        do { data = try await client.read(key: info.key, offset: UInt64(offset), length: n) } catch {
            log.error("read \(info.key, privacy: .public) @\(offset)+\(n): \(String(describing: error), privacy: .public)")
            throw posix(error)
        }
        let copied = buffer.withUnsafeMutableBytes { dst in data.copyBytes(to: dst) }
        return FSReadFileResult(bytesRead: copied, itemAttributes: attrs)!
    }

    // MARK: - Extended attributes

    private func loadXattrs(_ item: VoidfsItem, key: String) async throws -> [String: Data] {
        if let x = lock.withLock({ xattrCache[item.objectId] }) { return x }
        let attrs: Attrs
        do { attrs = try await client.attrs(key: key) } catch { throw posix(error) }
        let x = attrs.xattrs.compactMapValues { Data(base64Encoded: $0) }
        lock.withLock { xattrCache[item.objectId] = x }
        return x
    }

    func xattrs(of item: FSItem, context: FSContext) async throws -> FSListXattrsResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        ops.count("listxattr")
        let info = snapshot(item)
        guard info.hasXattrs else { return FSListXattrsResult(xattrNames: [])! }
        let x = try await loadXattrs(item, key: info.key)
        return FSListXattrsResult(xattrNames: x.keys.sorted().map { FSFileName(string: $0) })!
    }

    func xattr(named name: FSFileName, of item: FSItem, context: FSContext) async throws -> FSGetXattrResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        ops.count("getxattr:\(name.string ?? "?")")
        let info = snapshot(item)
        guard info.hasXattrs, let n = name.string else { throw posix(ENOATTR) }
        guard let v = try await loadXattrs(item, key: info.key)[n] else { throw posix(ENOATTR) }
        return FSGetXattrResult(xattrValue: v)!
    }

    func setXattr(named name: FSFileName, to value: Data?, on item: FSItem, policy: FSVolume.SetXattrPolicy, context: FSContext) async throws -> FSSetXattrResult {
        ops.count("setxattr:\(name.string ?? "?")")
        throw posix(EROFS)
    }

    // MARK: - Kernel data cache

    func open(_ item: FSItem, modes: FSVolume.OpenModes, cacheMode: FSVolume.DataCacheMode, context: FSContext) async throws -> FSOpenItemResult {
        ops.count(modes.contains(.write) ? "open-write" : "open")
        if modes.contains(.write) { throw posix(EROFS) }
        return FSOpenItemResult(grantedCoherency: cacheMode == .none ? .noCache : .readCache)
    }

    func close(_ item: FSItem, context: FSContext) async {
        ops.count("close")
    }

    func upgrade(_ item: FSItem, cacheMode: FSVolume.DataCacheMode, context: FSContext) async throws -> FSUpgradeItemResult {
        ops.count("upgrade")
        return FSUpgradeItemResult(grantedCoherency: cacheMode == .none ? .noCache : .readCache)
    }

    /// Makes the kernel forget `items`, so the next access looks them up again and gets fresh
    /// attributes. Measured on macOS 27.0: `.invalidate` drops cached pages but keeps the cached
    /// size, so the next read is torn (new bytes cut at the old length); only `.revoke` also drops
    /// the attributes and, on a folder, its negative name-cache entries. Runs detached: the
    /// kernel may call back into the volume, so no lock may be held and no handler may wait on it.
    private func revokeInKernel(_ items: [VoidfsItem]) {
        guard !items.isEmpty else { return }
        Task.detached { [self] in
            for item in items {
                let err = setCacheState(for: item, cacheMode: .none, coherencyType: .noCache, action: .revoke)
                log.info("revoke \(item.info.key, privacy: .public) \(UInt(bitPattern: ObjectIdentifier(item).hashValue), format: .hex): \(err.map { String(describing: $0) } ?? "ok", privacy: .public)")
            }
        }
    }

    // MARK: - Change feed

    private func runChangeFeed() async {
        var backoff: Duration = .seconds(1)
        while !Task.isCancelled {
            let since = lock.withLock { seq }
            do {
                let c = try await client.changes(since: since, wait: 20)
                backoff = .seconds(1)
                apply(c)
            } catch VoidfsError.http(410, _) {
                log.notice("change feed expired at \(since); dropping every cached listing")
                let fresh = try? await client.describe()
                lock.withLock {
                    for id in dirs.keys { dirs[id]?.stale = true }
                    xattrCache.removeAll()
                    if let s = fresh?.seq { seq = s }
                }
            } catch {
                if Task.isCancelled { return }
                log.error("change feed: \(String(describing: error), privacy: .public); retrying in \(backoff, privacy: .public)")
                try? await Task.sleep(for: backoff)
                backoff = min(backoff * 2, .seconds(30))
            }
        }
    }

    private func apply(_ c: Changes) {
        guard !c.changes.isEmpty else {
            lock.withLock { seq = max(seq, c.seq) }
            return
        }
        let started = ContinuousClock.now
        let revoke = lock.withLock { record(c) }
        log.info("change feed: \(c.changes.count) changes to seq \(c.seq) applied in \(ContinuousClock.now - started, privacy: .public)")
        revokeInKernel(revoke)
    }

    /// Marks the listings a batch of changes touched as stale, and returns what the kernel must
    /// forget, in order: every known item that changed, then every known folder whose set of
    /// names changed, after its known children (so a name the kernel cached as missing is looked
    /// up again). Called with the lock held.
    private func record(_ c: Changes) -> [VoidfsItem] {
        var revoke: [ObjectIdentifier: VoidfsItem] = [:]
        var revokeLast: [ObjectIdentifier: VoidfsItem] = [:]
        var folders: [String: VoidfsItem] = ["": root]
        for item in items.values where item.info.kind == .folder {
            folders[item.info.key] = item
        }
        for ch in c.changes {
            let known = ch.objectId.flatMap { items[$0] }
            let namesChanged = known == nil || ch.op == "delete" || ch.fromKey != nil
            for key in [ch.key, ch.fromKey].compactMap({ $0 }) {
                guard let folder = folders[Self.parentKey(key)] else { continue }
                dirs[folder.id]?.stale = true
                if namesChanged {
                    // Measured on macOS 27.0: while the kernel holds any child of a folder, a
                    // revoke of the folder leaves its negative name-cache entries in place. Revoke
                    // the children the kernel may hold first, then the folder.
                    for child in items.values where child.parentID == folder.id {
                        revoke[ObjectIdentifier(child)] = child
                    }
                    revokeLast[ObjectIdentifier(folder)] = folder
                }
            }
            if let from = ch.fromKey, from.hasSuffix("/") {
                // A folder rename is one change: move every known descendant's key along.
                for item in items.values where item.info.key.hasPrefix(from) {
                    item.info.key = ch.key + item.info.key.dropFirst(from.count)
                }
            }
            if let known {
                xattrCache[known.objectId] = nil
                revoke[ObjectIdentifier(known)] = known
            }
        }
        seq = max(seq, c.seq)
        for id in revokeLast.keys { revoke[id] = nil }
        return Array(revoke.values) + Array(revokeLast.values)
    }

    private static func parentKey(_ key: String) -> String {
        var k = Substring(key)
        if k.hasSuffix("/") { k = k.dropLast() }
        guard let slash = k.lastIndex(of: "/") else { return "" }
        return String(k[...slash])
    }

    /// Spike instrumentation: random 4 KiB reads of media/big.bin issued from inside the
    /// extension, in the calling handler's context and in a detached user-initiated task, to
    /// separate the process's HTTP cost from the FSKit call path.
    private func selfBenchmark() async {
        func run() async -> String {
            var lat: [Double] = []
            for _ in 0..<200 {
                let off = UInt64.random(in: 0..<(1 << 30) - 4096) & ~4095
                let t = ContinuousClock.now
                _ = try? await client.read(key: "media/big.bin", offset: off, length: 4096)
                let d = ContinuousClock.now - t
                lat.append(Double(d.components.attoseconds) / 1e15 + Double(d.components.seconds) * 1000)
            }
            lat.sort()
            return String(format: "p50 %.2f ms p90 %.2f ms (qos %d)", lat[100], lat[180], qos_class_self().rawValue)
        }
        let inline = await run()
        let detached = await Task.detached(priority: .userInitiated) { await run() }.value
        log.notice("self-benchmark: handler context \(inline, privacy: .public); detached userInitiated \(detached, privacy: .public)")
    }

    private func statsLine() -> String {
        let http = client.stats.snapshot().sorted { $0.key < $1.key }.map { k, v in
            String(format: "%@=%d (%.1f ms avg, %.1f MiB)", k, v.count, Double(v.nanos) / Double(max(v.count, 1)) / 1e6, Double(v.bytes) / 1_048_576)
        }.joined(separator: " ")
        return "\(ops.dump()) | http: \(http)"
    }

    // MARK: - Everything that would write

    func createItem(named name: FSFileName, type: FSItem.ItemType, in directory: FSItem, attributes newAttributes: FSItem.SetAttributesRequest, context: FSContext) async throws -> FSCreateItemResult {
        ops.count("create:\(name.string ?? "?")")
        throw posix(EROFS)
    }

    func createSymbolicLink(named name: FSFileName, in directory: FSItem, attributes newAttributes: FSItem.SetAttributesRequest, linkContents contents: FSFileName, context: FSContext) async throws -> FSCreateSymlinkResult {
        throw posix(EROFS)
    }

    func createLink(to item: FSItem, named name: FSFileName, in directory: FSItem, context: FSContext) async throws -> FSCreateLinkResult {
        throw posix(EROFS)
    }

    func renameItem(_ item: FSItem, inDirectory sourceDirectory: FSItem, named sourceName: FSFileName, to destinationName: FSFileName, inDirectory destinationDirectory: FSItem, overItem: FSItem?, context: FSContext) async throws -> FSRenameItemResult {
        ops.count("rename")
        throw posix(EROFS)
    }

    func removeItem(_ item: FSItem, named name: FSFileName, from directory: FSItem, context: FSContext) async throws -> FSRemoveItemResult {
        ops.count("remove")
        throw posix(EROFS)
    }

    func setAttributes(_ newAttributes: FSItem.SetAttributesRequest, on item: FSItem, context: FSContext) async throws -> FSSetAttributesResult {
        ops.count("setattr")
        throw posix(EROFS)
    }

    func write(contents: Data, to item: FSItem, at offset: off_t) async throws -> FSWriteFileResult {
        ops.count("write")
        throw posix(EROFS)
    }
}
