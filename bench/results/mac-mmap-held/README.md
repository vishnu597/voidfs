# Held mmap stores and SQLite SHM, 14 October 2026

Step 5, handoff task 2. The 43-byte SQLite SHM divergence is reproduced on macOS
27.0.1 (26A434), Xcode 27.0 (27A5194q), Apple M5 Pro, from `main` at `c87b52d`.
It is a kernel-page delivery gap, before the Rust staging/publication path.

## Three independent views

The unchanged application matrix row has one failure:
[SQLite before](raw/sqlite-before.jsonl.gz), [console](raw/sqlite-before-console.jsonl.gz).
The `.db` contents, integrity, recovery and peer snapshot checks pass. After drain,
the 32,768-byte `wal.db-shm` still differs in 43 bytes.

[The independent observation](raw/sqlite-before-three-views.json) reads a second,
read-only daemon filesystem session first, then the server with `void show`, then
the mounted file. It creates no new FSKit open until the final kernel observation:

| View | SHA-256 |
| --- | --- |
| Kernel | `6b265bfde8c7135c2ac684fcb626563b8204ea1b110e1ccd7adbe54064598f82` |
| Daemon's logical staged view | `4ea39c44c3762398885c577116bd0523a541dcea7e94ba03e2494d108c813f37` |
| Server | `4ea39c44c3762398885c577116bd0523a541dcea7e94ba03e2494d108c813f37` |

The daemon and server agree. The changed bytes have not reached the module, so
the daemon's quiet-period publisher has no change to publish. Queue drain and
`daemon info`'s `memoryOnlyBytes` cannot detect dirty kernel pages.

The existing `mapped` flag is set only after a write arrives without a writable
daemon handle. A mapping written while its descriptor remains open never sets
that flag, so the existing push before a later open misses it. The original
[matrix trace](../mac-app-matrix/raw/fskit-trace.txt.gz) also shows inode 120285's
last SQLite close at 10:33:35.475 and its next page-write callback at 11:03:27.523,
with intervening read opens but no push. The older evidence remains unchanged.

## Minimal reproduction and default sweep

[`mmapheld.c`](../mac-fskit-writable/mmapheld.c) exclusively creates a 32 KiB file,
holds its descriptor and shared writable mapping, and stores distinctive bytes in
the first and last host VM pages. On this Mac the page size is 16 KiB. Its line
protocol allows stores, same-descriptor reads, close and unmap in either order.
Only explicit `msync`/`sync` commands synchronize; startup and cleanup do not.

[`mmap-held.py`](../../../apps/macos/scripts/mmap-held.py) samples the probe's
mapping/original descriptor, a read-only daemon session, and the server without
an observer opening the mounted file. Samples target 0.1, 1, 3 and 5 seconds.
Final observations log actual elapsed time; more than 250 ms of lateness is
inconclusive (exit 2), rather than a byte pass. A byte mismatch exits 1.

| Case | Original module | 500 ms periodic push |
| --- | --- | --- |
| Descriptor and mapping held, generation 1 | Fail | Pass |
| Later generation, both held | Fail | Pass |
| Close descriptor, keep mapping | Pass | Pass |
| Then release mapping | Pass | Pass |
| Unmap first, descriptor held | Fail | Pass |
| Then close descriptor | Pass | Pass |
| Explicit msync of generation 1 | Pass | Pass |
| Later store after msync, both held | Fail | Pass |
| Then release both | Pass | Pass |

[Original probe](raw/probe-before.jsonl.gz): four failures, no harness errors.
[Experiment](raw/probe-500ms.jsonl.gz): nine checks pass, no harness errors.
The final ordinary mount uses no interval override and also passes
[all nine checks](raw/probe-default.jsonl.gz).
An OS periodic page-out reaches the module near the end of two failed original
phases; its later server publication is ordinary quiet-period latency after
that callback, not a separate publication defect.

