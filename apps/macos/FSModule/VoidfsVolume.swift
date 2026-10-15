// SPDX-License-Identifier: Apache-2.0
//
// A drive as an FSKit volume, served by the daemon through the app's launch agent
// (Shared/BridgeClient.swift). The daemon's session answers every call; its invalidation relay says
// what changed, here or on other machines, and the volume makes the kernel forget it with revokes.

import CryptoKit
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
    /// `mount -r` (FSKit passes it as `-o ro`).
    var readOnly = false
    /// A log line for every data and namespace callback (`-o trace`).
    var trace = false
    /// Push possible shared mappings on a serial sweep. Zero disables it for reproduction.
    /// Unsynchronized stores are not acknowledged writes (step 5, 14 October).
    var mmapPushMilliseconds = 500
    /// What a write open is granted at most. Measured (step 5, item 4): with writeBack a write
    /// returns before the module sees it, and killing the extension lost acknowledged bytes; with
    /// writeThrough write(2) returns after the bytes are staged.
    static let writeCoherency: FSVolume.KernelCacheCoherencyType = .writeThrough

    mutating func apply(_ args: [String]) {
        var i = 0
        while i < args.count {
            let value = i + 1 < args.count ? args[i + 1] : ""
            switch args[i] {
            case "-u": uid = uid_t(value) ?? uid; i += 1
            case "-g": gid = gid_t(value) ?? gid; i += 1
            case "-r": readOnly = true
            case "-w": readOnly = false
            case "-o":
                for kv in value.split(separator: ",") {
                    let p = kv.split(separator: "=", maxSplits: 1).map(String.init)
                    switch (p.first ?? "", p.count > 1 ? p[1] : nil) {
                    case ("iosize", let v?): ioSize = Int(v) ?? ioSize
                    case ("ro", nil), ("rdonly", nil): readOnly = true
                    case ("rw", nil): readOnly = false
                    case ("trace", _): trace = true
                    case ("mmap-push-ms", let v?):
                        if let ms = Int(v), (0...60_000).contains(ms) { mmapPushMilliseconds = ms }
                    default: break
                    }
                }
                i += 1
            default: break
            }
            i += 1
        }
    }

    /// What the kernel gets for `requested`: at most `writeCoherency`, as the cache modes allow.
    static func grant(_ requested: FSVolume.DataCacheMode) -> FSVolume.KernelCacheCoherencyType {
        switch requested {
        case .readWriteWithCache: return writeCoherency
        case .readWithCache: return writeCoherency == .noCache ? .noCache : .readCache
        default: return .noCache
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
    /// FSKit supplies no mapping lifecycle callback. Every writable file open may leave dirty
    /// mapped pages, even after close; keep its incarnation until reclaim so a later sweep can push them.
    private var mmapPushCandidates: [ObjectIdentifier: VoidfsItem] = [:]
    private var mmapPushTimer: DispatchSourceTimer?
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
    private let pushes = DispatchQueue(label: "dev.voidfs.fs.push", attributes: .concurrent)
    /// Timer handlers run synchronously and serially here, so slow pushes cannot pile up sweeps.
    private let mmapPushes = DispatchQueue(label: "dev.voidfs.fs.mmap-push")
    private static let listingLimit = 200_000
    private static let missesLimit = 200_000

    init(client: BridgeClient, session: BridgeSession, drive: String, options: VolumeOptions, volumeID: FSVolume.Identifier) {
        self.client = client
        self.session = session
        self.drive = drive
        self.options = options
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

    deinit { mmapPushTimer?.cancel() }

    // MARK: - Properties FSKit reads

    var maximumLinkCount: Int { 1 }
    var maximumNameLength: Int { Bridge.maxName }
    var restrictsOwnershipChanges: Bool { true }
    var truncatesLongNames: Bool { false }
    #if VOIDFS_LARGE_XATTR_PROBE
    /// Test-only probe build: logs each time FSKit reads the limit.
    var maximumXattrSize: Int {
        log.notice("probe: FSKit read maximumXattrSize \(Bridge.maxXattr)")
        return Bridge.maxXattr
    }
    #else
    var maximumXattrSize: Int { Bridge.maxXattr }
    #endif
    #if VOIDFS_LARGE_XATTR_PROBE_BITS
    /// Test-only second probe build: FSKit uses this instead of `maximumXattrSize` when both exist.
    var maximumXattrSizeInBits: Int {
        log.notice("probe: FSKit read maximumXattrSizeInBits 26")
        return 26
    }
    #endif
    var maximumFileSize: UInt64 { 5 << 40 }
    var requestedMountOptions: FSVolume.MountOptions { options.readOnly ? .readOnly : [] }

    var supportedVolumeCapabilities: FSVolume.SupportedCapabilities {
        let c = FSVolume.SupportedCapabilities()
        // A drive can hold one, but nothing on the wire creates it (`ln -s` is ENOTSUP).
        c.supportsSymbolicLinks = false
        c.supportsHardLinks = false
        c.supports2TBFiles = true
        c.supports64BitObjectIDs = true
        c.supportsPersistentObjectIDs = true
        c.supportsFastStatFS = true
        // `chflags hidden` isn't kept (the core stores no flags, though §4.8 has `hidden`).
        c.supportsHiddenFiles = false
        c.supportsSparseFiles = true
        c.doesNotSupportSettingFilePermissions = false
        c.caseFormat = caseSensitive ? .sensitive : .insensitive
        return c
    }

    /// Space as this Mac's disk has it: writes are staged there before they publish, so what it
    /// has free bounds a copy (Finder checks before it starts). A drive has no quota to show.
    var volumeStatistics: FSStatFSResult {
        let s = FSStatFSResult(fileSystemTypeName: "voidfs")
        s.blockSize = 4096
        s.ioSize = options.ioSize
        var disk = statfs()
        if statfs(NSHomeDirectory(), &disk) == 0 {
            let block = UInt64(disk.f_bsize)
            s.totalBytes = UInt64(disk.f_blocks) * block
            s.availableBytes = UInt64(disk.f_bavail) * block
            s.freeBytes = UInt64(disk.f_bfree) * block
            s.usedBytes = s.totalBytes - min(s.totalBytes, s.freeBytes)
        } else {
            s.totalBytes = 1 << 50
            (s.availableBytes, s.freeBytes, s.usedBytes) = (s.totalBytes, s.totalBytes, 0)
        }
        s.totalFiles = 1 << 32
        s.freeFiles = 1 << 31
        return s
    }

    // MARK: - Life cycle

    /// The mount's options arrive here, not at load: the session `loadResource` opened is
    /// read-only, and a writable mount replaces it with a writable one now.
    func activate(options taskOptions: FSTaskOptions) async throws -> FSActivateResult {
        options.apply(taskOptions.taskOptions)
        if !options.readOnly {
            let s = try await client.open(drive: drive, readOnly: false)
            guard s.info.root == root else {
                try? await s.release()
                throw Bridge.error(ESTALE, "OtherDrive", "the drive's root changed: mount it again")
            }
            s.listen { [weak self, weak s] event in
                guard let self, let s else { return }
                self.relay(event, from: s)
            }
            let old: BridgeSession = lock.withLock { defer { session = s }; return session }
            try? await old.release()
        }
        startMmapPushes()
        log.notice("activate \(taskOptions.taskOptions, privacy: .public): drive \(self.drive, privacy: .public), root inode \(self.root); wanted attributes: lookup \(String(FSLookupItemResult.requestedAttributes.wantedAttributes.rawValue, radix: 16), privacy: .public), getattr \(String(FSGetAttributesResult.requestedAttributes.wantedAttributes.rawValue, radix: 16), privacy: .public), read \(String(FSReadFileResult.requestedAttributes.wantedAttributes.rawValue, radix: 16), privacy: .public)")
        return FSActivateResult(rootItem: rootItem)!
    }

    func deactivate(options: FSDeactivateOptions = []) async throws {
        await stopMmapPushes()
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
        // A cache push can reenter `write` and need the live session. Drain it before marking
        // the volume unmounted or releasing that session.
        await stopMmapPushes()
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

    /// Never called on macOS 27.0.1: not for fsync, F_FULLFSYNC, sync(2) or unmount (step 5,
    /// item 4's measurements). The daemon makes staged bytes durable within a second of a write
    /// instead; should this come, every writable handle is flushed now.
    func synchronize(flags: FSSyncFlags) async throws {
        let handles = lock.withLock { open.values.compactMap(\.handle).filter(\.write) }
        log.notice("synchronize flags \(flags.rawValue): \(handles.count) writable handles")
        var failed: Error?
        for h in handles {
            do { try await h.session.fsync(h.fh) } catch { failed = failed ?? error }
        }
        if let failed { throw failure(failed) }
    }

    /// The kernel's writeThrough grant does not make
    /// unsynchronized mmap stores arrive as write callbacks on the daemon's quiet-period clock.
    private func startMmapPushes() {
        guard !options.readOnly, options.mmapPushMilliseconds > 0 else { return }
        let ms = options.mmapPushMilliseconds
        lock.withLock {
            guard !unmounted, mmapPushTimer == nil else { return }
            let timer = DispatchSource.makeTimerSource(queue: mmapPushes)
            timer.schedule(deadline: .now() + .milliseconds(ms), repeating: .milliseconds(ms), leeway: .milliseconds(min(ms / 10, 100)))
            timer.setEventHandler { [weak self] in self?.pushMmapCandidates() }
            mmapPushTimer = timer
            timer.resume()
        }
        log.notice("mmap page push enabled, interval \(ms) ms; writeThrough unchanged")
    }

    private func stopMmapPushes() async {
        let timer = lock.withLock {
            let timer = mmapPushTimer
            mmapPushTimer = nil
            mmapPushCandidates.removeAll()
            return timer
        }
        timer?.cancel()
        // Dispatch cancellation does not interrupt a handler already in setCacheState. The
        // serial queue's barrier lets its callbacks finish before the session is released.
        await withCheckedContinuation { done in mmapPushes.async { done.resume() } }
    }

    private func rememberMmapPushCandidate(_ item: VoidfsItem) {
        lock.withLock {
            guard mmapPushTimer != nil, !unmounted, item.kind == .file else { return }
            mmapPushCandidates[ObjectIdentifier(item)] = item
        }
    }

    /// Runs only on `mmapPushes`. Never hold `lock` through setCacheState: it may call write,
    /// close or reclaim back into the volume. Recheck each incarnation after taking the snapshot.
    private func pushMmapCandidates() {
        let start = ContinuousClock.now
        let candidates = lock.withLock { Array(mmapPushCandidates.values) }
        var pushed = 0, skipped = 0, errors = 0
        var totalLatency = Duration.zero, maximumLatency = Duration.zero
        for item in candidates {
            let eligible = lock.withLock {
                mmapPushTimer != nil && !unmounted && mmapPushCandidates[ObjectIdentifier(item)] === item
                    // A retired incarnation may still have a bound writable handle. Without
                    // one, handle(_:write:) refuses its page-out rather than opening a new version.
                    && (!item.retired || item.handle?.write == true)
            }
            guard eligible else { skipped += 1; continue }
            let before = ContinuousClock.now
            let error = setCacheState(for: item, cacheMode: .readWriteWithCache, coherencyType: VolumeOptions.writeCoherency, action: .push)
            let latency = ContinuousClock.now - before
            pushed += 1
            totalLatency += latency
            maximumLatency = max(maximumLatency, latency)
            if let error {
                errors += 1
                let e = error as NSError
                log.error("mmap page push \(item.ino) failed in \(latency, privacy: .public): \(e.domain, privacy: .public) \(e.code)")
            }
        }
        if options.trace {
            log.notice("mmap page push sweep: \(candidates.count) candidates, \(pushed) pushed, \(skipped) skipped, \(errors) errors, elapsed \(ContinuousClock.now - start, privacy: .public), push total \(totalLatency, privacy: .public), push max \(maximumLatency, privacy: .public)")
        }
    }

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
            let s = try await client.open(drive: drive, readOnly: options.readOnly)
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
        let local = event.local && !event.resync && !event.all
        if back { log.notice("the feed delivered changes again at sequence \(event.seq): the mount is current again") }
        if event.resync || event.all {
            // The feed reconnected or lost track: a missed generation means relisting everything.
            resync("resync at generation \(event.generation)")
            return
        }
        if options.trace { log.notice("event \(local ? "local" : "remote", privacy: .public) seq \(event.seq) gen \(event.generation) inodes \(event.inodes, privacy: .public) objects \(event.objects, privacy: .public)") }
        if local { changedHere(event) } else { invalidate(event) }
    }

    /// A change made on this Mac, through any session of the drive's core: everything already
    /// shows it (the core's shared local view), so nothing is revoked, except a folder holding a
    /// name the kernel was told is missing and another session (the CLI, say) made since: the
    /// volume's own creates and renames forget their misses first (`naming`). Listings that name
    /// the change go, and open files read their attributes again.
    private func changedHere(_ event: BridgeEvent) {
        let changed = Set((event.objects + event.subtrees).flatMap(Self.components))
        let targets: [VoidfsItem] = lock.withLock {
            epoch += 1
            var targets: [VoidfsItem] = []
            for ino in event.inodes {
                if let l = listings.removeValue(forKey: ino) { listed -= l.names.count }
                for item in open.values where item.ino == ino { item.stale = true }
                if let below = purge(ino, changed), let folder = items[ino] { targets += below + [folder] }
            }
            var seen = Set<ObjectIdentifier>()
            return targets.filter { seen.insert(ObjectIdentifier($0)).inserted }
        }
        revoke(targets)
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
                    purged += purge(ino, changed) ?? []
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

    /// When `changed` holds a name the kernel was told is missing in folder `ino`: everything it
    /// may hold below the folder, to revoke before the folder. Called with the lock held.
    private func purge(_ ino: UInt64, _ changed: Set<String>) -> [VoidfsItem]? {
        guard let missed = misses[ino], missed.matches(changed) else { return nil }
        misses[ino] = nil
        if ino == root {
            log.notice("a name missed in the drive's top folder now exists: it shows by path once the drive is mounted again")
            return nil
        }
        if openBelow(ino) { pending.insert(ino) }
        return subtree(ino)
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

    /// An open incarnation's attributes are those bound at open, as this Mac's changes leave them
    /// (the core's shared local view); others are current.
    private func attr(_ item: VoidfsItem) async throws -> BridgeAttr {
        let (h, stale) = lock.withLock { (item.handle, item.stale) }
        guard let h else { return try await call { try await $0.getattr(item.ino) } }
        guard stale else { return h.attr }
        lock.withLock { item.stale = false }
        do {
            let a = try await h.session.handleAttr(h.fh)
            settle(item, h, a)
            return a
        } catch {
            lock.withLock { item.stale = true }
            if Bridge.errno(error) == ESTALE || Bridge.errno(error) == EBADF { return h.attr }
            throw failure(error)
        }
    }

    /// Keeps `a` as the attributes of the item's handle `h`, if it still has it.
    private func settle(_ item: VoidfsItem, _ h: OpenFile, _ a: BridgeAttr) {
        lock.withLock {
            guard item.handle?.fh == h.fh, item.handle?.session === h.session else { return }
            item.handle?.attr = a
        }
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

    /// The kernel learns of `name` in `folder` from this call's reply, not from a lookup, so a
    /// miss it was told of needs no purge when the change's own event comes back. Forgotten before
    /// the call (its event may come first), and remembered again if the call fails.
    private func naming<T>(_ name: String, in folder: UInt64, _ body: () async throws -> T) async throws -> T {
        let was = lock.withLock { misses[folder]?.names.remove(name) != nil }
        do { return try await body() } catch {
            if was { missed(name, in: folder) }
            throw error
        }
    }

    func reclaimItem(_ item: FSItem) async throws {
        guard let item = item as? VoidfsItem, item !== rootItem else { return }
        let handle: OpenFile? = lock.withLock {
            if items[item.ino] === item { items[item.ino] = nil }
            open[ObjectIdentifier(item)] = nil
            mmapPushCandidates[ObjectIdentifier(item)] = nil
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

    // MARK: - Opens, reads and writes

    /// The incarnation's daemon handle: opened at its first open (or read), and kept until the
    /// kernel closes or reclaims it. A write open, or a write that arrives without one (pages
    /// pushed after the last close), turns it into a writable handle. Its version may be newer
    /// than the read handle's, by this Mac's own publications: what the kernel caches is still
    /// what the core shows. A retired incarnation (another computer's version replaced it, or it
    /// was removed here) gets no new handle: its cached pages are not of what the core shows.
    private func handle(_ item: VoidfsItem, write: Bool = false) async throws -> OpenFile {
        while true {
            let task: Task<OpenFile, Error> = try lock.withLock {
                if let h = item.handle, h.write || !write { return Task { h } }
                if let t = item.opening { return t }
                if item.retired, write {
                    log.error("write \(item.ino): the file was replaced or removed since the kernel cached it; refused")
                    throw posix(ESTALE)
                }
                let old = item.handle
                let t = Task {
                    do {
                        let f = try await self.call { s in
                            let (fh, a) = try await s.open(item.ino, write: write)
                            return OpenFile(fh: fh, attr: a, session: s, write: write)
                        }
                        self.lock.withLock {
                            item.handle = f
                            item.opening = nil
                            item.stale = old != nil
                            self.open[ObjectIdentifier(item)] = item
                        }
                        if let old { try? await old.session.close(old.fh) }
                        return f
                    } catch {
                        self.lock.withLock { item.opening = nil }
                        throw error
                    }
                }
                item.opening = t
                return t
            }
            let f = try await task.value
            if f.write || !write { return f }
        }
    }

    func open(_ item: FSItem, modes: FSVolume.OpenModes, cacheMode: FSVolume.DataCacheMode, context: FSContext) async throws -> FSOpenItemResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        let write = modes.contains(.write)
        if write && options.readOnly { throw posix(EROFS) }
        if item.kind == .file, lock.withLock({ item.mapped }) { await push(item) }
        if item.kind == .file {
            _ = try await handle(item, write: write)
            if write { rememberMmapPushCandidate(item) }
        }
        // A folder's listing is the extension's to cache (`listings`, kept by the relay): the
        // kernel's, once granted, outlives every revoke of the top folder (see `revoke`).
        let granted: FSVolume.KernelCacheCoherencyType
        if item.kind == .folder { granted = .noCache }
        else if write { granted = VolumeOptions.grant(cacheMode) }
        else { granted = cacheMode == .none ? .noCache : .readCache }
        if options.trace { log.notice("open \(item.ino) modes \(modes.rawValue) cache \(cacheMode.rawValue) granted \(granted.rawValue)") }
        return FSOpenItemResult(grantedCoherency: granted)
    }

    /// Pushes the kernel's dirty pages of `item` to `write`. Measured on macOS 27.0.1, with every
    /// grant and with kernel caching left to FSKit: once a shared mapping's pages are pushed with
    /// no descriptor open (msync after close), the mapping's later stores are dropped at the next
    /// open of the file unless something pushed them first (sync(2), the periodic sync, unmount).
    /// So that open pushes them first.
    private func push(_ item: VoidfsItem) async {
        let start = ContinuousClock.now
        let error: Error? = await withCheckedContinuation { done in
            pushes.async { done.resume(returning: self.setCacheState(for: item, cacheMode: .readWriteWithCache, coherencyType: VolumeOptions.writeCoherency, action: .push)) }
        }
        if let error {
            let e = error as NSError
            log.error("push \(item.ino) before an open failed, a mapping's latest stores may be lost: \(e.domain, privacy: .public) \(e.code)")
        } else if options.trace {
            log.notice("push \(item.ino) before an open, in \(ContinuousClock.now - start, privacy: .public)")
        }
    }

    func close(_ item: FSItem, context: FSContext) async {
        guard let item = item as? VoidfsItem else { return }
        let handle: OpenFile? = lock.withLock {
            open[ObjectIdentifier(item)] = nil
            defer { item.handle = nil }
            return item.handle
        }
        let start = ContinuousClock.now
        if let handle {
            do { try await handle.session.close(handle.fh) }
            catch { log.error("close \(item.ino): \(error.localizedDescription, privacy: .public)") }
        }
        if options.trace { log.notice("close \(item.ino) \(handle?.write == true ? "writable" : "read", privacy: .public) in \(ContinuousClock.now - start, privacy: .public)") }
        released(item)
    }

    func upgrade(_ item: FSItem, cacheMode: FSVolume.DataCacheMode, context: FSContext) async throws -> FSUpgradeItemResult {
        let write = (item as? VoidfsItem).flatMap { i in lock.withLock { i.handle?.write } } ?? false
        let granted = write ? VolumeOptions.grant(cacheMode) : cacheMode == .none ? .noCache : .readCache
        if options.trace { log.notice("upgrade \((item as? VoidfsItem)?.ino ?? 0) cache \(cacheMode.rawValue) granted \(granted.rawValue)") }
        return FSUpgradeItemResult(grantedCoherency: granted)
    }

    func read(from item: FSItem, at offset: off_t, length: Int, into buffer: FSMutableFileDataBuffer) async throws -> FSReadFileResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        var f = try await handle(item)
        if lock.withLock({ item.stale }) { f.attr = try await attr(item) }
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

    /// Stages the bytes in the daemon's core: the write is acknowledged once they survive a crash
    /// of this process, the agent or the daemon (step 5 §1).
    func write(contents: Data, to item: FSItem, at offset: off_t) async throws -> FSWriteFileResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        if options.readOnly { throw posix(EROFS) }
        guard offset >= 0 else { throw posix(EINVAL) }
        let start = ContinuousClock.now
        let opened = lock.withLock { item.handle?.write == true }
        var f = try await handle(item, write: true)
        if !opened {
            lock.withLock { item.mapped = true }
            rememberMmapPushCandidate(item)
            log.notice("write \(item.ino) @\(offset)+\(contents.count) without a write open: a writable handle opened for it")
        }
        var a: BridgeAttr
        do {
            if !f.session.isWatching { f = try await reopen(item, f) }
            do {
                a = try await stage(f, offset, contents)
            } catch let error where Bridge.errno(error) == ESTALE && !f.session.isWatching {
                f = try await reopen(item, f)
                a = try await stage(f, offset, contents)
            }
        } catch {
            log.error("write \(item.ino) @\(offset)+\(contents.count): \(error.localizedDescription, privacy: .public)")
            throw failure(error)
        }
        settle(item, f, a)
        if options.trace { log.notice("write \(item.ino) @\(offset)+\(contents.count) staged in \(ContinuousClock.now - start, privacy: .public), size \(a.size)") }
        return FSWriteFileResult(bytesWritten: contents.count, itemAttributes: attributes(a, parent: lock.withLock { item.parent }), freeSpace: FSFreeSpace.noUpdate)!
    }

    /// Writes `data` at `offset` in pieces of at most `maxIo`, and returns the attributes after.
    private func stage(_ f: OpenFile, _ offset: off_t, _ data: Data) async throws -> BridgeAttr {
        let max = Int(f.session.info.maxIo)
        var at = 0
        while at < data.count {
            let piece = data.subdata(in: at..<min(at + max, data.count))
            let n = try await f.session.write(f.fh, offset: UInt64(offset) + UInt64(at), data: piece)
            guard n == piece.count else { throw posix(EIO) }
            at += n
        }
        return try await f.session.handleAttr(f.fh)
    }

    private func reopen(_ item: VoidfsItem, _ old: OpenFile) async throws -> OpenFile {
        let f = try await call { s in
            let (fh, a) = try await s.open(item.ino, write: old.write)
            return OpenFile(fh: fh, attr: a, session: s, write: old.write)
        }
        // A writable handle's own staged bytes outlive the daemon; its size is theirs.
        guard f.attr.objectId == old.attr.objectId, f.attr.versionId == old.attr.versionId, old.write || f.attr.size == old.attr.size else {
            try? await f.session.close(f.fh)
            log.notice("\(old.write ? "write" : "read", privacy: .public) \(item.ino): its version changed while the daemon was away; the open file is stale")
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
        var names = try await call { try await $0.listxattr(item.ino) }
        // APFS treats the fixed-size, all-zero FinderInfo structure as absent. FSKit's
        // kernel also returns ENOATTR for that value, but doesn't remove its name from
        // the module's list. Match APFS's listing without changing stored bytes/history.
        // Other empty or all-zero xattrs are ordinary values and must remain visible.
        if names.contains("com.apple.FinderInfo") {
            let value = try await call { try await $0.getxattr(item.ino, "com.apple.FinderInfo") }
            if value.count == 32 && value.allSatisfy({ $0 == 0 }) {
                names.removeAll { $0 == "com.apple.FinderInfo" }
            }
        }
        if options.trace { log.notice("listxattr \(item.ino) \(names.count) names \(names.joined(separator: ","), privacy: .public)") }
        return FSListXattrsResult(xattrNames: names.map { FSFileName(string: $0) })!
    }

    /// Probe trace: a value's SHA-256, to show whether the module sees it whole.
    private func digest(_ value: Data?) -> String {
        guard let value else { return "-" }
        return SHA256.hash(data: value).map { String(format: "%02x", $0) }.joined()
    }

    func xattr(named name: FSFileName, of item: FSItem, context: FSContext) async throws -> FSGetXattrResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        guard let n = name.string, try await attr(item).hasXattrs else {
            if options.trace { log.notice("getxattr \(item.ino) \(name.string ?? "?", privacy: .public) absent") }
            throw posix(ENOATTR)
        }
        let start = ContinuousClock.now
        do {
            let value = try await call { try await $0.getxattr(item.ino, n) }
            if options.trace { log.notice("getxattr \(item.ino) \(n, privacy: .public) \(value.count) bytes sha256 \(self.digest(value), privacy: .public) in \(ContinuousClock.now - start, privacy: .public)") }
            return FSGetXattrResult(xattrValue: value)!
        } catch {
            if options.trace { log.notice("getxattr \(item.ino) \(n, privacy: .public) failed errno \(Bridge.errno(error))") }
            throw error
        }
    }

    /// Every name is stored, so the kernel never falls back to AppleDouble `._` files.
    func setXattr(named name: FSFileName, to value: Data?, on item: FSItem, policy: FSVolume.SetXattrPolicy, context: FSContext) async throws -> FSSetXattrResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        if options.readOnly { throw posix(EROFS) }
        guard let n = name.string else { throw posix(EINVAL) }
        if options.trace { log.notice("setxattr \(item.ino) \(n, privacy: .public) policy \(policy.rawValue) \(value?.count ?? -1) bytes sha256 \(self.digest(value), privacy: .public)") }
        let start = ContinuousClock.now
        do {
            if policy == .delete {
                try await call { try await $0.removexattr(item.ino, n) }
            } else {
                guard let value else { throw posix(EINVAL) }
                let how = policy == .mustCreate ? 1 : policy == .mustReplace ? 2 : 0
                try await call { try await $0.setxattr(item.ino, n, value, how: how) }
            }
        } catch {
            if options.trace { log.notice("setxattr \(item.ino) \(n, privacy: .public) failed errno \(Bridge.errno(error))") }
            throw error
        }
        if options.trace { log.notice("setxattr \(item.ino) \(n, privacy: .public) stored in \(ContinuousClock.now - start, privacy: .public)") }
        lock.withLock { item.stale = item.handle != nil }
        return FSSetXattrResult(freeSpace: FSFreeSpace.noUpdate)!
    }

    // MARK: - Names and attributes changed here

    /// Forgets the listings of `folders` before and after a change of this volume's, so that no
    /// listing fetched across it is kept (the relay's event for it may come later).
    private func changing<T>(_ folders: [UInt64], _ body: () async throws -> T) async throws -> T {
        func forget() {
            lock.withLock {
                epoch += 1
                for ino in folders { if let l = listings.removeValue(forKey: ino) { listed -= l.names.count } }
            }
        }
        forget()
        defer { forget() }
        return try await body()
    }

    private func folderAttributes(_ dir: VoidfsItem) async throws -> FSItem.Attributes {
        attributes(try await call { try await $0.getattr(dir.ino) }, parent: lock.withLock { dir.parent })
    }

    func createItem(named name: FSFileName, type: FSItem.ItemType, in directory: FSItem, attributes newAttributes: FSItem.SetAttributesRequest, context: FSContext) async throws -> FSCreateItemResult {
        guard let dir = directory as? VoidfsItem else { throw posix(EINVAL) }
        if options.readOnly { throw posix(EROFS) }
        guard let n = name.string else { throw posix(EINVAL) }
        guard type == .file || type == .directory else { throw posix(ENOTSUP) }
        let folder = type == .directory
        let mode = newAttributes.isValid(.mode) ? newAttributes.mode & 0o7777 : folder ? 0o755 : 0o644
        var a = try await naming(n, in: dir.ino) { try await changing([dir.ino]) { try await call { try await $0.create(dir.ino, n, mode: mode, directory: folder) } } }
        var consumed: FSItem.Attribute = newAttributes.isValid(.mode) ? [.mode] : []
        if newAttributes.isValid(.modifyTime) {
            let t = newAttributes.modifyTime
            a = try await call { try await $0.setattr(a.ino, mtime: (Int64(t.tv_sec), Int32(t.tv_nsec))) }
            consumed.insert(.modifyTime)
        }
        newAttributes.consumedAttributes = consumed
        if options.trace { log.notice("create \(folder ? "folder" : "file", privacy: .public) \(a.ino) in \(dir.ino) mode \(String(mode, radix: 8), privacy: .public) asked \(newAttributes.isValid(.size) ? "size " : "", privacy: .public)\(newAttributes.isValid(.uid) ? "uid " : "", privacy: .public)\(newAttributes.isValid(.flags) ? "flags " : "", privacy: .public)\(newAttributes.isValid(.birthTime) ? "birth " : "", privacy: .public)") }
        let item = incarnation(a, parent: dir.ino)
        return FSCreateItemResult(newItem: item, newItemName: name, newItemAttributes: attributes(a, parent: dir.ino), directoryAttributes: try await folderAttributes(dir), freeSpace: FSFreeSpace.noUpdate)!
    }

    func createSymbolicLink(named name: FSFileName, in directory: FSItem, attributes newAttributes: FSItem.SetAttributesRequest, linkContents contents: FSFileName, context: FSContext) async throws -> FSCreateSymlinkResult {
        if options.readOnly { throw posix(EROFS) }
        throw posix(ENOTSUP)
    }

    func createLink(to item: FSItem, named name: FSFileName, in directory: FSItem, context: FSContext) async throws -> FSCreateLinkResult {
        guard let item = item as? VoidfsItem, let dir = directory as? VoidfsItem, let n = name.string else { throw posix(EINVAL) }
        if options.readOnly { throw posix(EROFS) }
        // Refused by the core (`ENOTSUP`): one error mapping for every callback.
        let a = try await call { try await $0.link(item.ino, dir.ino, n) }
        return FSCreateLinkResult(linkName: name, linkAttributes: attributes(a, parent: dir.ino), directoryAttributes: try await folderAttributes(dir), freeSpace: FSFreeSpace.noUpdate)!
    }

    func renameItem(_ item: FSItem, inDirectory sourceDirectory: FSItem, named sourceName: FSFileName, to destinationName: FSFileName, inDirectory destinationDirectory: FSItem, overItem: FSItem?, context: FSContext) async throws -> FSRenameItemResult {
        guard let item = item as? VoidfsItem, let from = sourceDirectory as? VoidfsItem, let to = destinationDirectory as? VoidfsItem,
              let fromName = sourceName.string, let toName = destinationName.string else { throw posix(EINVAL) }
        if options.readOnly { throw posix(EROFS) }
        let over = overItem as? VoidfsItem
        var overAttr: BridgeAttr?
        if let over { overAttr = try await attr(over) }
        if options.trace { log.notice("rename \(item.ino) \(from.ino)/\(fromName, privacy: .public) → \(to.ino)/\(toName, privacy: .public) over \(over?.ino ?? 0)") }
        try await naming(toName, in: to.ino) { try await changing([from.ino, to.ino]) { try await call { try await $0.rename(from.ino, fromName, to.ino, toName) } } }
        lock.withLock {
            item.parent = to.ino
            item.stale = item.handle != nil
            if let over { retire(over) }
        }
        return FSRenameItemResult(newName: destinationName, renamedItemAttributes: attributes(try await attr(item), parent: to.ino),
                                  sourceDirectoryAttributes: try await folderAttributes(from), destinationDirectoryAttributes: try await folderAttributes(to),
                                  overItemAttributes: overAttr.map { attributes($0, parent: to.ino) }, freeSpace: FSFreeSpace.noUpdate)!
    }

    /// The name goes; the item stays readable and writable through descriptors opened before
    /// (the core keeps an unlinked file's bytes until its last handle closes).
    func removeItem(_ item: FSItem, named name: FSFileName, from directory: FSItem, context: FSContext) async throws -> FSRemoveItemResult {
        guard let item = item as? VoidfsItem, let dir = directory as? VoidfsItem, let n = name.string else { throw posix(EINVAL) }
        if options.readOnly { throw posix(EROFS) }
        let before = try await attr(item)
        if options.trace { log.notice("remove \(item.ino) \(dir.ino)/\(n, privacy: .public)") }
        try await changing([dir.ino, item.ino]) { try await call { try await $0.remove(dir.ino, n, directory: item.kind == .folder) } }
        lock.withLock { retire(item) }
        return FSRemoveItemResult(itemAttributes: attributes(before, parent: dir.ino), directoryAttributes: try await folderAttributes(dir), freeSpace: FSFreeSpace.noUpdate)!
    }

    /// Size through the item's writable handle (one is opened if the file isn't open for
    /// writing), and mode and modification time through the core's `setattr`. Ownership, flags
    /// and the other times aren't stored: they aren't consumed, and FSKit answers for them.
    func setAttributes(_ newAttributes: FSItem.SetAttributesRequest, on item: FSItem, context: FSContext) async throws -> FSSetAttributesResult {
        guard let item = item as? VoidfsItem else { throw posix(EINVAL) }
        if options.readOnly { throw posix(EROFS) }
        var consumed: FSItem.Attribute = []
        if newAttributes.isValid(.size) {
            if item.kind == .file {
                let f = try await handle(item, write: true)
                try await f.session.truncate(f.fh, size: newAttributes.size)
                lock.withLock { item.stale = true }
            }
            consumed.insert(.size)
        }
        let mode = newAttributes.isValid(.mode) ? newAttributes.mode & 0o7777 : nil
        let t = newAttributes.isValid(.modifyTime) ? newAttributes.modifyTime : nil
        if mode != nil || t != nil {
            let a = try await call { try await $0.setattr(item.ino, mode: mode, mtime: t.map { (Int64($0.tv_sec), Int32($0.tv_nsec)) }) }
            if mode != nil { consumed.insert(.mode) }
            if t != nil { consumed.insert(.modifyTime) }
            lock.withLock { item.stale = item.handle != nil }
            if lock.withLock({ item.handle == nil }) {
                newAttributes.consumedAttributes = consumed
                return FSSetAttributesResult(attributes: attributes(a, parent: lock.withLock { item.parent }), freeSpace: FSFreeSpace.noUpdate)!
            }
        }
        if options.trace {
            let asked = [(FSItem.Attribute.size, "size"), (.mode, "mode"), (.uid, "uid"), (.gid, "gid"), (.flags, "flags"), (.accessTime, "atime"), (.modifyTime, "mtime"), (.changeTime, "ctime"), (.birthTime, "birth"), (.backupTime, "backup"), (.addedTime, "added")]
                .filter { newAttributes.isValid($0.0) }.map(\.1).joined(separator: " ")
            log.notice("setattr \(item.ino): \(asked, privacy: .public)")
        }
        newAttributes.consumedAttributes = consumed
        return FSSetAttributesResult(attributes: attributes(try await attr(item), parent: lock.withLock { item.parent }), freeSpace: FSFreeSpace.noUpdate)!
    }
}
