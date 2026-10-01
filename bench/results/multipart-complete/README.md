# Multipart completion without a chain of round trips

*30 September 2026, one Mac (Apple M5 Pro, 15 CPUs, macOS 27). versitygw 1.8.0 as the bucket, 12 ms
away through `BENCH_ONE_WAY_MS=4 bench/scripts/local.sh` and on loopback, with voidfs-server's
512 MiB cache. Step 3, item 5 of the [parity plan](../../../docs/PARITY.md#7-step-by-step-plan)
([step-3-performance.md](../../../docs/step-3-performance.md#item-5-multipart-completion)):
completing a multipart upload read the upload's record, listed the parts, read each part's record
one after another, committed, and deleted the staging records before it answered. Now it reads the
upload's record and the parts' records in one round trip, answers once it commits, and deletes the
staging records after. Each UploadPart looks its upload up while it reads the part, not before.*

| Files | voidfs-server |
|---|---|
| `rtt12-main-*`, `loopback-main-*` | `main` at `a03a25d` |
| `rtt12-branch-*`, `loopback-branch-*` | This branch |

Every run recorded voidfs's own requests to the bucket (`BENCH_BUCKET_REQUESTS=1`), and the
server's metrics were scraped every 2 seconds. Each binary was built with `cargo build --release
-p voidfs-server`, in a target directory of its own (`main` in a separate worktree), and passed
with `BENCH_SERVER_BIN`; their hashes differ, and only the branch's has `staging records of a
completed upload` in it (`strings`). The harness ran from the `main` worktree with
`BENCH_POOL_FEATURES=inline-data`, so every pool listed `inline-data`. Every file's `commit` label
says `a03a25d`; the file names say which server ran.

The focused runs' files are not kept; the tables below have their figures. Three diagnostic builds
were used, none of them in the branch:

| Name | voidfs-server |
|---|---|
| `mpt` | `main`, printing how long each step of a multipart upload took |
| `mptb` | The branch, printing the same |
| `diag` | The branch, with `VOIDFS_DIAG_MP_OLD` set turning completion and UploadPart back into `main`'s: the lookup before the part's body; a listing and one record at a time; the staging records deleted before answering |

Run order:
1. `mpt` at 12 ms, the two multipart rows, once; then `mptb` the same way once the branch was built.
2. Focused, A B B A B A A B, four times the operations: the two multipart rows, and put 64 and
   32 MiB as controls. At 12 ms, then on loopback.
3. The full 49 rows at 12 ms (A B B A), and on loopback (A B B A, then B A A B: `main-1`,
   `branch-1`, `branch-2`, `main-2`, `branch-3`, `main-3`, `main-4`, `branch-4`).
4. `diag` against itself, switched on and off, A B B A B A A B, at 12 ms, on the same four rows.
5. Focused again on the rows the full runs showed slower, `main` against the branch and then
   `diag` switched, A B B A B A A B each: five rows at 12 ms (four times the operations), six on
   loopback (eight times).

No run had errors.

## Where the time went

`mpt` at 12 ms, 35 uploads of each size (the warm-up rounds included), eight at once. The harness
creates an upload, sends all its parts at once (up to 64 requests in flight across the eight
uploads), then completes it. Medians, in milliseconds:

| Completion, `main` | 64 MiB, 8 parts | 256 MiB, 16 parts |
|---|--:|--:|
| Read `upload.json` | 11.1 | 11.9 |
| List the parts (11.8), then read each record, one after another | 102.8 | 197.8 |
| Describe the content | 0.0 | 0.0 |
| Commit | 14.8 | 16.1 |
| Delete the staging prefix, before answering | 24.6 | 27.9 |
| **Completion** | **156.7** | **257.5** |
| The whole upload, as the harness saw it | 452 | 1,163 |

| Each part, `main` | 8 MiB | 16 MiB |
|---|--:|--:|
| Read `upload.json`, before the body is read | 25.5 | 24.3 |
| Read the body, cut and upload its shards | 143.2 | 251.4 |
| Write the part's record | 11.8 | 19.2 |

- Completion was a third of the 64 MiB upload's time and a fifth of the 256 MiB's, and two thirds
  to three quarters of it was reading the part records one at a time: about 11.5 ms each.
- Each part waited for a read of `upload.json` before the server read its body: 12 ms at best,
  25 ms at the median with 64 requests in flight.
- The rest is taking in the parts. The server's CPU was not what an upload waited for, so it was
  not profiled.

## What changed

In [object.rs](../../../crates/voidfs-server/src/s3/object.rs):
- **Completion reads its records together.** The upload's record, and the record of each part the
  request lists, by name (`<part>.json`, as UploadPart writes them), in one round trip, at most 32
  at a time. Nothing is listed. The errors come in the same order as before: the upload's first,
  then the request body's, then each part's in the order listed. Reading stops at the first part
  that is missing, with at most 32 reads in flight.
- **It answers once it commits,** and deletes the staging prefix after: at once, then again after
  0.5, 2, 8 and 32 seconds if that fails. At shutdown, what is still there is deleted once more. A
  staging prefix that cannot be deleted stays a GC root until the collector aborts the upload
  (format §11), as before.
- **A claim per upload,** kept in memory by the server (`Uploads`), says an upload is completed
  until its staging records are gone. Completions and aborts of one upload take turns.
- **The completion runs in a task of its own,** so that a client that goes away while it commits
  cannot leave the upload committed and still open.
- **UploadPart reads the part while it looks the upload up.** If there is no upload, it answers
  NoSuchUpload as soon as the lookup says so and drops the part; the shards it uploaded by then are
  left to the garbage collector. A part whose upload is completed while it is read is refused
  before its record is written.
- **ListParts** reads the records it lists at most 32 at a time, not one after another.

The on-bucket format is unchanged: the same records, written and deleted by the same rules
(format §11). GC is unchanged: open uploads' records are roots until deleted, and a completion
commits only shards it saw in part records it read.

## What a client sees

Approved before it was built. Within one server:

| | Before | Now |
|---|---|---|
| Two CompleteMultipartUploads of one upload at once | Both could commit: two versions | The second waits for the first; it finds no upload (NoSuchUpload) if the first committed, and completes it itself if the first failed |
| A retry after a completion answered | NoSuchUpload, once the delete before the answer succeeded; if it failed (the error was ignored), a second version | NoSuchUpload |
| ListMultipartUploads, ListParts, UploadPart or an abort right after a completion | The upload is gone | The same, while the staging records are still being deleted |
| An abort while a completion runs | Both succeed: 204, and the object is created | The abort waits; NoSuchUpload if the completion committed |
| A client that goes away while its completion commits | The commit lands and the staging records stay: a retry commits a second version | The completion finishes, and the retry finds no upload |
| UploadPart to an upload that does not exist | NoSuchUpload before the body is read | NoSuchUpload once the lookup answers (one round trip), with part of the body read and some of its shards uploaded |

Another server on the same pool, or this one after a restart, sees a completed upload as open
until its staging records are deleted, which is at most the time the delete takes unless it fails.
Before, the same happened when the delete failed or the server stopped between the commit and the
delete. The claim is per server: two servers completing one upload at once can still both commit,
as before.

## Tests

In `object.rs`, each seen to fail with the code it guards broken (19 ways: records read one at a
time, the upload's record read before the parts', no check of the claim, no turn, a claim never
released, a failed completion marked done, an abort taking no turn, one delete attempt only, no
delete at shutdown, the part's lookup before its body, a lookup that waits for the whole body, no
check before a part's record is written, a check that misses a claim already released, completed
uploads listed, the completion in the request's task, the parts' errors before the upload's, the
parts in the wrong order, the attributes dropped, `upload.json` read as a part):
- a completed upload is its parts in the order listed: the bytes, the extents of the part records,
  the ETag those give, the content type and metadata it was created with, and the version its
  answer names;
- a completion reads its three records at once (held until all three are in flight) and lists
  nothing;
- once a completion answers, with the delete held, a retry finds no upload and commits nothing,
  and ListMultipartUploads, ListParts, UploadPart and an abort do not see it;
- racing completions commit once; one after a completion that failed completes the upload;
- an abort ends an upload; one during a completion waits for it and finds no upload;
- a failed delete is tried again; at shutdown, what is left is deleted;
- a part's shards upload while its lookup is held; with no upload, it answers without waiting for
  the rest of the body; a part read while its upload completes is refused and not recorded;
- the errors come in the same order as before;
- ListParts lists each part as last uploaded, in order; ListMultipartUploads lists the open
  uploads.

`cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass, and
`tests/interop/run.sh` over `memory`, `fs` and `versitygw`: 38 conformance cases in both addressing
styles, the rclone checks (a copy with a multipart upload among them), aws-chunked and the admin
listener. The boto3 checks were skipped: boto3 is not installed on this Mac (CI requires them).

## Completion answers in two round trips

`mptb`, the same run as `mpt` above, medians in milliseconds:

| Completion, branch | 64 MiB, 8 parts | 256 MiB, 16 parts |
|---|--:|--:|
| `upload.json` and every part's record, read together | 12.4 | 19.2 |
| Commit | 15.0 | 19.5 |
| **Answered** | **28.6** | **36.8** |
| Then: delete the staging prefix | 26.2 | 28.6 |

- Waiting for a turn took no time: no two completions of one upload met.
- Each part's read of `upload.json` now runs while its body is read, and took 50 ms at the median
  (it shares the connections with the part's shard uploads). The two together took 155 ms for a part
  of 8 MiB, where one after the other they took 169, and the whole part 171 ms against 193.

## Focused runs

A B B A B A A B, `main` against the branch, four times the operations. The bare bucket's own times
moved by up to 2× within each set of runs (its multipart rows most), so voidfs's own times are
given first, with each adjacent pair's
change, and then the ratios to the bare bucket in the same run (medians of four runs each).

**At 12 ms:**

| Scenario | `main` p50 (ms) | Branch p50 | Change | Each pair | Bare p50, `main`'s runs | Bare p50, branch's runs |
|---|--:|--:|--:|---|---|---|
| multipart put 64 MiB × 8 MiB | 343.2 | 254.3 | −25.9% | −29% −32% −5% −21% | 190, 222, 443, 549 | 263, 174, 495, 255 |
| multipart put 256 MiB × 16 MiB | 1,147.1 | 1,007.9 | −12.1% | −11% −18% −9% −2% | 1,003, 1,141, 2,383, 1,719 | 1,033, 1,006, 2,145, 2,073 |
| put 64 MiB | 239.9 | 217.2 | −9.4% | −3% −13% −2% −6% | 164, 199, 289, 259 | 154, 153, 256, 214 |
| put 32 MiB | 114.6 | 109.9 | −4.1% | +1% −6% −8% +0% | 82, 126, 124, 128 | 84, 83, 101, 125 |

| Scenario | `main` / bare | Branch / bare | Change | Each pair | p90, `main` → branch (ms) | SpaceFS |
|---|--:|--:|--:|---|---|--:|
| multipart put 64 MiB × 8 MiB | 1.19× | 0.98× | −17.8% | −49% −14% −15% +71% | 560 → 427 | 2.44× |
| multipart put 256 MiB × 16 MiB | 0.84× | 0.72× | −14.7% | −13% −7% +1% −19% | 3,235 → 2,123 | 1.79× |
| put 64 MiB | 1.06× | 1.19× | +11.7% | +4% +13% +11% +13% | 318 → 268 | 1.13× |
| put 32 MiB | 0.94× | 1.20× | +27.8% | −1% +44% +13% +3% | 177 → 142 | 1.20× |

- The uploads' own times fell in every pair: 89 ms at the median for 64 MiB, 139 ms for 256 MiB, of
  the 128 and 221 ms completion no longer waits.
- Relative to the bare bucket they are noisy, and the controls show by how much: put 64 and 32 MiB,
  which this change does not touch, took 4–9% less time in the branch's runs and still look 12–28%
  slower relative to the bare bucket, because its own times differed between the runs.
- The multipart rows were ahead of SpaceFS's ratios in both builds, in every run.

**On loopback**, four times the operations:

| Scenario | `main` p50 (ms) | Branch p50 | Change | Each pair | `main` / bare | Branch / bare |
|---|--:|--:|--:|---|--:|--:|
| multipart put 64 MiB × 8 MiB | 248.5 | 255.8 | +2.9% | +4% −9% +15% +1% | 0.50× | 0.52× |
| multipart put 256 MiB × 16 MiB | 1,078.0 | 1,075.4 | −0.2% | +5% +5% −5% +4% | 0.48× | 0.57× |
| put 64 MiB | 235.4 | 242.1 | +2.8% | −17% −16% +13% +19% | 0.98× | 1.11× |
| put 32 MiB | 113.6 | 115.7 | +1.9% | −24% −19% +2% +17% | 0.92× | 0.98× |

On loopback a round trip to the bucket takes well under a millisecond, so there was little for
this change to save, and nothing moved beyond the spread. The bare bucket's multipart times
doubled after the first two runs (256 MiB: 1,082 and 1,118 ms, then 1,745–2,383).

## One binary, switched

`diag` against itself at 12 ms, A B B A B A A B, with `VOIDFS_DIAG_MP_OLD` set (`main`'s path) and
not (the branch's), four times the operations. One LIST per upload with the switch set, none
without, as in `main` and the branch:

| Scenario | `main`'s path p50 (ms) | The branch's | Change | Each pair | Relative to bare | Each pair |
|---|--:|--:|--:|---|--:|---|
| multipart put 64 MiB × 8 MiB | 341.4 | 264.3 | −22.6% | −26% +1% −16% −28% | −24.6% | −47% −17% −12% −35% |
| multipart put 256 MiB × 16 MiB | 1,101.3 | 1,074.4 | −2.4% | −20% +23% +2% −7% | +3.5% | +16% +10% −2% −11% |
| put 64 MiB | 227.6 | 217.3 | −4.5% | −12% −1% −9% −5% | +18.2% | −7% +38% +11% +17% |
| put 32 MiB | 118.6 | 113.8 | −4.1% | −8% +8% −12% +2% | +4.7% | −1% +8% +3% +6% |

- In one binary, multipart put 64 MiB took 23% less time with the branch's path, as `main` against
  the branch showed.
- Multipart put 256 MiB gained 2% here, against 12% between the builds. In these runs the bare
  bucket's 256 MiB uploads took 1,601–2,184 ms throughout, where in the first focused set they took
  1,003–1,141 in the first runs: versitygw, on the same disk, was in its slow state. With eight
  uploads of 256 MiB at once, the disk is then what an upload waits for, and the round trips
  completion no longer waits are time other uploads' parts used anyway. In the full runs at 12 ms,
  with the bare bucket at 625–1,127 ms, it took 903–940 ms against 1,146–1,153.
- The puts, whose code is the same with the switch on and off, moved by as much as they did
  between `main` and the branch: 4–5% less time, and 5–18% slower relative to the bare bucket.

## The 49 rows at 12 ms

`rtt12-main-1`, `rtt12-branch-1`, `rtt12-branch-2`, `rtt12-main-2`, compared in the same position
(`main-1` with `branch-2`, `branch-1` with `main-2`): relative to the bare bucket in the same run,
the geometric mean of the p50 ratios over the 49 rows is **0.967** (median 0.974). By family: edits
0.980, metadata 0.955, reads 0.988, writes 0.927.

| Run | Rows at or ahead of SpaceFS's ratio (edits) | Rows faster than the bare bucket | Geometric mean speed-up over bare |
|---|--:|--:|--:|
| `rtt12-main-1` | 27 (11) | 30 | 2.9× |
| `rtt12-branch-1` | 29 (13) | 31 | 3.0× |
| `rtt12-branch-2` | 29 (12) | 31 | 3.0× |
| `rtt12-main-2` | 25 (9) | 30 | 2.9× |

| Scenario | voidfs p50, `main` (ms) | Branch | Bare | Ratio, `main` → branch | SpaceFS |
|---|---|---|---|---|--:|
| multipart put 64 MiB × 8 MiB | 414.6, 447.5 | 216.7, 342.6 | 228–294 | 1.45, 1.70 → 0.74, 1.50 | 2.44 |
| multipart put 256 MiB × 16 MiB | 1,153.4, 1,145.8 | 940.0, 903.1 | 625–1,127 | 1.58, 1.25 → 1.50, 0.80 | 1.79 |

- Both multipart rows took less time in both branch runs; relative to the bare bucket they moved
  −33% and −22%, with one position each going the other way, from the bare bucket's own times.
- The count of rows ahead of SpaceFS's ratio, 29 against 25–27, is the spread of the edits in 1 MiB
  files, which sit near SpaceFS's ratio: the multipart rows were ahead in all four runs.
- LIST per upload went from 1 to 0. GETs per upload stayed at 17 and 33, and PUTs at 42 and 138.
  `delete_prefix` per upload reads 0.86–0.94 where `main` reads 1.00: the deletes of the last
  uploads in a round land after the round's count is taken.
- Rows that moved 5% or more the same way in both positions, and touch none of this code: faster,
  list 200 keys −10.4%, insert at the start of 32 MiB −10.9%, append to 64 MiB −9.3%, put 32 MiB
  −9.0%, patch in 1 MiB −8.4%, delete at the start of 64 MiB −6.7%; slower, fan-out get 1,000 ×
  4 KiB at 32 +7.9% (+11.0% / +4.9%), delete 4 KiB at the start of 32 MiB +6.7% (+11.1% / +2.4%),
  delete 4 KiB in the middle of 64 MiB +5.8% (+6.9% / +4.8%). The slower ones were run again
  (below).

## On loopback

Two sets, A B B A then B A A B, compared by position over all eight runs: the geometric mean of the
p50 ratios is **1.015** relative to the bare bucket (median 1.022). By family: edits 1.020, metadata
1.018, reads 0.986, writes 1.031.

| Runs | Rows at or ahead of SpaceFS's ratio (edits) | Rows faster than the bare bucket | Geometric mean speed-up over bare |
|---|--:|--:|--:|
| `loopback-main-1` to `-4` | 35–36 (21–22) | 31 | 3.4–3.8× |
| `loopback-branch-1` to `-4` | 34–36 (20–22) | 30–32 | 3.5× |

- The multipart rows look 12% and 17% slower relative to the bare bucket (+26% and +41% in one
  position, 0% and −2% in the other): in `loopback-main-4` the bare bucket stalled in both rows
  (1,146 and 2,125 ms, against 254–312 and 1,034–1,345 in the other runs). voidfs's own times were
  199–206 ms for 64 MiB against `main`'s 218–228, and 733–968 for 256 MiB against 714–920.
- Rows that looked slower in both positions, all touching none of this code: insert 4 KiB at the
  start of 64 MiB +15.4%, delete 4 KiB at the start of 32 MiB +19.7%, and the edits in 1 MiB files
  and overwrite 1 MiB, +7–9% (a few tenths of a millisecond on 4–7 ms). They were run again
  (below).

## The rows that looked slower

Focused again, A B B A B A A B, relative to the bare bucket: `main` against the branch, then `diag`
switched (`main`'s multipart path against the branch's, in one binary). None of these rows runs
the code this change touches.

| Scenario | Full runs | `main` → branch | Each pair | `diag` switched | Each pair |
|---|--:|--:|---|--:|---|
| **At 12 ms**, four times the operations | | | | | |
| fan-out get 1,000 × 4 KiB, 32 at once | +7.9% | −4.7% | −10% −2% −2% −7% | −1.8% | −1% −1% −7% −1% |
| delete 4 KiB, start of 32 MiB | +6.7% | +0.1% | +0% +6% −2% +9% | +0.6% | +8% +0% +3% −10% |
| delete 4 KiB, middle of 64 MiB | +5.8% | −2.3% | −2% +11% −18% −3% | +1.5% | +4% −13% +12% −3% |
| write at 4 KiB in 64 MiB | +5.3% | −2.5% | +2% −6% −6% +2% | −1.0% | −5% +2% +1% +3% |
| overwrite 4 KiB | +4.9% | +0.8% | +2% +1% −4% +0% | −0.5% | −2% +2% −3% +2% |
| **On loopback**, eight times the operations | | | | | |
| delete 4 KiB, start of 32 MiB | +19.7% | −0.1% | +5% +1% +16% −9% | +2.0% | +4% −8% −1% +93% |
| insert 4 KiB, start of 64 MiB | +15.4% | +6.2% | +13% −14% +71% −49% | +9.1% | +41% −20% +15% −36% |
| write at 4 KiB in 1 MiB | +8.9% | −2.6% | −2% −0% −3% −6% | +1.7% | +0% −2% +3% +8% |
| insert 4 KiB, middle of 1 MiB | +8.7% | −4.2% | −4% −7% −5% −1% | +2.6% | +8% −3% +171% −4% |
| append 4 KiB to 1 MiB | +8.6% | +0.5% | +7% +4% −3% −4% | +7.9% | +26% −62% +29% −1% |
| overwrite 1 MiB | +7.4% | −1.7% | −3% −1% −2% +1% | −0.2% | −2% −1% −0% +76% |

- At 12 ms none of them is slower when run on its own: −4.7% to +0.8%, and the pairs disagree.
- On loopback, insert at the start of 64 MiB moved +6.2% with pairs from −49% to +71%; the same
  binary, switched where these rows never go, moved it +9.1%, and append to 1 MiB +7.9%. Those
  moves are what the same code does from run to run here.
- The `diag` runs on loopback had stalls: p90s up to 32.7 ms where the medians are 5–9 ms (the
  +171% and +93% pairs).

## What is left

- **The GETs.** An upload of n parts still reads `upload.json` n + 1 times and each part's record
  once: 17 GETs for 64 MiB, 33 for 256 MiB. They are now off the path (a part reads `upload.json`
  while its body arrives; completion reads everything in one round trip), but each is a request
  the bucket bills. Keeping the part list in one record would remove most, and is a format change
  (an RFC first). Keeping `upload.json` in memory would remove the parts' reads, but would miss
  an abort by another server or by `voidfs-server gc`, after which parts would be written where
  nothing deletes them.
- **One server's claim.** Two servers completing one upload at once can still both commit, as
  before, and another server sees a completed upload as open until its staging records are
  deleted. Closing that across servers needs the upload's id in the commit, a format change.
- **Orphan part records.** A part whose record lands after its upload's staging prefix was deleted
  leaves the record behind, as before, though now only when it lands within one round trip of the
  completion. GC never aborts a staging prefix without `upload.json`, so such records stay roots.
- **Taking in the parts** is most of an upload's time now: at 12 ms, with 64 parts in flight, a
  part of 8 MiB took 171 ms at the median, 155 of them reading its body and uploading its shards.

## Caveats

- One machine: the harness, versitygw, the relay and the server share 15 CPUs and one disk. The
  bare bucket's large writes, multipart ones most, slowed down by up to 2× partway through each set
  of runs and stayed slow, so this compares voidfs's own times as well as ratios, and pairs runs
  that sat next to each other.
- The relay adds its delay each way with a timer, about 14–15 ms a round trip in all.
- The timing builds printed a line per request; the figures from them are for where the time goes,
  not for comparing builds.
- The bare bucket's large transfers are fast here (no bandwidth limit), so the multipart and large
  put rows are understated for the bare bucket; only SpaceFS's real setup judges them.
- SpaceFS's figures are from their cloud run, not this setup; the comparison is of each row's ratio
  to the bare bucket.
