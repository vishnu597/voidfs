# Writable FSKit: risk-first measurements (step 5, item 4)

12 October 2026, macOS 27.0.1, Xcode 27 beta (27A5194q), on this Mac's own test setup:
a memory-store `voidfs-server` on 127.0.0.1:9000, a test daemon in a `mktemp -d` state
directory, the development-signed build of `voidfs.app` from `apps/macos/build`, and the drive
`wtest` mounted with `mount -F -t voidfs -o <options> voidfs://wtest /tmp/vw`. Q1–Q5 used the
volume's probe options (commit 2edbdc7): `coherency=nocache|readcache|writethrough|writeback`
(the most a write open is granted), `writedelay=<ms>` (a pause before each write callback stages
its bytes, so that a slow call shows which system call waited for it), `readgrant=…` and `trace`
(a log line per callback). The answers chose writeThrough, and 8a11a2f removed the options but
`trace`. The probes are `apps/macos/scripts/writeprobe.c` and `semantics.c` and, here,
`mmapseq.c`, `swap.c`, `bulk.c`, `ffs.c`, `atomic.c` and `locks.c`. `raw/` has their outputs and
the extension's log of the data and namespace callbacks. [Final build](#the-final-build) has the
checks run on the finished item. Daemon version lines in `raw/` name the commit the binary was built
from before the branch was rebased (the daemon's code is d80e71a's).

## Q1. When do writes reach the module, and does fsync?

| Granted to the write open | `write(2)` | `fsync`, `F_FULLFSYNC` | last `close(2)` | `sync(2)` |
| --- | --- | --- | --- | --- |
| noCache, readCache, writeThrough | waits for the write callback (520–535 ms with a 500 ms delay) | 0.0 ms: nothing reaches the module | waits for the module's close (its flush) | 84–107 ms, no callback |
| writeBack | 0.0 ms: the page stays in the kernel | pushes the dirty pages through ordinary write callbacks, and waits | pushes, then the module's close | starts the push; the close waits for the rest |
| (data cache handler inhibited) | 0.0 ms | 1–2 ms | 2 ms | |

- **No callback says fsync.** `synchronize(flags:)` never came: not on `fsync`, `F_FULLFSYNC`,
  `F_BARRIERFSYNC`, `sync(2)` or unmount, under any grant (the extension exports the selector).
  Under writeBack a pushed page arrives as an ordinary write, the same as a page-out.
- The kernel sends a write open as `modes 3 cache 2` (read-write, `readWriteWithCache`).
- Sequential 4 KiB writes arrive as the dirty range from the page's start: `@0+4096`, `@0+8192`,
  `@0+12288`… (16 KiB pages), so small appends resend up to a page.
- Costs (`raw/fullwrite-cost.txt`), writeThrough with each write staged at `NORMAL`: 4 KiB writes
  1.9 MiB/s (p50 0.87 ms), 64 KiB 56 MiB/s, 1 MiB 159 MiB/s. A full flush after every write
  callback through today's core fsync took about 1 s a callback (it also queues publication); this
  Mac's floor for a 4 KiB append and `F_FULLFSYNC` on APFS is 3.2 ms.

## Q2. Killing the extension

