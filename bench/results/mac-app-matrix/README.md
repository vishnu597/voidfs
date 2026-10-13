# macOS application matrix, 13 October 2026

This is the step 5, item 6 rig and its evidence on the writable **FSKit** adapter. Item 6 remains
**not complete**: the two-Mac scenario is untested, remote changes do not notify file watchers,
and the previously documented FSKit rename-swap and cached-miss blockers remain. Expanded GUI controls and unvalidated app recovery/export cases are named explicitly below. A passing operation is scoped to
the operation actually run; it does not make the adapter's release criteria pass.

The writable volume commits from [vishnu597/voidfs#60](https://github.com/vishnu597/voidfs/pull/60)
were promoted to `main` by [#61](https://github.com/vishnu597/voidfs/pull/61), merged as
`0fe8913ba3d2d6a94ab899762943accbff259cca`. The promoted head was
`f2f711e4ef852322121296c1ff8fe83fc655f624`; all eight jobs passed in the exact-head
[CI workflow-dispatch run](https://github.com/vishnu597/voidfs/actions/runs/38411218456).
The [PR-triggered run](https://github.com/vishnu597/voidfs/actions/runs/38411424007) initially
failed and its second attempt passed; the
[main-branch rerun](https://github.com/vishnu597/voidfs/actions/runs/38412574302) also passed.
Their [PR metadata](raw/promotion-pr-ci-rerun.json) and
[main metadata](raw/promotion-main-ci-rerun.json) preserve the current completed conclusions.

## Running a row

Run from the repository root. The commands below use a fresh output directory and **test-only
environment**; do not source the repository's `.env*` files. Existing runs used
`/private/tmp/vmi.de9zgl_x/env.sh`, the `matrix-19835` drive, and
`/private/tmp/vmi.de9zgl_x/mnt`. Those are disposable run paths, not persistent configuration.

After preparing the isolated rig as described below, set the command macros in its shell:

```bash
source "$MATRIX_ENV"             # the isolated rig's test-only env.sh
MATRIX_REPO="$PWD"
MATRIX_VOID="$MATRIX_REPO/target/item6/debug/void"
MATRIX_PEER="$MATRIX_REPO/target/item6/debug/examples/mount_peer"
MATRIX_METADATA="$MATRIX_REPO/target/item6/debug/examples/mount_metadata"
MATRIX_OUT=$(mktemp -d "$MATRIX_REPO/bench/results/mac-app-matrix/raw/rerun.XXXXXX")
MATRIX_CONTROL=$(mktemp -d /private/tmp/voidfs-matrix-apfs.XXXXXX)
```

`MATRIX_MOUNT`, `MATRIX_DRIVE`, and `VOIDFS_STATE_DIR` must point to that run's fresh local test
drive, mount, and daemon state directory. Every mutating row owns new scratch children. The
collector reads an existing row subtree and the corresponding local test server. App profiles,
compiler caches and build products stay outside the mounted dataset.

```bash
xcrun swiftc -swift-version 5 -warnings-as-errors \
  -module-cache-path /private/tmp/voidfs-matrix-swift-cache \
  apps/macos/scripts/foundation-save.swift -o /private/tmp/voidfs-foundation-save
xcrun swiftc -swift-version 5 -warnings-as-errors \
  -module-cache-path /private/tmp/voidfs-matrix-swift-cache \
  apps/macos/scripts/watch-events.swift -o /private/tmp/voidfs-watch-events

/private/tmp/voidfs-foundation-save "$MATRIX_CONTROL" > "$MATRIX_OUT/foundation-apfs.jsonl"
/private/tmp/voidfs-foundation-save "$MATRIX_MOUNT" > "$MATRIX_OUT/foundation-fskit.jsonl"
/private/tmp/voidfs-watch-events local "$MATRIX_CONTROL" both > "$MATRIX_OUT/watch-apfs.jsonl"
/private/tmp/voidfs-watch-events local "$MATRIX_MOUNT" both > "$MATRIX_OUT/watch-local.jsonl"
/private/tmp/voidfs-watch-events peer "$MATRIX_MOUNT" fsevents "$MATRIX_MOUNT" \
  "$MATRIX_PEER" "$MATRIX_DRIVE" > "$MATRIX_OUT/watch-peer-fsevents.jsonl"
/private/tmp/voidfs-watch-events peer "$MATRIX_MOUNT" both "$MATRIX_MOUNT" \
  "$MATRIX_PEER" "$MATRIX_DRIVE" > "$MATRIX_OUT/watch-peer-both.jsonl"
```

The Foundation probe takes an optional final case name, for example `resource-fork-offsets`,
`copyfile-all`, `copyfile-all-no-resource-fork`, or `file-version`. The watcher takes an optional
final case name, such as `create`, `grow`, `rename` or `missed-create`. Exit 1 preserves an
unsupported/failed row, exit 2 reports a harness error, and Foundation exit 3 preserves evidence
of acknowledged wrong bytes or an unrecoverable failed safe save. A failed, non-atomic copy is
recorded as `PARTIAL_FAILURE` with both source and destination; it is not treated as an
acknowledged successful save. Native `NSDocument` safe-save rows disable version storage to
isolate the save strategy; the separate `NSFileVersion` row measures native version storage.

The execution sandbox blocks macOS coordinator/FSEvents services and daemon sockets. Run these
controls with the same permitted service access on APFS and FSKit; an APFS sandbox failure is a
harness limitation, not a volume result. Timed shell runs use
`/usr/bin/perl -e 'alarm shift; exec @ARGV' SECONDS command ...`.

## GUI rerun cookbook

Run the same native workflow on a fresh APFS control and a fresh FSKit fixture. Keep the
window title, selected path, saved bytes, close/reopen result and any alert in an append-only
UI log. `xcodebuild`, AppleScript, CSV/PDF decoding and AVFoundation readback have their own
scope; they do not supply evidence that an unobserved GUI operation succeeded.

These standard-library commands seed new editor, TextEdit and Preview fixtures on both
surfaces. Numbers documents are created in Numbers, and Xcode/media fixtures use the runners
below. Export the paths so the fixture generator can read them:

```bash
export MATRIX_GUI=$(mktemp -d "$MATRIX_MOUNT/gui.XXXXXX")
export MATRIX_GUI_CONTROL=$(mktemp -d "$MATRIX_CONTROL/gui.XXXXXX")
/usr/bin/python3 - <<'PY'
import os
from pathlib import Path
text = b'BT /F1 16 Tf 40 730 Td (voidfs application matrix original PDF) Tj ET\n'
objects = [b'<< /Type /Catalog /Pages 2 0 R >>',
           b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
           b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>',
           b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
           b'<< /Length ' + str(len(text)).encode() + b' >>\nstream\n' + text + b'endstream']
pdf, offsets = bytearray(b'%PDF-1.4\n'), [0]
for number, body in enumerate(objects, 1):
    offsets.append(len(pdf))
    pdf.extend(str(number).encode() + b' 0 obj\n' + body + b'\nendobj\n')
xref = len(pdf)
pdf.extend(b'xref\n0 6\n0000000000 65535 f \n')
for offset in offsets[1:]:
    pdf.extend(('%010d 00000 n \n' % offset).encode())
pdf.extend(b'trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n' + str(xref).encode() + b'\n%%EOF\n')
for variable in ['MATRIX_GUI', 'MATRIX_GUI_CONTROL']:
    folder = Path(os.environ[variable])
    for name in ['cursor.txt', 'zed.txt', 'notes.txt']:
        (folder / name).write_text('voidfs GUI initial draft — 名前\n', encoding='utf-8')
    (folder / 'preview.pdf').write_bytes(pdf)
    print(folder)
PY
```

Set `MATRIX_ROW` to each mounted fixture folder and run the collector before opening it and
again after closing its documents. Initial and edited versions then have distinct evidence.
For GUI rows, observe the actual native controls; preserve a rejected save or alert as its own
result. Do not disable native Versions preferences to make a GUI workflow pass.

| Application | Replay on the owned fixture | Expected readback and evidence |
| --- | --- | --- |
| Cursor / Zed | Open the respective `.txt` with `open -a Cursor` or `open -a Zed`; replace the text with `voidfs GUI edited draft — 名前`, save, use Save As to create a second owned file, close, and reopen the saved path. | Both paths contain the edited UTF-8 marker. Log editor/version/path and actual save/reopen controls; collect bytes and metadata after the editor releases them. Separate logs are required for peer-triggered refresh. |
| TextEdit | Open `notes.txt`; edit, save twice, close and reopen. Separately choose Format → Make Rich Text and Save As an owned `notes.rtf`; edit/save/reopen that RTF. | Exact plain text or decoded RTF marker survives. Record Versions/read-only/autosave alerts and whether the app retained a recoverable draft or duplicate. A failed ordinary save and a successful Save As remain separate outcomes. |
| Preview | Open `preview.pdf`; add a Note containing exactly `voidfs saved annotation — 名前`, save, export as `preview-export.pdf`, close, and reopen both. | One page, original text and the exact Note contents. Run `/private/tmp/voidfs-pdf` on both saved paths and collect originals/exports. Native alerts and appearance are recorded separately from PDFKit decoding. |
| Numbers | Create a blank table with headers `Name`, `Value`, `State`; row 2 is `名前`, numeric `21`, `Original`. Save an owned `Twice Table.numbers`, move it to the mounted fixture using Numbers, and use the native file-type control to make a package. Change B2 to `22` and C2 to `Saved on FSKit`; save, close/reopen, and export an owned CSV. | Native reopened cells show those three final values; the package is a directory and the CSV contains them. Record any permanent-version warning. This fixture does not make a formula compatibility claim. Collect all package files and the CSV. |
| Xcode | Run `app-matrix-xcode.py` on each surface; its emitted root contains `MatrixFixture.xcodeproj` and `Sources/main.swift`. Open that exact project in Xcode, keep project DerivedData/build products in an owned APFS folder, edit the print to `Xcode GUI revision — 名前: 16`, then Cmd-B and Cmd-R. Close the project before final collection. | Native build/run succeeds and Console prints the Unicode marker with exit 0. The fresh PBX fixture sets `ALWAYS_SEARCH_USER_PATHS=NO` in Debug/Release. New headless controls are [APFS](raw/xcode-no-header-search-apfs.jsonl), [FSKit](raw/xcode-no-header-search-fskit.jsonl) and [server](raw/xcode-no-header-search-server.jsonl); they do not claim a new GUI run. |
| QuickTime | Make a fresh movie with `/private/tmp/voidfs-media make <owned.mov> 300 1920 1080 1313`, then open it. Observe playback advancing and seeks forward/back. Duplicate within the owned fixture, attempt Edit → Trim and save/export, close and reopen any output. Repeat identical actions on APFS. | The source is 300 real frames / 10 seconds; native playback/seeks and each output are separate checks. Preserve the alert if trim/save/export rejects. Successful AVFoundation/`avconvert` export does not turn a rejected QuickTime GUI save into a pass. |

The large runner is distinct from the small GUI fixture. Compile the media helper with `-O`
and run `app-matrix-large-media.py --root "$MATRIX_MOUNT" --case media-50gb --target-bytes
50000000000 --frames-per-clip 3000`, plus the common drive/mount/CLI/metadata options and
`--state-dir "$VOIDFS_STATE_DIR" --server-dir "$MATRIX_SERVER_DIR" --out <fresh.jsonl>`.
It verifies alias pause against the mounted drive ID, hashes unpublished source bytes once,
streams server hashes, and records frame/seed/hash/size and sampled disk peaks. Reserve the
declared disk budget before starting. Keep prior failed logs; repeat `--resume-evidence` for
every earlier run to continue only verified clips. Existing clip bodies are never rewritten.
Do not run the general collector across the published 50 GB tree: that would reread every
mounted movie and refill the cache. Use its manifest and streamed server verification.

For a cold client/kernel check, fork the fully published source drive to a new alias, mount that
fork read-only at a fresh path, and resolve its stable drive ID. Before any media open, run
`app-matrix-cold-cache.py` with that ID, alias, state, mount, CLI and a fresh `--out`, plus
`--expect-cold`. It must report zero indexed fork body blocks, stages and journal entries.
Start its bounded monitor in a second terminal while the first native QuickTime open/play/seeks
run; then repeat on the same fork for warm behavior. Server/object-store caches may be warm.
Use the movie's exact URL with `open -a "QuickTime Player" <movie>` after the baseline;
a file chooser or Quick Look preview can read its body before the intended cold open. For
native-framework decoding, run `/private/tmp/voidfs-media inspect <movie>` directly after
verifying that movie's ETag has zero indexed blocks, then repeat for warm frame counts. Gate
the first and last clips separately; an already read first clip cannot establish a cold last
clip without the last clip's own zero-cache observation.
Never hash or inspect the mounted movie before that first native open. The helper observes
cache occupancy; it does not mount, clear caches, copy bodies or operate the GUI.

## Scoring and evidence

| Status | Meaning |
| --- | --- |
| Pass | Every declared operation succeeds with the expected bytes/semantics. Applicable server reconciliation and metadata checks also succeed. The scope is named explicitly. |
| Partial | Useful declared operations pass, but a measured API limitation or an untested part of the app workflow prevents a full row pass. The limitation and missing work are named. |
| Fail | An actual operation, notification requirement, or strict byte/metadata comparison fails. A known limitation remains a failed requirement when it prevents the intended scenario; it is not silently waived. |
| Untested | The app, licence, hardware, or required interaction is unavailable, or no run exists. Substitutes have their own rows. |
| Pending | A row is still in progress in this working draft; it is not counted as a pass. |

JSONL preserves commands, exit codes, timing, SHA-256, mode, mtime, xattr hashes, upload/version
records, and failures. `.DS_Store` is deliberately local under the accepted policy and is
identified separately. No AppleDouble `._` files may be silently ignored. App readback is separate
from server equality: a file must both reopen with the intended content and match the server
after publication. The common collector resolves the drive alias to its server ID, waits for
scoped staged work and queue entries to drain across the quiet period, then checks stable local
bytes, server bytes, version history, and SDK metadata.

Files whose names contain `harness-preflight`, `collector-path-preflight`, `bootstrap-limit` or
`early-open-limit` are retained investigations, not successful controls. Intentional broken
detectors are [Foundation](raw/foundation-apfs-detector-break.jsonl),
[watcher event flags](raw/watch-apfs-flags-break.jsonl), and
[publication draining](raw/drain-self-test-broken.jsonl); their expected failures validate the rig.
Use the final/correct-drain files linked in the matrix for claims about current behavior.

## Environment and matrix

Apple M5 Pro, macOS **27.0.1 (26A434)**, Xcode **27.0 beta (27A5194q)**, Apple Git
**2.54.0 (Git-156)**, SQLite **3.54.0**, system Python **3.9.6**. The initial app/tool inventory
and disk sample are in [inventory.jsonl](raw/inventory.jsonl); that sample had 637 GiB free on
the Data volume. The loopback server uses a filesystem store and `multi-object-versions`, not
the memory store. FSKit callbacks are in [fskit-trace.txt](raw/fskit-trace.txt.gz). The initial daemon reported `f2f711e`, the promoted head. After the fixes were reviewed it was rebuilt from merged `main` (`0fe8913`) plus the recorded working changes in PRs #63–#65; the real-media reruns used that rebuilt daemon/module.

| App / surface | Scope and result | Evidence and limits |
| --- | --- | --- |
| Foundation / AppKit, OS frameworks | **Partial.** Complete APFS control: 21 cases, zero errors. General FSKit run: 19 cases, 13 pass and 6 fail/unsupported. The other two final cases were run separately. Atomic Data writes, replacements with retained/deleted backups, same-volume replacement directory, coordinator, actual NSDocument save/autosave, and old-descriptor consistency pass. | [APFS](raw/foundation-apfs.jsonl.gz), [FSKit general](raw/foundation-fskit.jsonl.gz), [positional fork reads](raw/foundation-fskit-resource-offsets.jsonl), [diagnostic copyfile callback](raw/foundation-fskit-copyfile-trace.jsonl). Native Versions reject safely with Cocoa error 512. Tiny resource forks make copyfile fail as described below. Earlier no-fork Foundation publication control reconciled 28 files: [correct drain](raw/foundation-correct-drain.jsonl.gz); that does not certify all later fork fixtures. |
| FSEvents / kqueue | **Pass locally; fail for peer notifications.** Nine APFS and nine FSKit local operations pass. Seven peer operations produce no notifications within 5 s. Without held descriptors, fresh-path bytes/listings update in 14–31 ms; with kqueue holds, paths remain old/missing while listings update in 12–33 ms, and bytes appear 0.2–5.9 ms after release. | [APFS](raw/watch-apfs-both.jsonl), [FSKit local](raw/watch-fskit-local-both.jsonl), [peer FSEvents only](raw/watch-fskit-peer-fsevents.jsonl), [peer with holds](raw/watch-fskit-peer-both.jsonl). kqueue is not recursive; nested-create registers its subdirectory. Final FSEvents uses FileEvents/UseCFTypes/NoDefer, without WatchRoot; peer results isolate explicit descriptor holds. Remote atomic save and chmod/xattrs are untested because mount_peer exposes put/fill/rename/delete only. |
| Git 2.54.0 | **Pass for tested operations, with the stated symlink limitation.** Local clone of the committed repository, status, edit/add/commit, rebase, stash/pop, checkout, gc/repack, worktree add/remove, index.lock refusal/release and fsck succeed. All 804 source files match; final 872 regular files match server bytes, mode, mtime and xattrs after correct drain. | [APFS](raw/git-apfs.jsonl.gz), [operations](raw/git-fskit.jsonl.gz), [final collector](raw/git-correct-drain.jsonl.gz). FSKit clone sets `core.symlinks=false`; symlink targets become ordinary files. Restore/reopen of an earlier test version passes. No EAGAIN/ESTALE/EIO failure occurred in this operation set. Editor and two-Mac coverage remains separate. |
| Xcode 27.0 | **Pass for tested headless and native GUI operations.** Mounted project/source pass clean and incremental builds and execution. Native Xcode opens/edits/saves the source, builds and runs it, printing `Xcode GUI revision — 名前: 16`, exit 0. | [GUI](raw/xcode-ui.jsonl), [GUI server collector](raw/xcode-ui-collector.jsonl.gz), [warning-free APFS/FSKit controls](raw/xcode-no-header-search-summary.json). DerivedData stayed outside the mount. Native close warns that permanent Versions storage is unavailable; it was acknowledged without changing preferences. Remote GUI refresh and expanded APFS GUI controls are untested. |
| SQLite 3.54.0 | **Pass for durable `.db` semantics; fail for strict SHM equality.** Rollback/WAL transactions, contention, committed-reader visibility, writer process crash rollback, checkpoint/integrity, database restore/reopen and peer replacement of a closed database pass. New connection sees peer commit in 215 ms; an already-open connection keeps its original snapshot. | [APFS](raw/sqlite-apfs.jsonl), [operations](raw/sqlite-fskit-final.jsonl.gz), [correct drain](raw/sqlite-final-correct-drain.jsonl.gz), [43-byte SHM difference](raw/sqlite-shm-byte-diff.json), [stable recheck](raw/sqlite-shm-stable.jsonl.gz). The 32,768-byte `wal.db-shm` differs from its server copy in 43 bytes after drain. The database contents/integrity are correct; the SHM failure stays visible and is not treated as a `.db` pass. Cross-Mac concurrent writers remain unsupported: locks are local. |
| Blender 5.2.2 LTS | **Pass for native background persistence scope.** Twenty semantic checks per surface cover create/save, fresh-process reopen, edit/save, Unicode names/custom metadata, geometry, relative external texture load, animation, native backup reopening with its first revision and rendering the reopened scene. | [APFS](raw/free-app-apfs-2/results.json), [FSKit](raw/free-app-fskit/results.json), [six-artifact common collector](raw/free-app-server-verified.jsonl.gz), [software/runtime evidence](raw/free-app-software.json). Blender GUI editing, peer refresh and a 50 GB project are untested by this small synthetic scene. |
| LibreOffice Writer / Calc 26.8.1.1 | **Pass for native background persistence scope.** Thirty semantic checks per surface cover ODT/ODS create/save, fresh-process reopen/edit/save, Unicode, metadata, formulas and lock-file lifecycle. Together Blender+LibreOffice provide 50 checks on APFS and 50 on FSKit. | [Native controls](raw/free-app-fskit/results.json), [server collector](raw/free-app-server-verified.jsonl.gz). Separate copies restore their first server versions, then fresh native processes verify 18 Blender/Writer/Calc semantics: [restore and native reopen](raw/free-app-recovery/results.json), [checks](raw/free-app-recovery/native-reopen/results.json). LibreOffice is distinct; **Microsoft Office remains untested**. This is version recovery, not crash recovery. |
| Finder 27.0 | **Partial; large-fork copy fails.** Eleven native checks cover 5 MiB copy in/out, Unicode tree, duplicate, rename, small forks/user xattrs, tags/label, Quick Look and Get Info. Fresh Move to Trash deletes the scratch path; server history restores its first published version with exact SHA/mode/mtime/all xattrs. A 65,537-byte fork is silently dropped by successful FSKit Finder copy/duplicate/copy-out, while APFS preserves it. | [APFS final](raw/finder-apfs-final.jsonl), [FSKit final](raw/finder-fskit-final.jsonl), [Trash/recovery and large-fork result](raw/finder-trash-recovery-final/results.jsonl), [restored server collector](raw/finder-trash-recovery-final/restored-server.jsonl), [final three-file collector](raw/finder-trash-recovery-final/after-large-fork-server.jsonl.gz). No `.Trashes`; history supplies recovery. Fresh restored bytes appear in 78 ms. Finder copies above the 5 MiB fixture remain untested; a known silent fork loss is a failed copy requirement. |
| TextEdit 1.21 | **Partial.** Two safe saves/fresh reopen preserve Unicode text. Native Revert To Last Opened recovers previous Unicode bytes; Duplicate saves its own fixture; edits without Command-S reach the duplicate after about 60 s (autosave). A mode-0444 edit prompts to Unlock/Duplicate; Cancel preserves bytes and mode. Browse All Versions opens the current-document view but shows no earlier document. | [APFS basic saves](raw/textedit-apfs.jsonl), [FSKit basic saves](raw/textedit-fskit.jsonl), [sourced native UI observations](raw/textedit-ui-parent-observations.jsonl), [locked-file UI](raw/textedit-locked-ui.jsonl). Earlier native Versions recovery is not demonstrated; the separate NSFileVersion probe rejects with Cocoa 512. Hidden/immutable flags, expanded APFS GUI controls and server restore/reopen remain untested. |
| Preview 11.0 | **Partial.** GUI annotation and PDF export preserve original text and a saved Unicode note in both files, checked through fresh PDFKit reads. | [PDF semantics](raw/preview-readback.jsonl), [native UI observations](raw/native-cocoa-ui.jsonl), [final native-Cocoa byte/metadata collector](../mac-finderinfo/raw/native-export-collector-after.jsonl). FinderInfo listing is fixed in #63. Earlier-version restore and a fresh exported-document GUI reopen are untested. |
| Numbers Creator Studio 15.1 | **Partial.** Native package document saves and freshly reopens Unicode cell text, value 22 and the saved marker. CSV exports decode Unicode/value and reconcile with the server. | [Native observations](raw/native-cocoa-ui.jsonl), [CSV readback](../mac-finderinfo/raw/native-export-read-after.jsonl), [10-file collector](../mac-finderinfo/raw/native-export-collector-after.jsonl). The initial [formula readback](raw/numbers-readback.jsonl) failed after formula entry had already failed before moving onto FSKit; it is retained as harness preflight. Formula computation and package history restore remain untested. Native Versions warns unsupported. |
| Zed 1.7.2 / Cursor 3.24.12 | **Partial.** Native UI open-folder/Unicode edit/save and project search pass on FSKit; each search finds two matches in two files. Open buffers remain old more than 5 s after a peer SDK commit; close/reopen sees the peer bytes. | [GUI observations](raw/editor-ui.jsonl), [Zed saved/server bytes](raw/editor-local-zed.jsonl), [Cursor saved/server bytes](raw/editor-local-cursor.jsonl). GUI reopen latency was not measured against 5 s. APFS GUI controls and two-Mac coverage remain untested; fast fresh-path reads in the watcher probe are separate evidence. |
| QuickTime Player 10.5 | **Partial.** Native UI opens the 1,718,031,721-byte, 100 s ProRes clip, plays for 25.6 s and seeks through Show Clips from 90.03 to 9.97 s. Duplicate creates a whole-size movie with matching header; full duplicate byte/server equality is untested. Cold native open-workflow playback on a fresh read-only fork advances 47.469 s and seeks 90→10→90; see [cold GUI](raw/quicktime-cold-ui.jsonl). The chooser may preload before Play; no zero-cache-at-play claim is made. GUI 0–2 s trim/save fails with the same compatibility alert on this FSKit clip and a 300-frame/10 s APFS control, before an export target or success acknowledgement exists. | [Native UI and APFS control](raw/quicktime-ui.jsonl). Trim/export remains unvalidated due to the app/fixture constraint; the paired failure does not establish a new FSKit-specific bug or acknowledged-save loss. The source's original SHA remains unchanged; GUI access adds only last-used/quarantine metadata versions, as recorded by the media owner. |
| AVFoundation / avconvert | **Pass for the real 50 GB dataset and native decoder/export scope.** 30 unique ProRes clips contain **50,000,175,334 bytes**, 87,305 frames and 2,910.167 seconds; all source/server hashes, sizes, metadata and history match. First/last scrubs, warm repeats and a 2 s HEVC export pass. | [Summary](raw/media-50gb-summary.json), [manifest](raw/media-50gb-manifest.json), [resumed run](raw/media-50gb-resumed.jsonl.gz). A second fresh read-only fork has zero per-ETag cache before each first decoder URL open; all 24 cold/warm scrub ranges decode correctly: [strict cold proof](raw/native-cold-media-inspect.jsonl), [baseline](raw/native-cold-media-baseline.jsonl). Direct first cold/warm wall times 0.629/0.401 s; last 0.464/0.410 s. Sampled shared cache peaks at 4.707 GB, below 8 GB. Loopback server caches may remain warm. Exact editing apps and two Macs remain untested. |
| Photos 12.0 | **Untested.** Import would affect a Photos library and has not been run. | Requires the user's selected import/library scope. |
| Premiere / Final Cut / Microsoft Office | **Untested.** Not installed; require the respective app/licence. | LibreOffice does not certify Office. Other media players do not certify the editing applications. |
| DaVinci Resolve | **Untested.** Not installed; the free download requires registration. | Requires an available application copy and its own native save/reopen row. |
| Two independent Macs | **Untested.** User chose this Mac only. | A second SDK client on this Mac measures server/feed behavior; it cannot prove two-Mac app notifications, locks, crash durability or the 50 GB project criterion. |

## The resource-fork failure

The final fixtures use **17-byte** resource forks, distinct old/new xattrs, permissions and
timestamps. Direct reads on FSKit return 17 bytes at position 0, but `EINVAL` at positions 1,
17 (EOF) and 18. APFS returns 16, 0 and 0 bytes at those positions. The replayable Swift case
is `resource-fork-offsets`; the initial separate native controls are
[APFS](raw/foundation-resource-offset-apfs.txt) and [FSKit](raw/foundation-resource-offset-fskit.txt).
This is additional to item 4's **over-64-KiB** protocol limit.

Apple's primary [copyfile source](https://github.com/apple-oss-distributions/copyfile/blob/main/copyfile.c)
copies small forks by advancing `fgetxattr`'s position until EOF. The copied first chunk succeeds,
then the EOF read fails here. This explains why COPYFILE_XATTR and COPYFILE_ALL fail, while
COPYFILE_DATA, COPYFILE_STAT and COPYFILE_ALL after removing only the forks pass. The
[callback evidence](raw/foundation-fskit-copyfile-trace.jsonl) and
[general cases](raw/foundation-fskit.jsonl.gz) retain both destination and source states.

COPYFILE_ALL over an existing destination truncates it before that error, leaving an empty
destination with copied xattrs and an intact source. This is a visible failed non-atomic copy,
not a successful save whose bytes disappeared. Its initial stop/investigation remains in
[foundation-fskit-copyfile-initial-partial.jsonl](raw/foundation-fskit-copyfile-initial-partial.jsonl);
the final probe classifies it as `PARTIAL_FAILURE` rather than hiding it or continuing as a pass.

The installed SDK and Apple's [XattrHandler documentation](https://developer.apple.com/documentation/fskit/fsvolume/xattrhandler)
give the module name, item and context, returning the complete xattr value; they expose no
positional parameter. No contained module fix can choose a suffix or EOF for an offset it is not
given. Changing the protocol's maximum xattr size would not fix these small-fork EOF reads.
Treat this as an FSKit API/kernel limitation requiring an Apple change or a newly supported API;
the matrix does **not** claim that Apple's FAT module has been tested for this offset behavior.
No capability was falsely advertised and no resource fork was deliberately dropped to make the
copy pass.

## Contained fixes found by the matrix

Preview PDF and Numbers CSV exports exposed a separate, now-fixed all-zero FinderInfo listing
defect. They store exactly 32 zero bytes as `com.apple.FinderInfo`; the FSKit kernel reports
`ENOATTR` for the read, while the old module listed its name. The module now omits only that
exact zero32 value from listings, matching APFS absence. Stored values/history are retained,
and nonzero FinderInfo plus ordinary empty/all-zero xattrs stay visible. The focused probe
passes all 12 required listing checks after rebuilding and normally remounting, compared with
two failures before. Two CREATE/REPLACE storage-policy differences remain stated separately.
[FinderInfo fix PR #63](https://github.com/vishnu597/voidfs/pull/63) and
[FinderInfo evidence and replay](../mac-finderinfo/README.md) include current PDF/CSV
framework readback and a 10-file native-Cocoa subtree collector with no failures. The collector
normalizes only raw remote zero32 FinderInfo to macOS absence and records the raw values plus
that normalization; it does not ignore generic xattr errors. The small resource-fork failure
above remains open.

Drive-scoped upload pause resolved to an alias while mounted saves were queued by stable ID,
so mounted entries could continue publishing. Its contained fix preserves both entry
spellings, applies pause/resume/cancel to both, uses the validated offline identity memo and
migrates old alias-only pauses before the queue worker starts. All three new regressions fail
the promoted old code; all five upload tests pass with only this fix on promoted main.
[Focused upload-control evidence](../mac-upload-drive-scope/README.md) is packaged for separate
[PR #64](https://github.com/vishnu597/voidfs/pull/64). The fixed media runner pauses by alias, checks that the mounted stable ID is paused, and restores that scoped pause after source hashing.

Separately, the new 171,808,569-byte video in
[media preflight](raw/media-small-fskit.jsonl.gz) fails publication because multipart completion
cannot carry the required absence guard. The guarded, bounded stream fix in [PR #65](https://github.com/vishnu597/voidfs/pull/65) publishes the original retained movie with its original SHA and metadata; a fresh save/export and the full 50 GB dataset then pass. [Focused evidence](../mac-guarded-large-put/README.md) preserves failing old/weakened-code regressions and successful native reruns. The original preflight remains a failure with staged bytes retained.

## Other rows and final server verification

The Finder follow-up owns a new synthetic `duplicate.bin`, publishes two distinct byte and
metadata revisions, invokes Finder Move to Trash, confirms the server's deleted entry and
restores the first version. The first/edited/restored versions are `7241.0`, `7246.0` and
`7248.0`. Full restored local and raw server xattrs, mode and mtime match the first revision.
Its large-fork source/control is outside the mount; the source contains 43,008 data bytes and
65,537 fork bytes. APFS copy/duplicate/copy-out pass, while all three FSKit destinations lack
the fork after Finder returns success. The final collector checks what was actually stored;
matching those fork-less server copies does not turn them into successful source copies.
The final three-file collector has zero failures and states two local `.DS_Store` exclusions
under the accepted policy. Those exclusions are separate from the three failed source-fork
comparisons.

The first [Trash harness preflight](raw/finder-trash-recovery-harness-preflight/results.jsonl)
failed before deletion because Finder could not resolve `folder (POSIX file ...)`. The final
runner preserves a copied AppleScript and the exact adjustment: explicitly coercing only its
own scratch `duplicate.bin` path to an alias. The shared approved script is unchanged; the
operation is still Finder's native `delete` (Move to Trash), rather than a POSIX deletion.
The runner returns exit 1 because the large-fork copy fails, while its separate `trashRecovery`
result is `pass`.

The Python runners use the standard library and system Git/SQLite; they do not install packages.
Server mode/xattr evidence uses the repository's `mount_metadata` SDK example. The free-app
runner uses Blender's Python and LibreOffice's bundled Python/UNO bridge, a private named pipe,
and fresh application profiles. The vendor app copies were temporary, outside `/Applications`.
Blender required ordinary unsandboxed Metal access; only the scratch LibreOffice bundled Python
executable needed ad hoc signing. Download URLs, checksums and these runtime adjustments are in
[free-app-software.json](raw/free-app-software.json).

```bash
/usr/bin/python3 apps/macos/scripts/app-matrix.py --row git \
  --root "$MATRIX_CONTROL" --source "$MATRIX_REPO" --out "$MATRIX_OUT/git-apfs.jsonl"
/usr/bin/python3 apps/macos/scripts/app-matrix.py --row sqlite \
  --root "$MATRIX_CONTROL" --out "$MATRIX_OUT/sqlite-apfs.jsonl"

/usr/bin/python3 apps/macos/scripts/app-matrix.py --row git \
  --root "$MATRIX_MOUNT" --source "$MATRIX_REPO" --mount "$MATRIX_MOUNT" \
  --drive "$MATRIX_DRIVE" --void "$MATRIX_VOID" --metadata "$MATRIX_METADATA" \
  --out "$MATRIX_OUT/git-fskit.jsonl"
/usr/bin/python3 apps/macos/scripts/app-matrix.py --row sqlite \
  --root "$MATRIX_MOUNT" --mount "$MATRIX_MOUNT" --drive "$MATRIX_DRIVE" \
  --void "$MATRIX_VOID" --metadata "$MATRIX_METADATA" --peer "$MATRIX_PEER" \
  --out "$MATRIX_OUT/sqlite-fskit.jsonl"

/usr/bin/python3 apps/macos/scripts/app-matrix-xcode.py \
  --root "$MATRIX_CONTROL" --out "$MATRIX_OUT/xcode-apfs.jsonl"
/usr/bin/python3 apps/macos/scripts/app-matrix-xcode.py \
  --root "$MATRIX_MOUNT" --out "$MATRIX_OUT/xcode-fskit.jsonl"

/usr/bin/python3 apps/macos/scripts/app-matrix-free.py \
  --folder "$MATRIX_MOUNT/free-apps-$(date +%s)" --output "$MATRIX_OUT/free-apps" \
  --blender "$MATRIX_BLENDER/Contents/MacOS/Blender" \
  --libreoffice "$MATRIX_LIBREOFFICE/Contents/MacOS/soffice" \
  --lo-python "$MATRIX_LIBREOFFICE/Contents/Resources/python" --label FSKit

/usr/bin/python3 apps/macos/scripts/app-matrix-native.py --row finder \
  --root "$MATRIX_MOUNT" --out "$MATRIX_OUT/finder.jsonl"
/usr/bin/python3 apps/macos/scripts/app-matrix-native.py --row textedit \
  --root "$MATRIX_MOUNT" --out "$MATRIX_OUT/textedit.jsonl"

/usr/bin/python3 apps/macos/scripts/app-matrix-finder-recovery.py \
  --mount "$MATRIX_MOUNT" --drive "$MATRIX_DRIVE" --void "$MATRIX_VOID" \
  --metadata "$MATRIX_METADATA" --out "$MATRIX_OUT/finder-trash-recovery"

xcrun swiftc -O -swift-version 5 -module-cache-path /private/tmp/voidfs-media-cache \
  apps/macos/scripts/app-matrix-media.swift -o /private/tmp/voidfs-media
/usr/bin/python3 apps/macos/scripts/app-matrix-native.py --row media \
  --root "$MATRIX_MOUNT" --media-bin /private/tmp/voidfs-media --out "$MATRIX_OUT/media.jsonl"

xcrun swiftc -swift-version 5 -module-cache-path /private/tmp/voidfs-pdf-cache \
  apps/macos/scripts/app-matrix-pdf.swift -o /private/tmp/voidfs-pdf
# After the Preview fixture is annotated/exported, pass both saved PDF paths:
/private/tmp/voidfs-pdf "$MATRIX_PREVIEW_ORIGINAL" "$MATRIX_PREVIEW_EXPORT"
```

Use fresh free-app paths and output folders for APFS controls too; do not reuse the completed
FSKit dataset. `MATRIX_BLENDER` and `MATRIX_LIBREOFFICE` name already available scratch app copies.
The scripts do not download apps, change installed copies, or sign them automatically. The native
media command is a small real-video control, not the full 50 GB workload. The GUI cookbook below reproduces tested interactions; untested extensions are named in the matrix.

Every runner emits the exact artifact folder. Set `MATRIX_ROW` to that mounted subtree and run:

```bash
/usr/bin/python3 apps/macos/scripts/app-matrix.py --row collect \
  --root "$MATRIX_ROW" --mount "$MATRIX_MOUNT" --drive "$MATRIX_DRIVE" \
  --void "$MATRIX_VOID" --metadata "$MATRIX_METADATA" --out "$MATRIX_OUT/row-server.jsonl"
```

Do not infer publication from one empty queue/status sample. The collector waits through the
two-second quiet period and checks scoped unpublished/dirty work; stable bytes must still match
the server. Earlier collectors that used a too-weak drain or a display alias as the queue drive
ID are retained only as preflight evidence. SDK version IDs are recorded per file in the final
collector logs. Native app backups are separate from server history; the Git/SQLite rows also
exercise server restore and exact reopen, and separate Blender/Writer/Calc copies also restore earlier server versions and freshly reopen in the native apps.

The Git timing sample is not a controlled throughput benchmark: FSKit ran alongside other rows.
Its clone/status/gc/repack took 102.061/2.067/7.985/1.977 s; APFS took
0.207/0.058/0.071/0.029 s. The commands and workload sizes are in the linked Git logs.

## Preparing and restoring the isolated rig

Closed raw logs of at least 64 KiB are stored as deterministic `.gz` files without filtering
the original records. The [archive manifest](raw/archive-manifest.json) records original and
archive byte counts and SHA-256. The helper verifies decoded bytes before removing originals and syncs
the files and their containing directories. Read an archive with `gzip -dc raw/NAME.jsonl.gz`
or Python `gzip.open(..., 'rt')`; small logs stay plain. Preview the plan with:

```bash
/usr/bin/python3 apps/macos/scripts/app-matrix-archive.py \
  --raw-dir bench/results/mac-app-matrix/raw \
  --markdown bench/results/mac-app-matrix/README.md \
  --markdown docs/step-5-macos.md \
  --markdown docs/PARITY.md \
  --markdown apps/macos/README.md
```

Add `--apply` only after every evidence writer has stopped. The manifest and extraction instructions remain with the losslessly archived corpus.
This run archived 36 files from 112,138,593 bytes to 6,749,603 bytes; every decoded source SHA-256 matches.

Only a newly created drive on the local test server is mounted writable. Build the CLI/server
and the existing SDK helpers with `CARGO_TARGET_DIR=target/item6`, using `cargo build --locked
-p voidfs-cli -p voidfs-server` and `cargo build --locked -p voidfs-sdk --example mount_peer
--example mount_metadata`. Build the macOS development app as documented in
[apps/macos/README.md](../../../apps/macos/README.md); these probe scripts add no Swift package or
Rust dependency.

Create a short `mktemp -d /private/tmp/voidfs-matrix.XXXXXX` run directory. Its `env.sh` should
export the run's `VOIDFS_STATE_DIR`, endpoint `http://127.0.0.1:9000`, throwaway local credentials,
`MATRIX_MOUNT`, `MATRIX_DRIVE`, and `MATRIX_ENV`. The local-only throwaway credentials used here
are `VFSPIKEKEY0000000000` / `spikesecretspikesecretspikesecretspike1`. Start `voidfs-server` with
`--store fs:<run-directory>/store --listen 127.0.0.1:9000 --new-pool-feature multi-object-versions`
and those credentials; record the PID of the process you started. Create the scratch drive and
start its daemon with the isolated environment. Read its PID only through `void daemon status`.
Confirm that `~/Library/Application Support/voidfs` is absent before and after the run.

The following creates those setup files for a new run. Choose an unused loopback port before
starting; if 9000 is occupied, change both endpoint/listen values here. The key is the local
throwaway key above. Keep `env.sh` private and do not commit it. The state path stays short
enough for the daemon's Unix socket.

```bash
MATRIX_REPO="$PWD"
MATRIX_RUN=$(mktemp -d /private/tmp/vmr.XXXXXX)
MATRIX_ENV="$MATRIX_RUN/env.sh"
mkdir "$MATRIX_RUN/mnt" "$MATRIX_RUN/control" "$MATRIX_RUN/server" "$MATRIX_RUN/setup"
cat > "$MATRIX_ENV" <<EOF
export VOIDFS_STATE_DIR="$MATRIX_RUN/state"
export VOIDFS_ENDPOINT="http://127.0.0.1:9000"
export VOIDFS_LISTEN="127.0.0.1:9000"
export VOIDFS_STORE="fs:$MATRIX_RUN/server"
export VOIDFS_ACCESS_KEY_ID="VFSPIKEKEY0000000000"
export VOIDFS_SECRET_ACCESS_KEY="spikesecretspikesecretspikesecretspike1"
export MATRIX_ENV="$MATRIX_ENV"
export MATRIX_MOUNT="$MATRIX_RUN/mnt"
export MATRIX_DRIVE="matrix-$(date +%s)"
export MATRIX_SERVER_DIR="$MATRIX_RUN/server"
EOF
chmod 600 "$MATRIX_ENV"
source "$MATRIX_ENV"
MATRIX_VOID="$MATRIX_REPO/target/item6/debug/void"
MATRIX_APP="$MATRIX_REPO/apps/macos/build/Build/Products/Release/voidfs.app"
"$MATRIX_REPO/target/item6/debug/voidfs-server" \
  --new-pool-feature multi-object-versions > "$MATRIX_RUN/setup/server.log" 2>&1 &
MATRIX_SERVER_PID=$!
printf '%s\n' "$MATRIX_SERVER_PID" > "$MATRIX_RUN/setup/server.pid"
MATRIX_READY=0
for MATRIX_ATTEMPT in {1..15}; do
  if /usr/bin/perl -e 'alarm shift; exec @ARGV' 3 "$MATRIX_VOID" --json drives \
      > "$MATRIX_RUN/setup/server-ready.json" 2> "$MATRIX_RUN/setup/server-ready.stderr"; then
    MATRIX_READY=1
    break
  fi
  sleep 1
done
test "$MATRIX_READY" = 1 || { echo 'Owned local server did not become ready; inspect setup/server.log'; exit 1; }
"$MATRIX_VOID" drive create "$MATRIX_DRIVE" > "$MATRIX_RUN/setup/drive-create.txt"
"$MATRIX_VOID" --json drive show "$MATRIX_DRIVE" > "$MATRIX_RUN/setup/drive.json"
"$MATRIX_VOID" daemon start > "$MATRIX_RUN/setup/daemon-start.txt"
"$MATRIX_VOID" --json daemon status > "$MATRIX_RUN/setup/daemon-status.json"
"$MATRIX_VOID" --json status > "$MATRIX_RUN/setup/status.json"
```

Before mount registration changes, retain `preferences-before.txt` from `defaults read
dev.voidfs.app`, `extension-before.txt` from `"$MATRIX_APP/Contents/MacOS/voidfs" status`, and
`agent-before.txt` from its `agent-status`. A missing preferences domain is an explicit absent
baseline, not an empty file to infer later. Record the actual prior app paths from the status
output and the original override value. For this run, the prior application was
`/Applications/voidfs.app`; a replay must use its own observed baseline. Put these files under
`$MATRIX_RUN/setup` and retain matching after-restoration files. The example setup writes no
test file or key to the shared daemon directory.

Preserve `defaults read dev.voidfs.app` and the app/extension registration status before changing
the developer override. Set `VoidfsStateDirectory` to the test state, register the development
app's agent, unregister other app copies from LaunchServices, and add that build's
`Contents/Extensions/VoidfsFS.appex` with `pluginkit -a`. Confirm the enabled extension path points
to that build. If macOS requires user enablement, the user acts in System Settings → General →
Login Items & Extensions → File System Extensions; the rig does not change security settings.
An already-running `fskit_agent` can retain a previous copy, which may require user logout/login.

For an already built and permitted development copy, the recorded registration/mount commands
are:

```bash
defaults write dev.voidfs.app VoidfsStateDirectory "$VOIDFS_STATE_DIR"
"$MATRIX_APP/Contents/MacOS/voidfs" agent-unregister
"$MATRIX_APP/Contents/MacOS/voidfs" agent-register
# Unregister only the prior app path(s) recorded by this run before selecting the test copy.
pluginkit -a "$MATRIX_APP/Contents/Extensions/VoidfsFS.appex"
"$MATRIX_APP/Contents/MacOS/voidfs" status > "$MATRIX_RUN/setup/extension-selected.txt"
"$MATRIX_APP/Contents/MacOS/voidfs" agent-status > "$MATRIX_RUN/setup/agent-selected.txt"
log stream --predicate 'subsystem == "dev.voidfs"' \
  > "$MATRIX_RUN/setup/fskit-trace.txt" 2>&1 &
MATRIX_LOG_PID=$!
printf '%s\n' "$MATRIX_LOG_PID" > "$MATRIX_RUN/setup/log.pid"
mount -F -t voidfs -o trace "voidfs://$MATRIX_DRIVE" "$MATRIX_MOUNT"
mount > "$MATRIX_RUN/setup/mount-table.txt"
"$MATRIX_VOID" --json daemon info > "$MATRIX_RUN/setup/daemon-info.json"
```

Successful setup has the owned local endpoint/state in `daemon-status.json`, a resolved drive ID
in `drive.json`, the selected development extension path in `extension-selected.txt`, and a
`voidfs://<scratch-drive>` line at the expected mount in `mount-table.txt`. Keep commands, return
codes and selected paths in the run evidence as well as these snapshots. The existing run's
initial snapshots are [setup](raw/setup.jsonl) and [mount setup](raw/mount-setup.jsonl); its
[cold-fork setup](raw/cold-media-setup.jsonl) records the separate read-only identity. Those
setup captures include mixed command output, so parse the specifically named JSON snapshots
or individual framed records rather than treating every line as JSON.

Capture `log stream --predicate 'subsystem == "dev.voidfs"'` to a fresh evidence file, then mount
the local scratch resource with `mount -F -t voidfs -o trace voidfs://<scratch-drive> <empty-mount>`.
The initial sandbox attempt in [mount-setup.jsonl](raw/mount-setup.jsonl) failed with “Network is
down”; subsequent permitted execution provided the volume captured in the callback trace.
Do not take a sandbox service-access failure as a successful mount, or change the user's shared
daemon to work around it.

After collecting evidence, unmount the test volume, stop its test daemon and only the server/log
processes started by this run, unregister the development agent, delete the temporary
`VoidfsStateDirectory` override, and restore the pre-run app/extension registrations. Re-register
the original `/Applications/voidfs.app` as found; do not replace its files. Its previously known
Developer-ID agent launch constraint may still require the user to toggle Login Items, log out
and in, or reboot. Do not reset background-item/security settings to force it. Verify defaults,
registration paths, absence of test mounts/processes, and absence of shared daemon state again.
Retain the raw evidence in this repository, then remove the owned server store, staged/cache
data, synthetic datasets, compiler caches, temporary app copies and DMGs. Final cleanup and
registration restoration are **pending in this working draft** and must be recorded before the
run is declared finished.

## Final verification and cleanup

The combined client/daemon/module fixes pass the [local workspace tests](raw/combined-workspace-tests.txt.gz) and [all-target clippy](raw/combined-workspace-clippy.txt); protocol validation (56 cases/432 steps) and five credential-script checks pass. Local memory, filesystem and Versity interoperability runs pass; local system Python lacks boto3, so that client is explicitly skipped locally and covered by each PR's required CI jobs. See `raw/combined-workspace-tests.txt`, `raw/combined-workspace-clippy.txt` and `raw/combined-interop-*.txt`. [Recorded required CI](raw/fix-pr-ci.json) for #62–#65 is green; #63–#65 were retargeted to main before their shared fixture-fix base is merged. These follow-up PRs remain open for review.

All three test mounts were normally unmounted, and only the test daemon/server/logger PIDs were stopped. The installed `/Applications/voidfs.app` and its enabled agent registration were restored; the temporary state-directory preference was deleted. Shared daemon state remained absent. All synthetic datasets, temporary free-app copies and newly created Rust/Swift build scratch were removed; [cleanup evidence](raw/cleanup.json) records each owned path and the recovered disk space. No production drive, real credentials, library, specification or security setting was changed.
