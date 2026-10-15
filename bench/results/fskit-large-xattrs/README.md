# FSKit and extended attributes over 64 KiB, 15 October 2026

This answers [RFC 0006](https://github.com/vishnu597/voidfs/pull/69)'s open question 1: does FSKit
hand the voidfs module values over 64 KiB whole, and does Finder then keep them? It used a
test-only probe build, which was never merged. The module advertised 16 MiB and the daemon, core
and server accepted that much. It ran on one Mac, macOS 27.0.1 (26A434), Apple M5 Pro. RFC 0006's
status is unchanged.

## Answer

- **Up to 262,143 bytes (2^18 − 1), yes.** Each value arrives in one `setXattr` call of its full
  size, with the source's SHA-256, and the module returns it whole to `getXattr`. Finder copy onto
  the mount, Finder Duplicate within it and Finder copy back out to APFS all keep resource forks
  and `user.*` values of 65,000, 65,537 and 262,143 bytes byte for byte. `cp` does too. This
  includes the 65,537-byte fork the [application matrix](../mac-app-matrix/README.md) loses today.
- **From 262,144 bytes, no, whatever the module advertises.** The kernel refuses the value with
  `EFBIG` ("File too large") before it calls the module. The module advertised 16 MiB
  (`maximumXattrSize`), then 2^26 (`maximumXattrSizeInBits`). FSKit read
  `maximumXattrSizeInBits`, and `pathconf(_PC_XATTR_SIZE_BITS)` still said 18. The kernel also
  refuses any `getxattr` buffer over 262,144 bytes, even for a small value.
- **Finder loses those values and mostly reports success.** Of the six Finder copies of 262,144,
  1 MiB and 16 MiB forks and `user.*` values onto the mount, five reported success with the value
  gone. The sixth, the 262,144-byte fork, failed with Finder error -8003 and left an empty file.
  No `setXattr` of those values reached the module.
- **The kernel never split a value into pieces.** Every `setXattr` the module received was either
  the whole value or a piece a caller wrote on purpose. There are two such cases:
  - Positional `fsetxattr`: the kernel refuses any position above 0 with `EINVAL`, so only the
    first piece is stored, as if it were the whole value.
  - Writes through `..namedfork/rsrc` over 128 KiB: the module gets one `setXattr` of the first
    131,072 bytes, then `close()` fails with `EINVAL`. `copyfile` copies forks over 1 MiB this
    way, so **`cp` of a 16 MiB fork exits 0 and leaves a 131,072-byte fork**.

So a larger limit helps Finder on FSKit only from 64 KiB to 256 KiB − 1. Above that, the Finder
failure stays, and it is FSKit's own limit, so fixing it needs Apple. The daemon socket, the server
and publication carried 16 MiB values whole, so clients that do not go through FSKit would
benefit.

### Points for RFC 0006

These are measurements. What to do about them is RFC 0006's decision.

- On FSKit, the effective value limit is 262,143 bytes, whatever voidfs chooses above it. The
  module's advertised limit only changes what `pathconf` reports, and only downwards. With
  `maximumXattrSizeInBits` = 17, `pathconf` said 17, yet the kernel still passed 131,072- and
  262,143-byte values to the module (`bits17-sweep.jsonl`). The module has to enforce any lower
  limit itself.
- The module cannot tell a `..namedfork/rsrc` prefix from a whole 131,072-byte value; both are one
  `setXattr` with policy 0. Today's 64 KiB module refuses that prefix. With the probe's 16 MiB
  limit, and so with any limit of 131,072 bytes or more, `cp` of a fork over 1 MiB onto the mount
  becomes a silent truncation.
  This run did not measure what `cp` does on the shipping 64 KiB build.
- Values written over the cap by another client cannot be read through FSKit, because no read
  buffer over 262,144 bytes is accepted. This was inferred, not measured
  ([Not measured](#not-measured)).

## Results

A cell is what the destination holds, plus whether the operation reported success. "kept" means
the destination has the source's size and SHA-256. "prefix" means its first N bytes: all ten short
values match the SHA-256 of the source value's first N bytes. "lost" means the destination has no
such attribute. The tables use the readback taken after the mount settled
(`raw/fskit-redump.jsonl`, about a minute after the last case); see
[the stale listing](#a-stale-listing-right-after-finder-copies). Each file also carries 43,008
data bytes and a 26-byte `user.voidfs.sentinel`. Every Finder and `cp` destination kept both,
except the empty file Finder left at -8003.

`com.apple.ResourceFork`, value size in bytes:

| Operation | 65,000 | 65,537 | 262,143 | 262,144 | 1,048,576 | 16,777,216 |
|---|---|---|---|---|---|---|
| Finder copy onto the mount | kept, success | kept, success | kept, success | **lost**, error -8003¹ | **lost**, success | **lost**, success |
| Finder Duplicate within the mount | kept, success | kept, success | kept, success | n/a² | n/a² | n/a² |
| Finder copy back out to APFS | kept, success | kept, success | kept, success | n/a² | n/a² | n/a² |
| `cp` onto the mount | kept, success | kept, success | kept, success | **lost**, error³ | **lost**, error³ | **131,072 B prefix, success** |
| one `fsetxattr`, whole | kept, success | kept, success | kept, success | **lost**, `EFBIG` | **lost**, `EFBIG` | **lost**, `EFBIG` |
| positional `fsetxattr`, 64 KiB pieces | kept, success | 65,536 B prefix, `EINVAL`⁴ | 65,536 B prefix, `EINVAL` | 65,536 B prefix, `EINVAL` | 65,536 B prefix, `EINVAL` | 65,536 B prefix, `EINVAL` |
| positional `fsetxattr`, 1 MiB pieces | kept, success | kept, success | kept, success | **lost**, `EFBIG` | **lost**, `EFBIG` | **lost**, `EFBIG` |
| `write()` to `..namedfork/rsrc`, 1 MiB pieces | kept, success | kept, success | 131,072 B prefix, close `EINVAL` | 131,072 B prefix, close `EINVAL` | 131,072 B prefix, close `EINVAL` | 131,072 B prefix, close `EINVAL` |

`user.voidfs.large`, value size in bytes:

| Operation | 65,000 | 65,537 | 262,143 | 262,144 | 1,048,576 | 16,777,216 |
|---|---|---|---|---|---|---|
| Finder copy onto the mount | kept, success | kept, success | kept, success | **lost**, success | **lost**, success | **lost**, success |
| Finder Duplicate within the mount | kept, success | kept, success | kept, success | n/a² | n/a² | n/a² |
| Finder copy back out to APFS | kept, success | kept, success | kept, success | n/a² | n/a² | n/a² |
| `cp` onto the mount | kept, success | kept, success | kept, success | **lost**, error³ | **lost**, error³ | **lost**, error³ |
| one `fsetxattr`, whole | kept, success | kept, success | kept, success | **lost**, `EFBIG` | **lost**, `EFBIG` | **lost**, `EFBIG` |

1. "The operation can't be completed. (-8003)". It left a 0-byte file on the mount with no
   attributes.
2. Duplicate and copy-out start from the Finder copy on the mount. Over the cap, that copy never
   had the value, so these rows test nothing new. They ran anyway and are in the raw files.
3. `cp: …: could not copy extended attributes to …: File too large`, exit 1, after copying the
   data.
4. The first piece, at position 0, succeeds. The second, at position 65,536, fails with
   `EINVAL`, and the module never sees it.

`summarize.py` regenerates both tables from the raw files (see [Commands](#commands)).

### What the module received

`raw/fskit-trace-per-op.jsonl` joins the module trace to each operation, by inode (a file's
`st_ino` on this mount is the module's inode plus 2). For every case at or under 262,143 bytes
that kept its value, the module received exactly one `setXattr` of that value, with the full size
and the source's SHA-256 (`opSetxattrWhole: true`). Over the cap, Finder, `cp`, a whole
`fsetxattr` and 1 MiB positional pieces sent the module no `setXattr` of that attribute at all,
except `cp` of the 16 MiB fork, which went through `..namedfork/rsrc`.
The only short `setXattr` calls came from 64 KiB positional pieces (65,536 bytes) and from
`..namedfork/rsrc` writes (131,072 bytes).

Excerpts, from `raw/trace-excerpts.txt` (timestamps trimmed, hashes shortened, some lines left out):

```text
== fork-65537 Finder copy onto the mount: exit 0
create file 498 in 200 mode 644 asked uid flags birth
write 498 @0+43008 staged in 0.001830542 seconds, size 43008
setxattr 498 com.apple.ResourceFork policy 0 65537 bytes sha256 4ea27a5a82e2a83a…
setxattr 498 com.apple.ResourceFork stored in 0.018106625 seconds
getxattr 498 com.apple.ResourceFork 65537 bytes sha256 4ea27a5a82e2a83a… in 0.023125333 seconds

== fork-262143 Finder copy onto the mount: exit 0
setxattr 821 com.apple.ResourceFork policy 0 262143 bytes sha256 316d62c7539bad57…
getxattr 821 com.apple.ResourceFork 262143 bytes sha256 316d62c7539bad57… in 0.022522959 seconds

== fork-1048576 Finder copy onto the mount: exit 0 (the fork never reaches the module)
create file 1609 in 200 mode 644 asked uid flags birth
write 1609 @0+43008 staged in 0.003486375 seconds, size 43008
setxattr 1609 com.apple.FinderInfo policy 3 -1 bytes sha256 -
setxattr 1609 com.apple.provenance policy 0 11 bytes sha256 452c7ac82d540a28…
setxattr 1609 user.voidfs.sentinel policy 0 26 bytes sha256 7a3d085190a9c53a…
close 1609 writable in 0.018241042 seconds
getxattr 1609 com.apple.ResourceFork failed errno 93

== fork-16777216 cp onto the mount: exit 0
setxattr 2174 com.apple.ResourceFork policy 0 131072 bytes sha256 8b5f418cbba5f322…
close 2174 writable in 0.041981 seconds

== FSKit reading the probe's size property (builds 3 and 4)
probe: FSKit read maximumXattrSizeInBits 26
probe: FSKit read maximumXattrSizeInBits 17
```

Policy 0 is FSKit's `alwaysSet` and policy 3 is `delete`. Errno 93 is `ENOATTR`: Finder checking
for a fork the file does not have. `8b5f418c…` is the SHA-256 of the first 131,072 bytes of the
16 MiB source fork.

### The kernel's cap

All of these use a whole `fsetxattr` at position 0 on the FSKit mount (`xattr-probe set … whole`):

| Probe build | Module advertises | `pathconf` bits | Largest value accepted | Smallest refused (`EFBIG`) | Raw |
|---|---|---|---|---|---|
| 1 | `maximumXattrSize` 16 MiB | 18 | 262,143 | 262,144 | `sweep.jsonl`, `bisect.jsonl` |
| 2 | also `maximumXattrSizeInBits` 26 | 18 | 262,143 | 262,144 | `bits-sweep.jsonl` (also 1 MiB and 16 MiB) |
| 3 | build 2, logging FSKit's reads | 18 | 262,143 | 262,144 | `bits-logged-sweep.jsonl`, trace (logged read of 26) |
| 4 | `maximumXattrSizeInBits` 17 | 17 | 262,143 | 262,144 | `bits17-sweep.jsonl` |

Build 1's bisection ran from 131,073 to 262,144 in 17 steps. Builds 2 to 4 did not bisect; they
set only the sizes in their raw files. Build 3's two results are copied from the session's output
into `bits-logged-sweep.jsonl`.

Reads: with a 65,000-byte fork on the mount, `getxattr` with a buffer of up to 262,144 bytes
returns 65,000. A buffer of 262,145 bytes or more fails with `EFBIG`. APFS accepts a 1 MiB buffer
(`read-buffer.jsonl`).

### The daemon, server and publication path

With the probe's Rust build, nothing below the module refused or split a 16 MiB value
(`raw/setup/socket-xattr.jsonl`). The test made all its calls from one process, because daemon
sessions are bound to the caller's pid.

| Daemon socket call | Bytes | Result | Time |
|---|---|---|---|
| `setxattr` / `getxattr` `user.big` | 16,777,216 | 200 / 200, whole SHA-256 | 2.9 s / 1.3 s |
| `setxattr` / `getxattr` `com.apple.ResourceFork` | 16,777,216 | 200 / 200, whole SHA-256 | 7.0 s / 3.5 s |
| `setxattr` `user.over` | 16,777,217 | 413 `TooLarge`, errno 7 (`E2BIG`) | 0.03 s |

After publication, the server's attributes for that object held both 16 MiB values with the same
SHA-256 (`raw/setup/socket-server-attrs.json`, values replaced by their size and hash). The mount
core keeps a memo of each value as a JSON array of numbers in SQLite, which probably accounts for
most of those times; they were not profiled. The server's copies of mount-written files match what
the mount held (`raw/server-attrs.jsonl`): 262,143 bytes for the Finder copy and its Duplicate,
131,072 bytes for the `cp` of the 16 MiB fork, and no fork for the Finder copy of the 1 MiB
fork.

### APFS controls

The same script ran on APFS at 65,000, 65,537, 1 MiB and 16 MiB, in two setups:

- Within the boot volume (`raw/apfs.jsonl`). Finder's same-volume copies there are clones.
- From the boot volume onto a separate APFS sparse disk image (`raw/apfs-other-volume.jsonl`).

Both kept all 52 of 52 values byte for byte, with every operation reporting success, including
64 KiB positional pieces and `..namedfork/rsrc` writes of 16 MiB. The APFS runs did not include
262,143 or 262,144 bytes. The `dataMatches: false` rows in every surface's raw file are the
positional and namedfork cases: their destination files were created with different data on
purpose.

### A stale listing right after Finder copies

Right after a Finder copy or Duplicate onto the mount, 21 of the 22 destinations that hold
attributes listed none at all (`raw/fskit.jsonl`). About a minute later, every one listed them
(`raw/fskit-redump.jsonl`). `cp` destinations listed theirs at once. The module's `listxattr` and
`getxattr` return early when the item's cached attributes say `hasXattrs` is false
(`apps/macos/FSModule/VoidfsVolume.swift:1074` and `:1091`). This looks like a stale flag after
Finder's create, write and `setxattr` sequence. It is a module bug candidate, separate from this
question, and the tables use the settled readback.

## Not measured

- **Reading over-cap values written by another client.** Inferred unreadable through FSKit from
  the read-buffer cap; not tried.
- `cp` back out of the mount. The matrix's
  [resource-fork failure](../mac-app-matrix/README.md#the-resource-fork-failure) covers
  `copyfile`'s positional reads from the mount. Finder's copy-out here read each fork whole.
- `cp` of a fork over 1 MiB on the shipping 64 KiB build. There, the module would refuse the
  131,072-byte namedfork prefix.
- Why Finder reports -8003 for the 262,144-byte fork but success for the 262,144-byte `user.*`
  value and for every larger value.
- Repeats. Each case ran once; the cap is a deterministic refusal.
- Other macOS versions.

## How it was run

### Probe build

The branch is [`probe/fskit-large-xattrs`](https://github.com/vishnu597/voidfs/tree/probe/fskit-large-xattrs),
pushed and never merged, based on `main` at `bf34f87`. Its diffs are
[`probe-build-1.diff`](probe-build-1.diff) (`bf34f87..3e93746`) and
[`probe-builds-2-4.diff`](probe-builds-2-4.diff) (`3e93746..762b3c0`).

- Cargo feature `large-xattr-probe`, on `voidfs-core` and `voidfs-daemon`:
  - the core's per-object total (`MAX_XATTR_BYTES`) becomes 64 MiB;
  - the daemon socket's value bound (`fs_api::MAX_XATTR`) becomes 16 MiB;
  - the server's attributes POST takes `MAX_EXTENSION_BODY` instead of 4 MiB.
  Values stay inline; there is no protocol change.
- Swift compile condition `VOIDFS_LARGE_XATTR_PROBE`:
  - `Bridge.maxXattr` becomes 16 MiB, which sets the module's `maximumXattrSize` and the agent's
    checks;
  - with `-o trace`, the module also logs `getxattr` and `listxattr`, and each value's size and
    SHA-256.
- `VOIDFS_LARGE_XATTR_PROBE_BITS` adds `maximumXattrSizeInBits`: 26 in builds 2 and 3, 17 in
  build 4. Build 3 logs FSKit's reads of it.

| Build | Commit | Used for |
|---|---|---|
| Rust (debug) | tree of `3e93746`, built before it was committed, so `void daemon status` says `bf34f87` | everything |
| App 1 | tree of `3e93746` | all FSKit cases, sweep, bisect, read buffer, trace |
| App 2 | `084abc2` | `pathconf-fskit-bits.json`, `bits-sweep.jsonl` |
| App 3 | `3380d3d` | `pathconf-fskit-bits-logged.json`, the logged read of 26 |
| App 4 | `762b3c0` | `pathconf-fskit-bits17.json`, `bits17-sweep.jsonl`, the logged read of 17 |

Tools: Xcode 27.0 (27A5194q), rustc and cargo 1.98.1 (`raw/setup/versions.txt`). Each app build's
`swiftc` lines carried the conditions (`raw/setup/xcodebuild-summary.txt`). The apps were signed
with an Apple Development certificate.

### Rig

This follows the matrix's
[isolated rig](../mac-app-matrix/README.md#preparing-and-restoring-the-isolated-rig):

- a local `voidfs-server` on 127.0.0.1:9000, with a filesystem store and a test-only key;
- an isolated daemon state directory from `mktemp`, and drive `lxattr-1792077585`;
- the agent pointed at that state directory with `defaults write dev.voidfs.app
  VoidfsStateDirectory`;
- the probe app registered in place of `/Applications/voidfs.app`.

The user approved replacing the registrations before it started. Run scratch was
`/private/tmp/vfx.1ioGhf`; it has been deleted.

### Commands

Probe binaries, from the probe branch, with a fresh `CARGO_TARGET_DIR`:

```bash
cargo build --locked -p voidfs-cli -p voidfs-server --features voidfs-core/large-xattr-probe,voidfs-daemon/large-xattr-probe
```

```bash
cargo build --locked -p voidfs-sdk --example mount_metadata --features voidfs-core/large-xattr-probe
```

```bash
xcodebuild -project apps/macos/voidfs.xcodeproj -scheme voidfs -configuration Release -derivedDataPath apps/macos/build -allowProvisioningUpdates -allowProvisioningDeviceRegistration 'SWIFT_ACTIVE_COMPILATION_CONDITIONS=$(inherited) VOIDFS_LARGE_XATTR_PROBE' build
```

Builds 2 to 4 added `VOIDFS_LARGE_XATTR_PROBE_BITS` to the conditions. The test tool:

```bash
cc -O2 -Wall -Wno-deprecated-declarations -o "$PROBE" bench/results/fskit-large-xattrs/xattr-probe.c
```

Server, drive and daemon, with `env.sh` exporting `VOIDFS_STATE_DIR`, `VOIDFS_ENDPOINT`,
`VOIDFS_LISTEN`, `VOIDFS_STORE=fs:$RUN/server` and a test-only key pair:

```bash
voidfs-server --new-pool-feature multi-object-versions
```

```bash
void drive create "$MATRIX_DRIVE" && void daemon start
```

Registration (`raw/setup/register.txt` has the full record):

```bash
defaults write dev.voidfs.app VoidfsStateDirectory "$RUN/state"
/Applications/voidfs.app/Contents/MacOS/voidfs agent-unregister
"$APP/Contents/MacOS/voidfs" agent-register
lsregister -u /Applications/voidfs.app
pluginkit -a "$APP/Contents/Extensions/VoidfsFS.appex"
launchctl kickstart -k gui/$(id -u)/dev.voidfs.agent
```

The trace, then the mount:

```bash
log stream --style ndjson --level info --predicate 'subsystem == "dev.voidfs"' > "$RUN/setup/fskit-trace.ndjson"
```

```bash
mount -F -t voidfs -o trace "voidfs://$MATRIX_DRIVE" "$MATRIX_MOUNT"
```

The cases. `apfs` and `apfs-other-volume` used the default sizes, and the control volume came from
`hdiutil create -size 1g -fs APFS -type SPARSE` and `hdiutil attach -nobrowse`:

```bash
/usr/bin/python3 run-cases.py --surface fskit --src "$RUN/fskit/src" --dest "$MATRIX_MOUNT/cases" --export "$RUN/fskit/export" --probe "$PROBE" --script finder.applescript --sizes 65000,65537,262143,262144,1048576,16777216 --out "$RUN/results/fskit.jsonl"
```

```bash
/usr/bin/python3 redump.py "$PROBE" "$RUN/results/fskit.jsonl" > "$RUN/results/fskit-redump.jsonl"
```

```bash
/usr/bin/python3 trace-xattrs.py "$RUN/setup/fskit-trace.ndjson" "$RUN/results/fskit.jsonl" "$MATRIX_MOUNT" > "$RUN/results/fskit-trace-per-op.jsonl"
```

```bash
/usr/bin/python3 summarize.py raw/fskit.jsonl raw/fskit-redump.jsonl
```

The cap, by bisection. Each step is `xattr-probe set "$F" user.voidfs.bisect "$mid" 9 whole`
between 131,073 and 262,144 on a file in `$MATRIX_MOUNT/sweep`. The read-buffer check is inline
`ctypes` in the session; its output is `raw/read-buffer.jsonl`. The daemon socket check:

```bash
/usr/bin/python3 socket-xattr.py "$VOIDFS_STATE_DIR/daemon.sock" "$MATRIX_DRIVE" "$PROBE" "$RUN/setup/socket-xattr.jsonl"
```

Server-side attributes came from `mount_metadata "$MATRIX_DRIVE" attrs <key>`.

## Registrations and defaults, restored

The record is `raw/setup/restore.txt`:

1. Stopped the daemon and the server, and detached the control image.
2. Unregistered the probe agent and removed the probe app from LaunchServices.
3. Re-registered `/Applications/voidfs.app` with `lsregister -f -R`, then `pluginkit -a` for its
   extension and `agent-register`.
4. Deleted the `VoidfsStateDirectory` default.

The first `voidfs bridge-check` afterwards timed out (`bridge-check-after.txt` is empty), and
`launchctl print` showed the `/Applications` agent at `last exit code = 78: EX_CONFIG`, job state
"spawn failed". One more unregister and register from `/Applications` (`reregister-1.txt`)
cleared it: the agent ran, and `bridge-check` printed `refused (NSCocoaErrorDomain 4097): ok`
(`bridge-check-after-2.txt`), the Developer ID build's known launchd refusal.

`raw/setup/after-restore.txt` matches the `*-before.txt` baseline:

- `defaults read dev.voidfs.app` is `{}`;
- `dev.voidfs.app.fskit` is enabled at `/Applications/voidfs.app/Contents/Extensions/VoidfsFS.appex`;
- `agent: enabled`;
- no voidfs mounts and no test processes;
- no shared state directory.

One difference remains: `pluginkit -m -v` now shows the extension's version as `(null)`, where it
showed `0.1` before. The path, the enabled state and `voidfs status` match. The probe's own
registration also showed `(null)` right after `pluginkit -a`, and `0.1` after a rebuild
(`register.txt`), so this looks cosmetic. The probe worktree, its build products and the run
scratch are deleted; the probe branch stays on origin.

`raw/setup/launchctl-before.txt` omits the agent's inherited environment, which is the login
session's and not about voidfs.

## Files

| File | What |
|---|---|
| `xattr-probe.c` | Makes files with a value, sets a value whole, in positional pieces or through `..namedfork/rsrc`, and dumps a file's attributes with sizes and SHA-256 |
| `run-cases.py`, `finder.applescript` | The cases; Finder copies go through `osascript`, as the matrix's `finder-copy` does |
| `redump.py`, `trace-xattrs.py`, `summarize.py` | Settled readback, trace join, tables |
| `socket-xattr.py` | Daemon socket check |
| `probe-build-1.diff`, `probe-builds-2-4.diff` | The probe's changes |
| `raw/fskit.jsonl`, `raw/fskit-redump.jsonl`, `raw/fskit-trace-per-op.jsonl` | FSKit cases, settled readback, per-operation trace |
| `raw/apfs.jsonl`, `raw/apfs-other-volume.jsonl` | APFS controls |
| `raw/fskit-trace.ndjson.gz`, `raw/trace-excerpts.txt` | The whole module trace, and the excerpts quoted above |
| `raw/sweep.jsonl`, `raw/bisect.jsonl`, `raw/bits-sweep.jsonl`, `raw/bits-logged-sweep.jsonl`, `raw/bits17-sweep.jsonl`, `raw/read-buffer.jsonl` | The cap |
| `raw/server-attrs.jsonl`, `raw/setup/socket-*.json*` | Daemon and server path |
| `raw/setup/` | Baseline, registration, mount, `pathconf`, versions, restore records |
