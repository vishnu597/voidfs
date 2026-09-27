// SPDX-License-Identifier: Apache-2.0

import CryptoKit
import FSKit
import Foundation

enum Kind: String {
    case file, folder, symlink
}

/// What the mount knows about an object, from its parent's listing.
struct ItemInfo {
    var key: String         // full key; folders end in "/", the root is ""
    var kind: Kind
    var size: UInt64
    var mtime: timespec
    var mode: UInt32
    var etag: String?
    var versionId: String?
    var hasXattrs: Bool
    var target: String?

    init(key: String, entry e: ListEntry) {
        self.key = key
        self.kind = Kind(rawValue: e.kind) ?? .file
        self.size = e.size ?? 0
        self.mtime = parseTimestamp(e.mtime)
        self.mode = parseMode(e.mode)
        self.etag = e.etag
        self.versionId = e.versionId
        self.hasXattrs = e.hasXattrs ?? false
        self.target = e.target
    }

    init(root mtime: timespec) {
        key = ""
        kind = .folder
        size = 0
        self.mtime = mtime
        mode = 0o755
        hasXattrs = false
    }

    /// The name inside the parent folder, without the folder's trailing "/".
    var name: String {
        var k = Substring(key)
        if k.hasSuffix("/") { k = k.dropLast() }
        return String(k[(k.lastIndex(of: "/").map { k.index(after: $0) } ?? k.startIndex)...])
    }
}

/// One object of the drive. FSKit keeps one per kernel vnode; the volume keeps one per object id
/// so repeated lookups return the same instance.
final class VoidfsItem: FSItem {
    let objectId: String
    let id: FSItem.Identifier
    /// Guarded by the volume's lock.
    var info: ItemInfo
    var parentID: FSItem.Identifier

    init(objectId: String, id: FSItem.Identifier, info: ItemInfo, parentID: FSItem.Identifier) {
        self.objectId = objectId
        self.id = id
        self.info = info
        self.parentID = parentID
    }

    /// A stable 63-bit file id derived from the object id, so inode numbers survive remounts
    /// (Finder aliases and bookmarks record them). 0–2 are reserved by FSKit.
    static func fileID(for objectId: String) -> FSItem.Identifier {
        let digest = SHA256.hash(data: Data(objectId.utf8))
        var v: UInt64 = 0
        for b in digest.prefix(8) { v = v << 8 | UInt64(b) }
        v &= 0x7fff_ffff_ffff_ffff
        if v <= FSItem.Identifier.rootDirectory.rawValue { v += 3 }
        return FSItem.Identifier(rawValue: v)!
    }
}
