# All-zero FinderInfo listings on FSKit

13 October 2026, macOS 27.0.1 (26A434), Xcode 27 beta (27A5194q). The test mount used a local filesystem-store server and an isolated daemon state directory.

Preview's PDF export and Numbers' CSV export stored exactly 32 zero bytes as `com.apple.FinderInfo`. FSKit listed that name, but the kernel's `getxattr` returned `ENOATTR` (93), so `xattr -l` and metadata collection failed. APFS treats that fixed-size all-zero structure as absent: it omits the name and returns `ENOATTR` on read. The raw server values are preserved in [native-export-server-before.json](raw/native-export-server-before.json).

The module now reads FinderInfo when its name is listed and omits only the exact 32-byte all-zero value from the returned names. Stored values and server history stay intact. Nonzero FinderInfo and ordinary empty or all-zero xattrs remain visible. No generic xattr read error is ignored.

| Focused listing probe | Failed required checks | Separate CREATE/REPLACE differences |
| --- | ---: | ---: |
| [APFS control](raw/apfs-listing.jsonl) | 0 | 0 |
| [Old mounted module](raw/fskit-listing-before.jsonl) | 2 | 2 |
| [Rebuilt and remounted module](raw/fskit-listing-after.jsonl) | 0 | 2 |

The 12 required checks cover zero FinderInfo omission and absent reads, ordinary nonzero FinderInfo visibility/readability, clearing its value, a rejected 33-byte write (`ERANGE`, 34), and custom empty/32-byte-zero attributes retaining their values.

Two storage-policy differences remain unchanged and are explicitly reported by the probe. After an all-zero FinderInfo value, `XATTR_CREATE` of nonzero FinderInfo succeeds on APFS but returns `EEXIST` (17) on FSKit because the raw value is stored. `XATTR_REPLACE` after clearing FinderInfo returns `ENOATTR` on APFS but succeeds on FSKit. `listing` reports these observations without claiming full APFS policy parity; `full` scores them as failures.

Build and run the probe first on APFS, then on the old test mount, and again after rebuilding the module and normally unmounting/remounting the test resource:

```sh
xcrun swiftc -swift-version 5 -module-cache-path /private/tmp/finderinfo-cache \
  apps/macos/scripts/app-matrix-finderinfo.swift -o /private/tmp/finderinfo-probe
/private/tmp/finderinfo-probe /private/tmp/apfs-control listing
/private/tmp/finderinfo-probe /Volumes/test-drive listing
```

Each run creates and retains its own synthetic UUID folder below the supplied scratch parent. The after test used the rebuilt module, confirmed by the normal remount and by the two existing exports' successful [PDF](raw/preview-xattrs-after.txt) and [CSV](raw/numbers-xattrs-after.txt) xattr reads. [Native framework reads](raw/native-export-read-after.jsonl) also open the PDF with PDFKit and decode the CSV with Foundation UTF-8, including its `名前,22` row; they do not imply spreadsheet GUI or formula coverage.

The separate application-matrix collector explicitly normalizes only raw remote zero32 FinderInfo to macOS absence and retains the raw server attributes plus a normalization note. Its after evidence is in [native-export-collector-after.jsonl](raw/native-export-collector-after.jsonl). Original application-matrix logs remain in `bench/results/mac-app-matrix/raw/`.

The native framework reader can be rerun on synthetic Preview and Numbers exports:

```sh
xcrun swiftc -swift-version 5 -module-cache-path /private/tmp/finderinfo-cache \
  bench/results/mac-finderinfo/read-exports.swift -o /private/tmp/read-exports
/private/tmp/read-exports /Volumes/test-drive/preview-export.pdf \
  /Volumes/test-drive/numbers-matrix.csv
```