The measured experiment used `-o mmap-push-ms=500`. The approved default now uses
the same 500 ms interval on ordinary writable mounts. It retains every writable file
incarnation until reclaim, including files already closed, because FSKit exposes
no mapping-created or mapping-dirty callback. A serial sweep calls
`setCacheState(..., action: .push)` outside module locks; sweeps cannot overlap.
Unmount stops and drains the sweep before releasing the daemon session.
An in-flight synchronous kernel push cannot be interrupted, so it can delay unmount.
The writeThrough grant is unchanged. A cache push preserves cached bytes;
invalidation, which can discard dirty pages, is not used for this purpose.

The full [SQLite rerun](raw/sqlite-500ms.jsonl.gz)
([console](raw/sqlite-500ms-console.jsonl.gz)) has zero failures, including strict
SHM byte equality. The final [ordinary-mount SQLite run](raw/sqlite-default.jsonl.gz)
([console](raw/sqlite-default-console.jsonl.gz)) also has zero failures. Its stated
limitation remains concurrent cross-Mac writers:
locks are local. No `*-shm` names-policy exception was introduced.

**Decision accepted by the user, 14 October:** enable the 500 ms sweep by default
and update the durability table to distinguish unsynchronized mmap stores from
acknowledged writes. Regular writeThrough guarantees and the names policy stay
unchanged. A mapping store without synchronization is not a write acknowledgement;
before a page push, it can exist only in kernel memory. After delivery, the existing
one-second disk flush and two-second quiet publication clock apply. The 500 ms
interval is a scheduling target, not a hard bound: a serial sweep can take longer.
The `mmap-push-ms=0` diagnostic override reproduces behavior without periodic pushes.

## Sweep cost

The isolated publication queue was paused during these measurements and resumed
in `finally`. Each count used a fresh remount, so the candidate count is exact.
All six measurement windows have ten sweeps and zero push errors. Dirty windows
end five seconds after `dirty-held`, excluding subsequent descriptor cleanup.

| Cached writable files | Clean sweep median / maximum | Dirty-window sweep median / maximum |
| --- | --- | --- |
| 0 | 0.0127 / 0.0279 ms | 0.0110 / 0.0151 ms |
| 1 | 0.1313 / 0.1735 ms | 0.1413 / 6.6520 ms |
| 1,000 | 18.2944 / 19.5608 ms | 17.3128 / 792.4435 ms |

The maximum dirty sweep includes pushing all 1,000 changed pages; later sweeps
are clean. This is wall time, not a CPU or battery measurement. It establishes
neither a maximum supported file count nor a bound under every workload.
[Summary and individual sweeps](raw/cost-summary.json), [zero](raw/cost-0.jsonl),
[one](raw/cost-1.jsonl), [1,000](raw/cost-1000.jsonl), and
[commands](raw/cost-commands.jsonl) preserve the measurements. The command log
also retains the first rejected `uploads pause` invocation, which omitted its
required scope and changed no pause state; the actual measurements used `--all`.

## Replay