`kill -9` of `VoidfsFS` with a file open: FSKit does not start the extension again. Here the
volume was unmounted at once and the open descriptor's `close` returned 0; on the final build,
mounted through the daemon, a kill with a file open left a dead mount instead (every call EIO,
`close` too) and one with nothing open unmounted it (see [Final build](#the-final-build)).

- writeThrough: the 64 KiB that `write(2)` acknowledged were staged; the daemon's session release
  flushed and published them (`void show` printed all 65,536 bytes).
- writeBack: the 64 KiB `write(2)` acknowledged were only in the kernel and were lost (the cloud
  object stayed empty), and `close` still returned 0. **writeBack can't meet "write returns after
  staging that survives an extension crash".**
- Pages stored through a shared mapping and not yet pushed are lost with the extension (no msync
  had acknowledged them).

## Q3. mmap

- With a descriptor open, `msync` pushes the dirty pages as write callbacks and the last close
  pushes the rest.
- After the last close, `msync` still pushes them: the write arrives with no open, and the volume
  opens a writable daemon handle for it.
- **FSKit drops a mapping's later stores** once its pages were pushed with no descriptor open
  (`msync` after `close`): the next open of the file discards the dirty page without pushing it,
  and nothing finds it again (`raw/q3-mmapseq.txt`; `mc1S2ur`: map, close, store 1, msync, store 2,
  munmap, read: store 2 missing, also after a remount). `sync(2)`, the periodic sync (about 22 s
  later) and unmount push it if they come first. The same under every grant and with the data
  cache handler inhibited, so it is the kernel side of FSKit.
  **Mitigation, verified:** the volume marks a file that received pages with no open, and every
  later open first asks the kernel to push (`setCacheState(…, action: .push)`, about 1 ms); the
  store then reads back and survives a remount (`raw/q3-fixed.txt`).
- Not checked: a vnode reclaimed under memory pressure between `munmap` and the next sync.

## Q4. The volume's own writes and publications

Every local change reaches the volume's own session as an event, and each publication comes back
through the feed. Before the fix the volume took its own publications for other computers'
changes: it retired and revoked the file it was writing, and a page-out after close was refused
with `ESTALE` because the version had moved on (`msync` returned 0 regardless: the loss was
silent). The daemon now reports feed changes the local view already shows (versions this Mac
published, and names with unpublished local edits) to no one, and each event says whether it
is local. Afterwards every echo arrived with no inodes and nothing was revoked.

## Q5. `semantics.c` on the voidfs module

`raw/q5-semantics.txt`. **`renamex_np(a, b, RENAME_SWAP)` returns 0 and overwrites `b`** (after:
`a` missing, `b` = A), although the volume advertises `RENAME_SWAP=no`. The module receives a
plain rename of `a` over `b`, the same call as an atomic save's, with no flag (FSKit's rename
has none). `b`'s published bytes come back with a folder restore to before the swap; unpublished
ones would be gone. `RENAME_EXCL` onto an existing name returns `EEXIST` (the kernel checks it),
`exchangedata` and `clonefile` `ENOTSUP`, xattrs set with no `._` files, atomic save, mmap and
open-unlink work.

## Q6. The same probes on Apple's FAT module

`raw/fat-check.txt`: a scratch 64 MiB FAT image (`hdiutil create -fs MS-DOS`, attached without
sudo), which macOS 27 serves with its FSKit `msdos` module. **`RENAME_SWAP` returns 0 and
overwrites there too** (`a` missing, `b` = A), so the kernel side of FSKit does it, not the voidfs
module: FSKit's `renameItem` has no flags, and the kernel doesn't refuse the swap for a volume
that doesn't advertise it. `RENAME_EXCL` onto an existing name returns `EEXIST` on both. The
mmap sequences that lost a store on voidfs before the push at open (Q3) keep it on FAT, so Q3's
"kernel side" is at least not all of it: Apple's module may handle a page-out with no open
differently. Q3's push at open fixed it on voidfs either way. FAT
can't hold a resource fork (`EINVAL`) and writes `._` files; voidfs writes none. `semantics.c`'s
`F_SETLK … Resource temporarily unavailable` on both is the probe's own `flock` on the same file
a line earlier (a process's flock and fcntl locks conflict on macOS); `locks.c` takes each lock
on its own file, and they behave as on APFS (below).

## The final build

12 October, the volume at 8a11a2f and the daemon at d80e71a, mounted with `void mount wtest
/tmp/vw` through the daemon (`raw/mounts.txt`), on a memory server whose pool has
`multi-object-versions`.

**Mounts** (`raw/mounts.txt`): `void mount` mounts writable; `void mount --read-only` mounts with
`mount -r`, and creating, appending, `mkdir`, `rm` and `xattr -w` all answer `EROFS` there. FSKit
mounts a resource once: a read-only mount of a drive that is also mounted writable is refused
(`Resource busy`). `df` shows this Mac's disk (`raw/trash-spotlight-size.txt`).

**Names** (`raw/names.txt`, `raw/misses.txt`): `.DS_Store` stays on this Mac; `DS_Store`,
`.DS_Store.txt`, `.ds_store`, `._notes`, `.localized` and `.hidden` are ordinary files and
publish. A name the kernel was told is missing shows once it is created, on the mount or through
the CLI (`void upload` into the mounted folder). No `.Trashes`, `.fseventsd` or
`.Spotlight-V100` appear; `mdutil -s` answers "unknown indexing state", so Spotlight doesn't
index the volume. Finder's Move to Trash deletes at once (there is no `.Trashes`); `void restore`
brings the file back, as `void show` says (`raw/trash-spotlight-size.txt`).

**Finder** (`raw/finder.txt`): Finder (driven by `osascript`) copied a 5 MiB file, a folder
tree, and a file with a 5-byte resource fork, `com.apple.quarantine`, `com.apple.provenance`
and an empty `user.note`; renamed and duplicated files. Contents match by SHA-256 on the mount
and the server; the xattrs round-trip byte for byte, and the server holds them all. No `._`
files. Each new file is two versions, `put` and `attrs`: the system sets
`com.apple.provenance` on every file a process writes, and a put can't carry xattrs (an item 9
candidate).

**Resource forks over 64 KiB** (`raw/resource-fork.txt`): protocol §4.8 allows 64 KiB of xattrs
a file. `cp` of a file with a 200 KiB fork copies the data, prints "could not copy extended
attributes … Argument list too long" (`E2BIG`) and exits 1. **Finder's duplicate succeeds and
drops the fork silently.** A 40,000-byte fork copies. No `._` file either way. Larger values
need a protocol change (§4.8 calls them a future feature).

**Atomic saves** (`raw/atomic-save.txt`, `raw/textedit.txt`): `atomic.c` renames a new file over
`doc.txt` while a descriptor holds the old one: the old descriptor reads the old bytes, a new
open the new ones, and the drive keeps both objects (protocol §4.7). TextEdit's safe save (a
`notes.txt.sb-…` folder, the new file in it, the original renamed aside, the new one renamed in,
the rest deleted) is 6 versions a save (versions 10–21 for two saves), and each save makes a new
object: `void history wtest ev/textedit/notes.txt` lists the newest object's versions only, and
the earlier drafts are recently deleted. A folder restore to before the saves brings the first
draft back, and the mount reads it. On a pool without `multi-object-versions`
(`raw/textedit-legacy-pool.txt`), the same restore lists the file with the folder's restore
version, which the file's history doesn't have (the step 4 finding RFC 0004 fixed), so the mount's
read by version answers `ESTALE` until something else changes the file.

