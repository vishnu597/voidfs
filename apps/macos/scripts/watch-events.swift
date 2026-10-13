// SPDX-License-Identifier: Apache-2.0
// Build: xcrun swiftc -swift-version 5 watch-events.swift -o /tmp/watch-events
// Run: watch-events local <scratch-parent> [fsevents|kqueue|both] [case-name]
//      watch-events peer <scratch-parent> <mode> <mount-root> <mount_peer> <drive> [case-name]
// Peer credentials are inherited from the caller's environment, never printed.
// A fresh tree is retained. Exit 1 reports missing events/visibility; exit 3 reports
// a local acknowledged write with wrong bytes. No missing-event result is a pass.

import CoreServices
import CryptoKit
import Darwin
import Foundation

let fm = FileManager.default
let outputLock = NSLock()
func emit(_ values: [String: Any]) {
    var object = values
    object["time"] = Date().timeIntervalSince1970
    let data = try! JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    outputLock.lock()
    print(String(decoding: data, as: UTF8.self))
    fflush(stdout)
    outputLock.unlock()
}
func sha(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
func errorObject(_ error: Error) -> [String: Any] {
    let e = error as NSError
    return ["domain": e.domain, "code": e.code, "message": e.localizedDescription]
}
func posixError() -> Error { NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }

let fsFlags: [(UInt32, String)] = [
    (UInt32(kFSEventStreamEventFlagMustScanSubDirs), "mustScanSubDirs"), (UInt32(kFSEventStreamEventFlagUserDropped), "userDropped"),
    (UInt32(kFSEventStreamEventFlagKernelDropped), "kernelDropped"), (UInt32(kFSEventStreamEventFlagRootChanged), "rootChanged"),
    (UInt32(kFSEventStreamEventFlagItemCreated), "created"), (UInt32(kFSEventStreamEventFlagItemRemoved), "removed"),
    (UInt32(kFSEventStreamEventFlagItemRenamed), "renamed"), (UInt32(kFSEventStreamEventFlagItemModified), "modified"),
    (UInt32(kFSEventStreamEventFlagItemInodeMetaMod), "inodeMetadata"), (UInt32(kFSEventStreamEventFlagItemXattrMod), "xattr"),
    (UInt32(kFSEventStreamEventFlagItemIsFile), "file"), (UInt32(kFSEventStreamEventFlagItemIsDir), "directory"),
    (UInt32(kFSEventStreamEventFlagMount), "mount"), (UInt32(kFSEventStreamEventFlagUnmount), "unmount")]
let vnodeFlags: [(UInt32, String)] = [(UInt32(NOTE_WRITE), "write"), (UInt32(NOTE_EXTEND), "extend"), (UInt32(NOTE_ATTRIB), "attrib"),
                                    (UInt32(NOTE_LINK), "link"), (UInt32(NOTE_RENAME), "rename"), (UInt32(NOTE_DELETE), "delete"), (UInt32(NOTE_REVOKE), "revoke")]

struct Notification {
    let source: String
    let time: Double
    let flags: UInt32
    let path: String
}

final class Watcher {
    let folder: URL
    let caseName: String
    let mode: String
    let lock = NSLock()
    let queue = DispatchQueue(label: "dev.voidfs.matrix.fsevents")
    var notifications: [Notification] = []
    var stream: FSEventStreamRef?
    var kq: Int32 = -1
    var descriptors: [Int32: String] = [:]

    init(folder: URL, caseName: String, mode: String) throws {
        self.folder = folder
        self.caseName = caseName
        self.mode = mode
        if mode != "kqueue" {
            var context = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(), retain: nil, release: nil, copyDescription: nil)
            let flags = FSEventStreamCreateFlags(kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer)
            stream = FSEventStreamCreate(nil, { _, info, count, paths, flags, ids in
                guard let info else { return }
                let watcher = Unmanaged<Watcher>.fromOpaque(info).takeUnretainedValue()
                let array = unsafeBitCast(paths, to: NSArray.self)
                for index in 0..<count {
                    let path = array[index] as! String
                    watcher.record(source: "fsevents", path: path, flags: flags[index], id: ids[index])
                }
            }, &context, [folder.path] as CFArray, FSEventStreamEventId(kFSEventStreamEventIdSinceNow), 0.05, flags)
            guard let stream else { throw NSError(domain: "WatchEventsProbe", code: 1, userInfo: [NSLocalizedDescriptionKey: "FSEventStreamCreate returned nil"]) }
            FSEventStreamSetDispatchQueue(stream, queue)
            guard FSEventStreamStart(stream) else { throw NSError(domain: "WatchEventsProbe", code: 2, userInfo: [NSLocalizedDescriptionKey: "FSEventStreamStart failed"]) }
        }
        if mode != "fsevents" {
            kq = kqueue()
            guard kq >= 0 else { throw posixError() }
            var watched = [(folder, "directory"), (folder.appendingPathComponent("original.bin"), "original-file")]
            if caseName == "nested-create" { watched.append((folder.appendingPathComponent("sub", isDirectory: true), "subdirectory")) }
            for (url, label) in watched {
                let fd = open(url.path, O_EVTONLY)
                guard fd >= 0 else { throw posixError() }
                descriptors[fd] = label
                var change = kevent(ident: UInt(fd), filter: Int16(EVFILT_VNODE), flags: UInt16(EV_ADD | EV_CLEAR),
                                    fflags: UInt32(NOTE_WRITE | NOTE_EXTEND | NOTE_ATTRIB | NOTE_LINK | NOTE_RENAME | NOTE_DELETE | NOTE_REVOKE), data: 0, udata: nil)
                guard kevent(kq, &change, 1, nil, 0, nil) == 0 else { throw posixError() }
            }
        }
    }

    func record(source: String, path: String, flags: UInt32, id: UInt64? = nil) {
        let time = Date().timeIntervalSince1970
        lock.lock()
        notifications.append(Notification(source: source, time: time, flags: flags, path: path))
        lock.unlock()
        let decoded = (source == "fsevents" ? fsFlags : vnodeFlags).filter { flags & $0.0 != 0 }.map { $0.1 }
        var object: [String: Any] = ["kind": "notification", "case": caseName, "source": source, "receivedAt": time,
                                   "flags": flags, "decoded": decoded, "watch": path]
        if let id { object["eventId"] = id }
        emit(object)
    }

    func poll() throws {
        if kq < 0 { return }
        var timeout = timespec(tv_sec: 0, tv_nsec: 0)
        var events = Array(repeating: kevent(), count: 16)
        let count = kevent(kq, nil, 0, &events, Int32(events.count), &timeout)
        guard count >= 0 else { throw posixError() }
        for event in events.prefix(Int(count)) {
            if event.flags & UInt16(EV_ERROR) != 0 { throw NSError(domain: NSPOSIXErrorDomain, code: Int(event.data)) }
            record(source: "kqueue", path: descriptors[Int32(event.ident)] ?? "unknown", flags: event.fflags)
        }
    }

    func first(source: String, after start: Double, paths: Set<String>) -> Notification? {
        lock.lock()
        defer { lock.unlock() }
        return notifications.first {
            guard $0.source == source && $0.time >= start else { return false }
            if source == "kqueue" {
                switch caseName {
                case "grow", "shrink": return $0.path == "original-file" && $0.flags & UInt32(NOTE_WRITE | NOTE_EXTEND) != 0
                case "attributes": return $0.path == "original-file" && $0.flags & UInt32(NOTE_ATTRIB) != 0
                case "rename": return ($0.path == "original-file" && $0.flags & UInt32(NOTE_RENAME) != 0) || ($0.path == "directory" && $0.flags & UInt32(NOTE_WRITE) != 0)
                case "delete": return ($0.path == "original-file" && $0.flags & UInt32(NOTE_DELETE) != 0) || ($0.path == "directory" && $0.flags & UInt32(NOTE_WRITE) != 0)
                case "atomic": return ($0.path == "original-file" && $0.flags & UInt32(NOTE_DELETE | NOTE_RENAME | NOTE_REVOKE) != 0) || ($0.path == "directory" && $0.flags & UInt32(NOTE_WRITE) != 0)
                case "nested-create": return $0.path == "subdirectory" && $0.flags & UInt32(NOTE_WRITE) != 0
                default: return $0.path == "directory" && $0.flags & UInt32(NOTE_WRITE) != 0
                }
            }
            let url = URL(fileURLWithPath: $0.path)
            let path = url.deletingLastPathComponent().resolvingSymlinksInPath().appendingPathComponent(url.lastPathComponent).path
            guard paths.contains(path) else { return false }
            let required: UInt32
            switch caseName {
            case "grow", "shrink": required = UInt32(kFSEventStreamEventFlagItemModified)
            case "attributes": required = UInt32(kFSEventStreamEventFlagItemInodeMetaMod | kFSEventStreamEventFlagItemXattrMod)
            case "rename": required = UInt32(kFSEventStreamEventFlagItemRenamed)
            case "delete": required = UInt32(kFSEventStreamEventFlagItemRemoved)
            case "atomic": required = UInt32(kFSEventStreamEventFlagItemRenamed | kFSEventStreamEventFlagItemCreated | kFSEventStreamEventFlagItemModified)
            default: required = UInt32(kFSEventStreamEventFlagItemCreated)
            }
            return $0.flags & required != 0
        }
    }

    func drainBaseline() throws {
        if let stream { FSEventStreamFlushSync(stream) }
        try poll()
        lock.lock()
        notifications.removeAll()
        lock.unlock()
    }

    func closeWatch() {
        if let stream {
            FSEventStreamStop(stream)
            FSEventStreamInvalidate(stream)
            queue.sync {}
            FSEventStreamRelease(stream)
            self.stream = nil
        }
        for fd in descriptors.keys { close(fd) }
        descriptors.removeAll()
        if kq >= 0 { close(kq); kq = -1 }
    }
    deinit { closeWatch() }
}

