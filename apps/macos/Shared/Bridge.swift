// SPDX-License-Identifier: Apache-2.0
//
// The XPC bridge between the sandboxed FSKit extension and the app's launch agent, which forwards
// each call to the Rust daemon's user-only socket (`/v1/fs`) and relays its invalidations. The
// interface is typed and bounded: the extension names a drive, inodes and handles, never a path or
// a socket request, and the agent accepts only the extension's code signature.

import Foundation

enum Bridge {
    /// The agent's Mach service: App-Group-prefixed, the only kind the extension's sandbox reaches.
    static let service = "\(MountStore.appGroup).agent"
    /// Who the agent accepts: the extension, signed by voidfs's team.
    static let peerRequirement = "anchor apple generic and certificate leaf[subject.OU] = \"HAUTK68F56\" and identifier \"dev.voidfs.app.fskit\""
    /// The daemon's own bounds (`crates/voidfs-daemon/src/fs_api.rs`), checked here before a call
    /// crosses the socket.
    static let maxIo: UInt64 = 8 << 20
    static let maxEntries = 256
    static let maxXattr = 64 << 10
    static let maxName = 255
    static let maxDrive = 1024
    /// Per extension connection.
    static let maxSessions = 16
    static let maxCalls = 32

    static func interface() -> NSXPCInterface {
        let i = NSXPCInterface(with: VoidfsBridgeProtocol.self)
        let strings = NSSet(array: [NSArray.self, NSString.self]) as! Set<AnyHashable>
        i.setClasses(strings, for: #selector(VoidfsBridgeProtocol.listxattr(_:ino:reply:)), argumentIndex: 0, ofReply: true)
        return i
    }

    static func eventsInterface() -> NSXPCInterface { NSXPCInterface(with: VoidfsBridgeEvents.self) }

    /// A failure as the extension sees it: the native errno, with the daemon's code and message.
    static func error(_ errno: Int32, _ code: String, _ message: String) -> NSError {
        NSError(domain: NSPOSIXErrorDomain, code: Int(errno), userInfo: [NSLocalizedDescriptionKey: message, "code": code])
    }

    /// The errno of any error a bridge call returns. An interrupted or invalid XPC connection is
    /// `EIO` for the call in flight; the client reconnects for the next one.
    static func errno(_ error: Error) -> Int32 {
        let e = error as NSError
        if e.domain == NSPOSIXErrorDomain { return Int32(e.code) }
        return EIO
    }
}

/// What the agent exports. Every call names one of the connection's own sessions by the number
/// `openSession` gave it; the daemon's session ID never reaches the extension.
@objc protocol VoidfsBridgeProtocol {
    /// The bare XPC hop, for measuring.
    func ping(_ payload: Data, reply: @escaping (Data) -> Void)
    func openSession(drive: String, readOnly: Bool, reply: @escaping (BridgeSessionInfo?, NSError?) -> Void)
    func releaseSession(_ session: Int64, reply: @escaping (NSError?) -> Void)
    /// Relays the session's invalidations to the connection's exported `VoidfsBridgeEvents`,
    /// starting with a full resync, until the session is released or the daemon stops.
    func watch(_ session: Int64, reply: @escaping (NSError?) -> Void)
    func lookup(_ session: Int64, parent: UInt64, name: String, reply: @escaping (BridgeAttr?, NSError?) -> Void)
    func getattr(_ session: Int64, ino: UInt64, reply: @escaping (BridgeAttr?, NSError?) -> Void)
    func readlink(_ session: Int64, ino: UInt64, reply: @escaping (String?, NSError?) -> Void)
    func readdir(_ session: Int64, ino: UInt64, after: String?, limit: Int, reply: @escaping (BridgeDirPage?, NSError?) -> Void)
    func open(_ session: Int64, ino: UInt64, write: Bool, reply: @escaping (UInt64, BridgeAttr?, NSError?) -> Void)
    func handleAttr(_ session: Int64, fh: UInt64, reply: @escaping (BridgeAttr?, NSError?) -> Void)
    func read(_ session: Int64, fh: UInt64, offset: UInt64, length: UInt64, reply: @escaping (Data?, NSError?) -> Void)
    func write(_ session: Int64, fh: UInt64, offset: UInt64, data: Data, reply: @escaping (Int, NSError?) -> Void)
    func truncate(_ session: Int64, fh: UInt64, size: UInt64, reply: @escaping (NSError?) -> Void)
    func fsync(_ session: Int64, fh: UInt64, reply: @escaping (NSError?) -> Void)
    func close(_ session: Int64, fh: UInt64, reply: @escaping (NSError?) -> Void)
    /// `create`, or `mkdir` when `directory`.
    func create(_ session: Int64, parent: UInt64, name: String, mode: UInt32, directory: Bool, reply: @escaping (BridgeAttr?, NSError?) -> Void)
    /// `unlink`, or `rmdir` when `directory`.
    func remove(_ session: Int64, parent: UInt64, name: String, directory: Bool, reply: @escaping (NSError?) -> Void)
    /// `how`: 0 replace, 1 exclusive (`RENAME_EXCL`), 2 swap (`RENAME_SWAP`, refused).
    func rename(_ session: Int64, fromParent: UInt64, fromName: String, toParent: UInt64, toName: String, how: Int, reply: @escaping (NSError?) -> Void)
    /// `link`, or `clone_file` when `clone`: both refused with `ENOTSUP` (or `EROFS`), and
    /// forwarded so that one error mapping answers every callback.
    func link(_ session: Int64, ino: UInt64, parent: UInt64, name: String, clone: Bool, reply: @escaping (BridgeAttr?, NSError?) -> Void)
    /// `nil` leaves a value as it is.
    func setattr(_ session: Int64, ino: UInt64, mode: NSNumber?, mtimeSeconds: NSNumber?, mtimeNanoseconds: Int32, reply: @escaping (BridgeAttr?, NSError?) -> Void)
    func getxattr(_ session: Int64, ino: UInt64, name: String, reply: @escaping (Data?, NSError?) -> Void)
    func listxattr(_ session: Int64, ino: UInt64, reply: @escaping ([String]?, NSError?) -> Void)
    /// `how`: 0 set, 1 create (`XATTR_CREATE`), 2 replace (`XATTR_REPLACE`).
    func setxattr(_ session: Int64, ino: UInt64, name: String, value: Data, how: Int, reply: @escaping (NSError?) -> Void)
    func removexattr(_ session: Int64, ino: UInt64, name: String, reply: @escaping (NSError?) -> Void)
}

/// What the extension exports on the same connection, for the agent to call.
@objc protocol VoidfsBridgeEvents {
    func invalidated(_ session: Int64, event: BridgeEvent)
    /// The session's relay ended: released, the daemon stopped, or the stream failed.
    func watchEnded(_ session: Int64, error: NSError?)
}

/// The daemon's `Attr`, with its RFC 3339 time as seconds and nanoseconds.
@objc(VoidfsBridgeAttr) final class BridgeAttr: NSObject, NSSecureCoding {
    @objc enum Kind: Int { case file, folder, symlink, unknown }
    @objc enum SyncState: Int { case saved, pending, saving, conflict, error }

