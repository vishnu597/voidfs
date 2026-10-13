// SPDX-License-Identifier: Apache-2.0

import FSKit
import Foundation

/// One incarnation of a daemon inode, as the kernel holds it. The volume answers lookups of an
/// inode with its current incarnation, so repeated lookups return the same instance; when another
/// machine changes the inode, the volume revokes it in the kernel and retires it, and the next
/// lookup makes a new one. A descriptor opened before the change keeps the retired instance, and
/// with it the daemon handle opened then: its reads, and the attributes they carry, come from the
/// version that was current at open (snapshot-at-open), never an old length with new bytes.
final class VoidfsItem: FSItem, @unchecked Sendable {
    let ino: UInt64
    let id: FSItem.Identifier
    /// Guarded by the volume's lock.
    var parent: UInt64
    var kind: BridgeAttr.Kind
    /// The daemon handle opened when the kernel first opened this incarnation, until the kernel
    /// closes it: read-only, or writable once the kernel opened it for writing.
    var handle: OpenFile?
    /// The open in flight, which other opens and reads of this incarnation wait for.
    var opening: Task<OpenFile, Error>?
    /// A later incarnation answers lookups now.
    var retired = false
    /// A change made on this Mac touched the inode: the handle's attributes are read again.
    var stale = false
    /// Pages came with no open: a shared mapping outlived its descriptors. Each later open
    /// pushes the kernel's cache first (see `VoidfsVolume.open`).
    var mapped = false

    init(ino: UInt64, id: FSItem.Identifier, parent: UInt64, kind: BridgeAttr.Kind) {
        (self.ino, self.id, self.parent, self.kind) = (ino, id, parent, kind)
    }

    /// The daemon's inode numbers are persistent (step 5's mount core), so they are the file
    /// IDs: the root is FSKit's root, and every other inode is shifted past FSKit's reserved 0–2.
    static func identifier(_ ino: UInt64, root: UInt64) -> FSItem.Identifier {
        ino == root ? .rootDirectory : FSItem.Identifier(rawValue: ino + 2) ?? .invalid
    }
}

/// A daemon handle: the attributes bound with it at open (then as this Mac's changes leave
/// them), and the session it belongs to (a handle means nothing to a later session).
struct OpenFile {
    let fh: UInt64
    var attr: BridgeAttr
    let session: BridgeSession
    let write: Bool
}