let args = CommandLine.arguments
guard args.count >= 3, ["local", "peer"].contains(args[1]) else {
    fputs("usage: watch-events local <scratch-parent> [fsevents|kqueue|both] [case-name]\n       watch-events peer <scratch-parent> <mode> <mount-root> <mount_peer> <drive> [case-name]\n", stderr)
    exit(2)
}
let peerMode = args[1] == "peer"
guard (!peerMode && args.count <= 5) || (peerMode && [7, 8].contains(args.count)) else { fputs("wrong number of arguments\n", stderr); exit(2) }
let mode = args.count > 3 ? args[3] : "fsevents"
guard ["fsevents", "kqueue", "both"].contains(mode) else { fputs("unknown watch mode\n", stderr); exit(2) }
let parent = URL(fileURLWithPath: args[2], isDirectory: true).resolvingSymlinksInPath()
let root = parent.appendingPathComponent("watch-\(args[1])-\(mode)-\(UUID().uuidString.lowercased())", isDirectory: true)
let mount = peerMode ? URL(fileURLWithPath: args[4], isDirectory: true).resolvingSymlinksInPath() : parent
let selectedCase = args.count == (peerMode ? 8 : 5) ? args.last : nil
let caseNames = ["create", "grow", "shrink", "atomic", "attributes", "rename", "delete", "nested-create", "missed-create"]
guard selectedCase == nil || caseNames.contains(selectedCase!) else { fputs("unknown case\n", stderr); exit(2) }
var failures = 0