**mmap** (`raw/mmap-final.txt`): a shared writable mapping's stores, with the descriptor open or
closed before the stores, reach the server: the same SHA-256 on the mount and the server, and
the `mmapseq` sequences that lost a store (Q3) keep both, on the server too.

**Killing the extension** (`raw/kill-extension.txt`): writes acknowledged within a second of
`kill -9`, a 1 MiB copy, an appended file, and 64 KiB still open, are all on the server, each
file one `put` and one `attrs`. With a file open at the kill, FSKit keeps a dead mount ("connect
called on dead mount entry" in fskitd's log): every call answers `EIO`, the open file's `close`
too, and it stays so. `void unmount` clears it. d80e71a makes a daemon starting mount a dead
mount anew, so `void daemon restart` brings the volume back (seen). With nothing open the volume
is unmounted, and `void daemon restart` mounts it again.

**Killing the daemon** (`raw/kill-daemon.txt`): while the daemon is down the mount answers `EIO`;
`void daemon start` adopts the mount and the same mount answers again, with every acknowledged
byte. With publication paused (`void uploads pause --all`), `void daemon info` said "6 changes
unpublished, 3.0 MiB to send, all of it on disk" before `kill -9`; after a start the mount
showed the local bytes, nothing was on the server, and `void uploads resume --all` published one
version for each change: a file written and appended 1.5 s later is one put, and no change
published twice. A file still open when its create hold (1 s) expires publishes empty first, then
its bytes at close.

**Locks and SQLite** (`raw/locks.txt`, `raw/sqlite.txt`): fcntl, flock and `O_EXLOCK` locks are
held against another process and released, as on APFS. They are this Mac's only: nothing
reaches the server. `sqlite3` creates, writes and checks rollback-journal and WAL databases; a
second writer is refused while the first holds a write transaction, and the server's copy
passes `integrity_check`. The `-journal`, `-wal` and `-shm` files are ordinary files and publish.

**Conflicts** (`raw/conflict.txt`): an edit on the mount while another computer changed the same
file: the save is refused (412), the mount keeps this Mac's bytes, the server keeps the other.
Before 6522123 `void uploads` said `failed: 412 PreconditionFailed …` for both entries of the
save and `void status` "2 failed"; now both say `conflict: the drive changed (now version 184.0);
this Mac keeps its copy`, `void status` "1 in conflict", and daemon.log has one line for it.

**`semantics.c`** (`raw/semantics-final.txt`): as in Q5, with `SYMBOLICLINKS=no` and `symlink`
`ENOTSUP`. `RENAME_SWAP` still overwrites (Q5, Q6).
