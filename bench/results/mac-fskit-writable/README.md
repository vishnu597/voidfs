# Writable FSKit: risk-first measurements (step 5, item 4)

12 October 2026, macOS 27.0.1, Xcode 27 beta (27A5194q), on this Mac's own test setup:
a memory-store `voidfs-server` on 127.0.0.1:9000, a test daemon in a `mktemp -d` state
directory, the development-signed build of `voidfs.app` from `apps/macos/build`, and the drive
`wtest` mounted with `mount -F -t voidfs -o <options> voidfs://wtest /tmp/vw`. The volume's
probe options: `coherency=nocache|readcache|writethrough|writeback` (the most a write open is
granted), `writedelay=<ms>` (a pause before each write callback stages its bytes, so that a slow
call shows which system call waited for it), `readgrant=…` and `trace` (a log line per callback).
The probes are `apps/macos/scripts/writeprobe.c` and, here, `mmapseq.c`, `swap.c`, `bulk.c` and
`ffs.c`. `raw/` has their outputs and the extension's log of the data and namespace callbacks.

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

`kill -9` of `VoidfsFS` with a file open: the volume is unmounted at once and FSKit does not
start it again. The open descriptor's `close` returns 0.

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