func peer(_ operation: [String]) throws -> Double {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: args[5]).standardizedFileURL
    process.arguments = [args[6]] + operation
    let output = Pipe()
    let error = Pipe()
    process.standardOutput = output
    process.standardError = error
    try process.run()
    process.waitUntilExit()
    let stdout = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
    let stderr = String(decoding: error.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
    guard process.terminationStatus == 0 else {
        throw NSError(domain: "MountPeer", code: Int(process.terminationStatus), userInfo: [NSLocalizedDescriptionKey: stderr])
    }
    guard let stamp = stdout.split(whereSeparator: { $0.isWhitespace }).last, let time = Double(stamp) else {
        throw NSError(domain: "WatchEventsProbe", code: 3, userInfo: [NSLocalizedDescriptionKey: "peer did not print its successful commit time"])
    }
    return time
}

func key(_ url: URL) throws -> String {
    // Resolving a nonexistent target can create the very negative lookup being measured.
    // The caller's mount and parent were canonicalized before test paths were constructed.
    let path = url.path
    guard path.hasPrefix(mount.path + "/") else { throw NSError(domain: "WatchEventsProbe", code: 4, userInfo: [NSLocalizedDescriptionKey: "scratch path is outside the mount root"]) }
    return String(path.dropFirst(mount.path.count + 1))
}

func fill(_ url: URL, count: Int, byte: UInt8) throws -> Double {
    if peerMode { return try peer(["fill", key(url), String(count), String(byte)]) }
    try Data(repeating: byte, count: count).write(to: url)
    return Date().timeIntervalSince1970
}