    let ino: UInt64
    let kind: Kind
    let size: UInt64
    let mtimeSeconds: Int64
    let mtimeNanoseconds: Int32
    let mode: UInt32
    let generation: UInt64
    let sync: SyncState
    let hasXattrs: Bool
    let target: String?
    let objectId: String?
    let versionId: String?

    init(ino: UInt64, kind: Kind, size: UInt64, mtimeSeconds: Int64, mtimeNanoseconds: Int32, mode: UInt32, generation: UInt64, sync: SyncState, hasXattrs: Bool, target: String?, objectId: String?, versionId: String?) {
        (self.ino, self.kind, self.size, self.mtimeSeconds, self.mtimeNanoseconds, self.mode) = (ino, kind, size, mtimeSeconds, mtimeNanoseconds, mode)
        (self.generation, self.sync, self.hasXattrs, self.target, self.objectId, self.versionId) = (generation, sync, hasXattrs, target, objectId, versionId)
    }

    static var supportsSecureCoding: Bool { true }

    func encode(with c: NSCoder) {
        c.encode(Int64(bitPattern: ino), forKey: "i")
        c.encode(kind.rawValue, forKey: "k")
        c.encode(Int64(bitPattern: size), forKey: "s")
        c.encode(mtimeSeconds, forKey: "ts")
        c.encode(mtimeNanoseconds, forKey: "tn")
        c.encode(Int64(mode), forKey: "m")
        c.encode(Int64(bitPattern: generation), forKey: "g")
        c.encode(sync.rawValue, forKey: "y")
        c.encode(hasXattrs, forKey: "x")
        c.encode(target, forKey: "l")
        c.encode(objectId, forKey: "o")
        c.encode(versionId, forKey: "v")
    }