Prepare a fresh local test drive and development module with the matrix's
[isolated-rig procedure](../mac-app-matrix/README.md#preparing-and-restoring-the-isolated-rig).
Use temporary state, preserve the existing registrations/preferences, and never
write to the shared daemon directory or replace `/Applications/voidfs.app`.
The observer needs the run's `VOIDFS_STATE_DIR`, local credentials and endpoint.

```bash
cc -std=c11 -Wall -Wextra -Werror \
  bench/results/mac-fskit-writable/mmapheld.c -o /private/tmp/voidfs-mmapheld

# Reproduce behavior without periodic pushes:
mount -F -t voidfs -o trace,mmap-push-ms=0 "voidfs://$MATRIX_DRIVE" "$MATRIX_MOUNT"
/usr/bin/python3 apps/macos/scripts/mmap-held.py \
  --mount "$MATRIX_MOUNT" --drive "$MATRIX_DRIVE" --void "$MATRIX_VOID" \
  --state-dir "$VOIDFS_STATE_DIR" --probe /private/tmp/voidfs-mmapheld \
  --out "$MATRIX_OUT/probe-before.jsonl"
umount "$MATRIX_MOUNT"

# Build the changed module, then select it with the same isolated rig:
mount -F -t voidfs -o trace "voidfs://$MATRIX_DRIVE" "$MATRIX_MOUNT"
/usr/bin/python3 apps/macos/scripts/mmap-held.py \
  --mount "$MATRIX_MOUNT" --drive "$MATRIX_DRIVE" --void "$MATRIX_VOID" \
  --state-dir "$VOIDFS_STATE_DIR" --probe /private/tmp/voidfs-mmapheld \
  --out "$MATRIX_OUT/probe-500ms.jsonl"
/usr/bin/python3 apps/macos/scripts/app-matrix.py --row sqlite \
  --root "$MATRIX_MOUNT" --mount "$MATRIX_MOUNT" --drive "$MATRIX_DRIVE" \
  --void "$MATRIX_VOID" --metadata "$MATRIX_METADATA" \
  --out "$MATRIX_OUT/sqlite-500ms.jsonl"
```

Use a fresh remount for each cost count, with only the isolated queue paused.
[`pushcost.py`](pushcost.py) explicitly synchronizes its initial generation,
holds clean mappings for five seconds, then stores a second generation without
synchronization and holds for five seconds. It retains the original descriptors
and Python mmap's duplicate descriptors. Match its wall timestamps to aggregate
FSKit sweep trace records; setup and descriptor-close work are separate phases.

```bash
"$MATRIX_VOID" uploads pause --all
/usr/bin/python3 bench/results/mac-mmap-held/pushcost.py "$MATRIX_MOUNT" 1000 \
  > "$MATRIX_OUT/cost-1000.jsonl"
"$MATRIX_VOID" uploads resume --all
```

## Detector validation

[Runner validation](raw/runner-validation.txt.gz) includes replayable APFS protocol
checks and cleanup/timing fixtures. [Individual breaks](raw/breaks-runner.log)
disable each malformed/error cleanup or late-sample detector separately; each
matching fixture fails. The original module's missing periodic push is the
single production break for the four failing mapped-store checks above.

## Final validation and cleanup

Workspace clippy with warnings denied and all-targets passes; the workspace has
739 passing tests, no failures and nine ignored cases. Protocol validation passes
56 cases/432 steps; all five credential-script checks pass. Local memory, filesystem
and Versity interop pass both addressing styles, admin checks, aws-chunked and
rclone. System Python lacks boto3, so its checks are explicitly skipped locally
and remain covered by required CI.

[Workspace tests](raw/workspace-tests.txt.gz), [clippy](raw/workspace-clippy.txt),
[interop memory](raw/interop-memory.txt), [filesystem](raw/interop-fs.txt) and
[Versity](raw/interop-versitygw.txt) retain the outputs. The final development app
built successfully; [baseline module identity](raw/module-before.sha256),
[experiment mount](raw/experiment-mount.json) and [final default mount](raw/default-mount.json)
distinguish the loaded builds.

The [setup baseline](raw/setup-before.json), [commands](raw/setup-commands.jsonl),
[callback trace](raw/setup-fskit-trace.txt.gz), and [cleanup](raw/cleanup.json) retain
registration and lifecycle evidence. The scratch mount was normally unmounted;
the owned daemon/server/logger stopped. The state-directory override was deleted,
the original `/Applications/voidfs.app` extension path and enabled agent were restored,
and shared daemon state stayed absent. A full LaunchServices refresh was needed
to clear the initially cached development path. No security setting was reset.
The owned server, stage/cache data and synthetic datasets were removed.

Twelve closed logs were losslessly archived from 5,187,237 to 381,374 bytes. The
[archive manifest](raw/archive-manifest.json) records decoded lengths and SHA-256;
each decoded file was verified before removing its original. Read a log with
`gzip -dc raw/NAME.jsonl.gz` or `gzip.open(..., 'rt')`. The existing
`apps/macos/scripts/app-matrix-archive.py` performs this operation and updates
only explicitly listed Markdown links.