func listed(_ url: URL) -> Bool {
    (try? fm.contentsOfDirectory(atPath: url.deletingLastPathComponent().path).contains(url.lastPathComponent)) ?? false
}

do {
    // Local mkdir clears its own cached misses; remote bootstrap of a new top-level folder
    // would mix the documented unrevokeable root miss into every notification row.
    try fm.createDirectory(at: root, withIntermediateDirectories: true)
    emit(["kind": "run", "probe": "watch-events", "writer": args[1], "mode": mode, "folder": root.path,
          "os": ProcessInfo.processInfo.operatingSystemVersionString,
          "note": "FSEvents uses FileEvents/UseCFTypes/NoDefer, without WatchRoot, and opens no watch descriptors in this process. kqueue/both hold the directory and original file with O_EVTONLY: they can pin FSKit cached misses and old versions. The nested-create row also watches its subdirectory because kqueue is not recursive. Event receipt and content visibility are scored separately. kqueue times are userspace poll delivery times."])
    for name in caseNames {
        if let selectedCase, selectedCase != name { continue }
        if peerMode && (name == "attributes" || name == "atomic") {
            emit(["kind": "untested", "case": name, "reason": "mount_peer exposes put/fill/delete/rename, not remote chmod/xattr or an app's local atomic save"])
            continue
        }
        let folder = root.appendingPathComponent(name, isDirectory: true)
        try fm.createDirectory(at: folder, withIntermediateDirectories: true)
        let original = folder.appendingPathComponent("original.bin")
        _ = try fill(original, count: 12, byte: 65)
        let nested = folder.appendingPathComponent("sub", isDirectory: true)
        try fm.createDirectory(at: nested, withIntermediateDirectories: false)
        _ = try fill(nested.appendingPathComponent("held.bin"), count: 1, byte: 72)
        // Setup visibility uses listings before opens so it creates no negative lookup.
        let setupDeadline = Date().timeIntervalSince1970 + 5
        while !listed(original) && Date().timeIntervalSince1970 < setupDeadline { usleep(10_000) }
        guard try Data(contentsOf: original) == Data(repeating: 65, count: 12) else { throw NSError(domain: "WatchEventsProbe", code: 5, userInfo: [NSLocalizedDescriptionKey: "seed bytes not visible before watching"] ) }
        let watcher = try Watcher(folder: folder, caseName: name, mode: mode)
        defer { watcher.closeWatch() }
        usleep(150_000)
        try watcher.drainBaseline()
        let target: URL
        let expected: Data?
        switch name {
        case "create": target = folder.appendingPathComponent("created.bin"); expected = Data(repeating: 66, count: 19)
        case "grow": target = original; expected = Data(repeating: 66, count: 32)
        case "shrink": target = original; expected = Data(repeating: 66, count: 4)
        case "atomic": target = original; expected = Data(repeating: 66, count: 64)
        case "attributes": target = original; expected = Data(repeating: 65, count: 12)
        case "rename": target = folder.appendingPathComponent("renamed.bin"); expected = Data(repeating: 65, count: 12)
        case "delete": target = original; expected = nil
        case "nested-create": target = nested.appendingPathComponent("new.bin"); expected = Data(repeating: 66, count: 23)
        default: target = folder.appendingPathComponent("missed.bin"); expected = Data(repeating: 66, count: 17)
        }
        if name == "missed-create" {
            var missing = stat()
            let rc = lstat(target.path, &missing)
            guard rc != 0 && errno == ENOENT else { throw NSError(domain: "WatchEventsProbe", code: 6, userInfo: [NSLocalizedDescriptionKey: "missed-create seed unexpectedly exists"]) }
        }
        var eventPaths: Set<String> = [target.path]
        if name == "rename" { eventPaths.insert(original.path) }
        emit(["kind": "READY", "case": name, "folder": folder.path, "heldDirectory": mode != "fsevents", "heldOriginalFile": mode != "fsevents"])
        let started = Date().timeIntervalSince1970
        let committed: Double
        switch name {
        case "rename":
            if peerMode { committed = try peer(["rename", key(original), key(target)]) }
            else { try fm.moveItem(at: original, to: target); committed = Date().timeIntervalSince1970 }
        case "delete":
            if peerMode { committed = try peer(["delete", key(original)]) }
            else { try fm.removeItem(at: original); committed = Date().timeIntervalSince1970 }
        case "attributes":
            try fm.setAttributes([.posixPermissions: NSNumber(value: 0o600)], ofItemAtPath: original.path)
            let attribute = Data("watch-metadata".utf8)
            guard attribute.withUnsafeBytes({ setxattr(original.path, "user.voidfs.watch", $0.baseAddress, attribute.count, 0, 0) }) == 0 else { throw posixError() }
            committed = Date().timeIntervalSince1970
        case "atomic": try expected!.write(to: target, options: .atomic); committed = Date().timeIntervalSince1970
        default: committed = try fill(target, count: expected!.count, byte: expected!.first!)
        }
        emit(["kind": "committed", "case": name, "commitReturnedAt": committed, "operationStartedAt": started,
              "expectedSha256": expected.map(sha) as Any? ?? NSNull(), "expectedSize": expected?.count as Any? ?? NSNull()])
        var listingAt: Double?
        var visibleAt: Double?
        let deadline = committed + 5
        repeat {
            try watcher.poll()
            let now = Date().timeIntervalSince1970
            let isListed = listed(target)
            if listingAt == nil && isListed == (expected != nil) { listingAt = now }
            if visibleAt == nil {
                // A normal new-name row must not poll opens before its listing arrives;
                // only missed-create deliberately inserts a negative lookup.
                var visible = expected == nil ? !fm.fileExists(atPath: target.path) : isListed && (try? Data(contentsOf: target)) == expected
                if name == "attributes" { visible = visible && ((try? fm.attributesOfItem(atPath: target.path)[.posixPermissions]) as? NSNumber)?.intValue == 0o600 }
                if visible { visibleAt = now }
            }
            let eventsPresent = (mode == "kqueue" || watcher.first(source: "fsevents", after: started, paths: eventPaths) != nil) &&
                                (mode == "fsevents" || watcher.first(source: "kqueue", after: started, paths: eventPaths) != nil)
            if listingAt != nil && visibleAt != nil && eventsPresent { break }
            usleep(10_000)
        } while Date().timeIntervalSince1970 < deadline
        let fsEvent = watcher.first(source: "fsevents", after: started, paths: eventPaths)
        let vnodeEvent = watcher.first(source: "kqueue", after: started, paths: eventPaths)
        let eventsPresent = (mode == "kqueue" || fsEvent != nil) && (mode == "fsevents" || vnodeEvent != nil)
        let knownHeld = peerMode && mode != "fsevents"
        let ok = eventsPresent && listingAt != nil && visibleAt != nil
        if !ok { failures += 1 }
        let actual = try? Data(contentsOf: target)
        emit(["kind": "result", "case": name, "ok": ok, "writer": args[1], "mode": mode,
              "fseventsMs": fsEvent.map { ($0.time - committed) * 1000 } as Any? ?? NSNull(),
              "kqueueMs": vnodeEvent.map { ($0.time - committed) * 1000 } as Any? ?? NSNull(),
              "listingMs": listingAt.map { ($0 - committed) * 1000 } as Any? ?? NSNull(),
              "visibleBytesMs": visibleAt.map { ($0 - committed) * 1000 } as Any? ?? NSNull(),
              "actualSha256": actual.map(sha) as Any? ?? NSNull(), "actualSize": actual?.count as Any? ?? NSNull(),
              "knownHeldVnodeVisibilityLimit": knownHeld && visibleAt == nil,
              "missingNotification": !eventsPresent, "windowSeconds": 5])
        if !peerMode && visibleAt == nil && actual != expected {
            emit(["kind": "DATA_LOSS", "case": name, "check": "local acknowledged operation not visible with expected bytes", "evidenceFolder": folder.path])
            exit(3)
        }
        watcher.closeWatch()
        if peerMode && visibleAt == nil {
            let released = Date().timeIntervalSince1970
            var found = false
            repeat {
                found = expected == nil ? !fm.fileExists(atPath: target.path) : (try? Data(contentsOf: target)) == expected
                if found { break }
                usleep(10_000)
            } while Date().timeIntervalSince1970 - released < 5
            emit(["kind": "after-release", "case": name, "visible": found,
                  "releasedToVisibleMs": found ? (Date().timeIntervalSince1970 - released) * 1000 : NSNull()])
        }
    }
    emit(["kind": "summary", "failures": failures, "folder": root.path, "preserved": true])
    exit(failures == 0 ? 0 : 1)
} catch {
    emit(["kind": "harness-error", "error": errorObject(error), "folder": root.path])
    exit(2)
}