    init?(coder c: NSCoder) {
        guard let kind = Kind(rawValue: c.decodeInteger(forKey: "k")), let sync = SyncState(rawValue: c.decodeInteger(forKey: "y")) else { return nil }
        let mode = c.decodeInt64(forKey: "m")
        guard mode >= 0, mode <= 0o7777 else { return nil }
        (self.ino, self.kind, self.size) = (UInt64(bitPattern: c.decodeInt64(forKey: "i")), kind, UInt64(bitPattern: c.decodeInt64(forKey: "s")))
        (self.mtimeSeconds, self.mtimeNanoseconds, self.mode) = (c.decodeInt64(forKey: "ts"), c.decodeInt32(forKey: "tn"), UInt32(mode))
        (self.generation, self.sync, self.hasXattrs) = (UInt64(bitPattern: c.decodeInt64(forKey: "g")), sync, c.decodeBool(forKey: "x"))
        self.target = c.decodeObject(of: NSString.self, forKey: "l") as String?
        self.objectId = c.decodeObject(of: NSString.self, forKey: "o") as String?
        self.versionId = c.decodeObject(of: NSString.self, forKey: "v") as String?
    }
}

/// The daemon's `SessionInfo`, under the connection's own session number.
@objc(VoidfsBridgeSessionInfo) final class BridgeSessionInfo: NSObject, NSSecureCoding {
    let session: Int64
    let drive: String
    let root: UInt64
    let generation: UInt32
    let metadataGeneration: UInt64
    let readOnly: Bool
    let maxIo: UInt64
    let maxEntries: Int
    /// The daemon's `capabilities` object, as JSON, for the adapter to advertise.
    let capabilities: Data

    init(session: Int64, drive: String, root: UInt64, generation: UInt32, metadataGeneration: UInt64, readOnly: Bool, maxIo: UInt64, maxEntries: Int, capabilities: Data) {
        (self.session, self.drive, self.root, self.generation, self.metadataGeneration) = (session, drive, root, generation, metadataGeneration)
        (self.readOnly, self.maxIo, self.maxEntries, self.capabilities) = (readOnly, maxIo, maxEntries, capabilities)
    }

    static var supportsSecureCoding: Bool { true }

    func encode(with c: NSCoder) {
        c.encode(session, forKey: "id")
        c.encode(drive, forKey: "d")
        c.encode(Int64(bitPattern: root), forKey: "r")
        c.encode(Int64(generation), forKey: "g")
        c.encode(Int64(bitPattern: metadataGeneration), forKey: "mg")
        c.encode(readOnly, forKey: "ro")
        c.encode(Int64(bitPattern: maxIo), forKey: "io")
        c.encode(maxEntries, forKey: "e")
        c.encode(capabilities, forKey: "c")
    }

    init?(coder c: NSCoder) {
        guard let drive = c.decodeObject(of: NSString.self, forKey: "d") as String?, let capabilities = c.decodeObject(of: NSData.self, forKey: "c") as Data? else { return nil }
        let generation = c.decodeInt64(forKey: "g")
        guard generation > 0, generation <= Int64(UInt32.max) else { return nil }
        (self.session, self.drive, self.root, self.generation) = (c.decodeInt64(forKey: "id"), drive, UInt64(bitPattern: c.decodeInt64(forKey: "r")), UInt32(generation))
        (self.metadataGeneration, self.readOnly) = (UInt64(bitPattern: c.decodeInt64(forKey: "mg")), c.decodeBool(forKey: "ro"))
        (self.maxIo, self.maxEntries, self.capabilities) = (UInt64(bitPattern: c.decodeInt64(forKey: "io")), c.decodeInteger(forKey: "e"), capabilities)
    }
}

/// One page of a directory, in exclusive UTF-8 name order, with the page's generation.
@objc(VoidfsBridgeDirPage) final class BridgeDirPage: NSObject, NSSecureCoding {
    let names: [String]
    let attrs: [BridgeAttr]
    let generation: UInt64

    init(names: [String], attrs: [BridgeAttr], generation: UInt64) { (self.names, self.attrs, self.generation) = (names, attrs, generation) }

    static var supportsSecureCoding: Bool { true }

    func encode(with c: NSCoder) {
        c.encode(names as NSArray, forKey: "n")
        c.encode(attrs as NSArray, forKey: "a")
        c.encode(Int64(bitPattern: generation), forKey: "g")
    }

    init?(coder c: NSCoder) {
        guard let names = c.decodeArrayOfObjects(ofClass: NSString.self, forKey: "n") as [String]?,
              let attrs = c.decodeArrayOfObjects(ofClass: BridgeAttr.self, forKey: "a"),
              names.count == attrs.count, names.count <= Bridge.maxEntries else { return nil }
        (self.names, self.attrs, self.generation) = (names, attrs, UInt64(bitPattern: c.decodeInt64(forKey: "g")))
    }
}

/// One of the daemon's invalidation events: changed object keys and subtrees, the inodes they
/// touch, or everything (`all`, always with `resync` after a gap or at the start).
@objc(VoidfsBridgeEvent) final class BridgeEvent: NSObject, NSSecureCoding {
    let generation: UInt64
    let seq: UInt64
    let resync: Bool
    let all: Bool
    let objects: [String]
    let subtrees: [String]
    let inodes: [UInt64]

    init(generation: UInt64, seq: UInt64, resync: Bool, all: Bool, objects: [String], subtrees: [String], inodes: [UInt64]) {
        (self.generation, self.seq, self.resync, self.all) = (generation, seq, resync, all)
        (self.objects, self.subtrees, self.inodes) = (objects, subtrees, inodes)
    }

    static var supportsSecureCoding: Bool { true }

    func encode(with c: NSCoder) {
        c.encode(Int64(bitPattern: generation), forKey: "g")
        c.encode(Int64(bitPattern: seq), forKey: "s")
        c.encode(resync, forKey: "r")
        c.encode(all, forKey: "a")
        c.encode(objects as NSArray, forKey: "o")
        c.encode(subtrees as NSArray, forKey: "t")
        c.encode(inodes.map { NSNumber(value: $0) } as NSArray, forKey: "i")
    }

    init?(coder c: NSCoder) {
        guard let objects = c.decodeArrayOfObjects(ofClass: NSString.self, forKey: "o") as [String]?,
              let subtrees = c.decodeArrayOfObjects(ofClass: NSString.self, forKey: "t") as [String]?,
              let inodes = c.decodeArrayOfObjects(ofClass: NSNumber.self, forKey: "i") else { return nil }
        (self.generation, self.seq) = (UInt64(bitPattern: c.decodeInt64(forKey: "g")), UInt64(bitPattern: c.decodeInt64(forKey: "s")))
        (self.resync, self.all, self.objects, self.subtrees) = (c.decodeBool(forKey: "r"), c.decodeBool(forKey: "a"), objects, subtrees)
        self.inodes = inodes.map(\.uint64Value)
    }
}
