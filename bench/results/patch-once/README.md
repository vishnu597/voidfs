# Patch: each touched shard chunked once

*30 September 2026, one Mac (Apple M5 Pro, 15 CPUs, macOS 27). versitygw 1.8.0 as the bucket, 12 ms
away through `BENCH_ONE_WAY_MS=4 bench/scripts/local.sh` and on loopback, with voidfs-server's
512 MiB cache. Step 3, item 4 of the [parity plan](../../../docs/PARITY.md#7-step-by-step-plan)
([step-3-performance.md](../../../docs/step-3-performance.md#item-4-patch-rewrite-each-touched-shard-once)):
a patch applied its edits one at a time, each re-chunking and re-hashing the shard it landed in,
so sixteen edits in a 1 MiB file hashed about 16 MiB. Now the edits that share a shard are applied
to it together, and it is chunked and hashed once.*

| Files | voidfs-server |
|---|---|
| `rtt12-main-*`, `loopback-main-*` | `main` at `10feda4` |
| `rtt12-branch-*`, `loopback-branch-*` | This branch |

Every run recorded voidfs's own requests to the bucket (`BENCH_BUCKET_REQUESTS=1`), and the
server's metrics were scraped every 2 seconds. Each binary was built with `cargo build --release
-p voidfs-server`, in a target directory of its own (`main` in a separate worktree), and passed
with `BENCH_SERVER_BIN`; their hashes differ, and only the branch's has `every group is cut` in it
(`strings`). The harness ran from the `main` worktree with `BENCH_POOL_FEATURES=inline-data`, so
every pool listed `inline-data`. Every file's `commit` label says `10feda4`; the file names say
which server ran.

The focused runs' files are not kept; the tables below have their figures. One diagnostic build,
not in the branch, was used:

| Name | voidfs-server |
|---|---|
| `diag` | The branch, with `VOIDFS_DIAG_EDIT_BY_EDIT` set turning `apply_edits` back into `main`'s edit-by-edit loop |

Run order:
1. A CPU profile of `main` (`sample`) during patch 16 × 4 KiB in 1 MiB on loopback.
2. In process, in a release build: the grouped patch against edit by edit, for CPU and for
   whether the extents come out the same (a throwaway test, not kept).
3. Focused, A B B A B A A B: the three patch rows, and write at 4 KiB and append 4 KiB in 1 MiB as
   controls. On loopback at eight times the operations, at 12 ms at four times.
4. The full 49 rows at 12 ms (A B B A), and on loopback (A B B A, then B A A B).
5. Focused again, the same way, on the rows the full runs showed slower: nine at 12 ms, nine on
   loopback.
6. `diag` against itself, switched on and off, A B B A B A A B, at 12 ms and on loopback.

No run had errors.

## Where the time went

A 1 MiB file is one shard or two (the minimum is 256 KiB and the average 2 MiB), so each of a
patch's 16 edits re-chunked and re-hashed about the whole file. Sampled every millisecond through
a focused run on loopback (1,024 patches at eight times the operations), the server's busy samples
were almost all in `compute`, which calls `apply_edits`: 8,414 of about 8,530. At the top of the
stack, SHA-256 had 4,204, FastCDC's `cut_gear` 3,677, and `memmove` 429; nothing else had more
than 150, sending to the bucket (`writev`, 137) included. The shards a patch reads were already
fetched in one go before it (`run_edit` asks `needed_shards` for every edit's range), and only
the shards the result uses were uploaded.

## What changed

`content::apply_edits` ([content.rs](../../../crates/voidfs-core/src/content.rs)):
- **Each edit's reach** is its range widened to the whole shards and data extents it touches. In a
  zero run only its own bytes count: zeros are cut, not re-chunked, as `splice` does.
- **Edits whose reaches share a byte are one group.** Its bytes are read once, every edit in it is
  applied in order (later ones win), and it is chunked once. Reaches that only meet at a boundary
  stay apart, as they would edit by edit: each is chunked up to its own end, and the boundary
  stays.
- **At a group's edges, what `splice` does:** a group shorter than the minimum absorbs the extent
  before it (a shard, a data extent, or the last new shard of the group before); while the last
  new shard is short, it pulls in up to two following extents, or the next group whole, which
  may then pull in two of its own. A pull re-cuts only from the start of the short last shard:
  a cut before it depends only on the bytes up to it.
- **Each new shard is hashed once,** at the end. Intermediate shards are never made.
- The shards this reads are all among those `needed_shards` names for each edit's range, which
  `run_edit` fetches in one go before it, so nothing changes in the server.

Tests in `content.rs`, each seen to fail with the code it guards broken (applying edits in reverse,
no absorbing, no pulls, touching groups merged, pulls not reset after a group, one pull only, a
reach not widened to its shard, a previous group's shard not absorbed, a wrong re-cut, one pull
fewer in `needed_shards`, no zeros past the end):
- properties: a patch holds what its edits wrote, over shards, data extents, zeros and past the
  end; a patch of one edit is that `write_at`, extent for extent (writes aimed at extents' edges,
  where those rules act); edits whose `needed_shards` neighbourhoods do not overlap give exactly
  the extents edit by edit gives. Patches are also mixed into the two existing edit properties;
- cases: overlapping edits, a patch within one shard (chunked once from its start, which edit by
  edit is not), edits in neighbouring shards, in a zero run, past the end, and on data extents;
  and a short group that pulls in the next group and what follows it.
- The test sources handed out each shard once (`remove`), so content holding the same shard twice
  failed a read. The new strategy found it; it fails the same way on `main`. They now hand them out
  as often as asked.

## Where the result differs from edit by edit

The bytes and the size are the same. The extents, and so the ETag (format §7.7, derived from the
extent list), are the same when no edit's neighbourhood overlaps another's, and in every in-file
patch tried. They can differ when writes land beside each other in a zero run, because edit by
edit the result depends on the order in which they come:

| Patch, default chunking (256 KiB / 2 MiB / 16 MiB) | Extents differ | CPU edit by edit | Grouped |
|---|--:|--:|--:|
| The benchmark's: 16 × 4 KiB, one per sixteenth of 1 MiB | 0 of 400 | 8.9 ms | 0.59 ms |
| The same in 32 MiB | 0 of 30 | 32.4 ms | 20.2 ms |
| The same in 64 MiB | 0 of 15 | 32.2 ms | 29.4 ms |
| 16 × 4 KiB anywhere in 8 MiB | 0 of 60 | 28.4 ms | 5.1 ms |
| 64 adjacent 4 KiB pages in 8 MiB, in shuffled order | 0 of 60 | 128.9 ms | 2.0 ms |
| 16 × 64 KiB pages into an empty file, shuffled | 7 of 200 | 0.56 ms | 0.28 ms |
| 8 × 512 KiB pages into an empty file, shuffled | 5 of 100 | 2.0 ms | 1.6 ms |
| 8 × 300 KiB pages into an empty file, shuffled | 1 of 100 | 0.88 ms | 0.72 ms |

For example, eight 512 KiB pages written into an empty file in the order 3 MiB, 512 KiB, 7 MiB,
4.5 MiB, 3.5 MiB, 6 MiB, 1 MiB, 7.5 MiB. Edit by edit, the page at 3 MiB is chunked while zeros
still follow it: a cut at 299,057 bytes leaves a 225,231-byte shard that has nothing to pull in.
The page at 3.5 MiB comes later and is a shard of its own. Grouped, the short shard pulls in that
page:

| | Shards from 3 MiB to 4 MiB |
|---|---|
| Edit by edit | 299,057 + 225,231 + 524,288 bytes |
| Grouped | 299,057 + 749,519 bytes |

In all 13 patches that differed, the grouped result had one shard fewer, and no more shards below
the minimum.

## Patch in 1 MiB takes what one edit does

Focused, A B B A B A A B, relative to the bare bucket in the same run (the median of four runs
each), with each pair's change:

**At 12 ms**, four times the operations:

| Scenario | `main` | Branch | Change | Each pair | SpaceFS | p50 (ms) | p90 (ms) | Log entries per op |
|---|--:|--:|--:|---|--:|---|---|---|
| patch 16 × 4 KiB in 1 MiB | 1.71× | 1.34× | −21.6% | −24% −24% −21% −15% | 1.45× | 45.1 → 34.6 | 53.4 → 40.1 | 0.467 → 0.366 |
| patch 16 × 4 KiB in 32 MiB | 0.81× | 0.69× | −14.9% | −22% −12% −15% −14% | 0.70× | 97.3 → 84.8 | 123.1 → 114.9 | 0.668 → 0.621 |
| patch 16 × 4 KiB in 64 MiB | 0.51× | 0.51× | −1.0% | −5% +3% −4% −1% | 0.47× | 131.8 → 128.4 | 189.2 → 178.0 | 0.682 → 0.693 |
| write at 4 KiB in 1 MiB | 1.43× | 1.35× | −5.3% | −7% +2% −8% +2% | 1.55× | 37.3 → 35.1 | 42.1 → 40.1 | 0.356 → 0.366 |
| append 4 KiB to 1 MiB | 1.39× | 1.35× | −3.4% | −3% +1% −2% −3% | 0.98× | 37.0 → 35.3 | 41.3 → 40.2 | 0.348 → 0.370 |

- Patch in 1 MiB and in 32 MiB are at or ahead of SpaceFS's ratio in all four branch runs, and in
  none of `main`'s: 1.31–1.43× against SpaceFS's 1.45×, and 0.65–0.69× against 0.70×.
- The time a patch saves is the CPU it no longer spends: 10.5 ms in 1 MiB and 12.5 ms in 32 MiB,
  where the in-process figures above say 8.3 and 12.2.
- Patch in 64 MiB saves little: its 16 edits, 4 MiB apart, rarely share a shard of about 2 MiB,
  so each shard is still chunked once, as before. What it spends now is below.

**On loopback**, eight times the operations:

| Scenario | `main` | Branch | Change | Each pair | p50 (ms) | p90 (ms) |
|---|--:|--:|--:|---|---|---|
| patch 16 × 4 KiB in 1 MiB | 4.46× | 1.33× | −70.2% | −71% −73% −69% −70% | 18.2 → 5.1 | 24.2 → 6.6 |
| patch 16 × 4 KiB in 32 MiB | 0.96× | 0.95× | −0.9% | −14% −15% +21% +31% | 78.2 → 78.3 | 107.5 → 115.3 |
| patch 16 × 4 KiB in 64 MiB | 0.64× | 0.61× | −4.1% | −2% −6% −22% +32% | 110.4 → 100.3 | 183.8 → 146.5 |
| write at 4 KiB in 1 MiB | 1.28× | 1.28× | +0.3% | +2% −1% +14% −8% | 5.0 → 5.0 | 6.6 → 6.2 |
| append 4 KiB to 1 MiB | 1.23× | 1.25× | +1.3% | +1% −10% −34% +11% | 5.7 → 5.6 | 7.6 → 7.5 |

Patch in 1 MiB now takes 5.1 ms, as a single write in the same file does (5.0), where it took
3.6 times as long; SpaceFS's ratio is 1.45×. The 32 and 64 MiB rows, which upload 12–15 shards
to a local disk, move by a quarter between identical runs here.

## The 49 rows at 12 ms

`rtt12-main-1`, `rtt12-branch-1`, `rtt12-branch-2`, `rtt12-main-2`, compared in the same position
(`main-1` with `branch-2`, `branch-1` with `main-2`): relative to the bare bucket in the same run,
the geometric mean of the p50 ratios over the 49 rows is **0.987** (median 0.987). By family: edits
0.992, metadata 0.937, reads 0.990, writes 0.991.

| Run | Rows at or ahead of SpaceFS's ratio (edits) | Rows faster than the bare bucket | Geometric mean speed-up over bare |
|---|--:|--:|--:|
| `rtt12-main-1` | 26 (10) | 30 | 2.8× |
| `rtt12-branch-1` | 30 (14) | 30 | 2.9× |
| `rtt12-branch-2` | 25 (9) | 31 | 3.0× |
| `rtt12-main-2` | 27 (11) | 31 | 3.0× |

| Scenario | Change relative to bare (odd / even) | Ratio, `main` → branch | SpaceFS |
|---|--:|---|--:|
| patch 16 × 4 KiB in 1 MiB | −15.6% (−6.3% / −24.0%) | 1.58, 1.71 → 1.30, 1.48 | 1.45 |
| patch 16 × 4 KiB in 32 MiB | −11.5% (−22.1% / +0.7%) | 0.86, 0.69 → 0.69, 0.67 | 0.70 |
| patch 16 × 4 KiB in 64 MiB | −10.4% (−9.6% / −11.1%) | 0.57, 0.49 → 0.43, 0.52 | 0.47 |

- The patch rows took 35.8–37.7, 95.8–100.5 and 122.3–139.2 ms, against `main`'s 44.9–45.0,
  97.0–116.4 and 141.0–141.3.
- `rtt12-branch-2` ran with a fast bare bucket in the 32 and 64 MiB rows (insert 4 KiB at the
  start of 32 MiB: 126.6 ms, against 144–154 in the other runs), which cost it the rows whose ratio
  sits near SpaceFS's; voidfs's own times there were `main`'s. Patch in 1 MiB, at 1.48×, was just
  behind SpaceFS's 1.45× in that run.
- Other rows that moved 5% or more, the same way in both positions: head −17.2%, put 64 MiB
  −6.7%, truncate at the end of 1 MiB −6.6%; and slower, write at 4 KiB in 64 MiB +8.3% and in
  32 MiB +7.4%, insert in the middle of 32 MiB +10.0% and at its start +13.3%, delete in the
  middle of 64 MiB +6.6%, get 32 MiB +6.5%, append to 64 MiB +5.8%, insert at the start of 1 MiB
  +5.4%. None of them patches; see below.

## On loopback

Two sets, A B B A then B A A B, compared by position over all eight runs: the geometric mean of the
p50 ratios is **0.967** relative to the bare bucket (median 0.991). By family: edits 0.939,
metadata 0.965, reads 0.997, writes 1.004. Patch in 1 MiB: −72.8% (−72.1% / −73.4%).

| Runs | Rows at or ahead of SpaceFS's ratio (edits) | Rows faster than the bare bucket | Geometric mean speed-up over bare |
|---|--:|--:|--:|
| `loopback-main-1` to `-4` | 34–37 (20–22) | 31–32 | 3.3–3.6× |
| `loopback-branch-1` to `-4` | 34–37 (21–22) | 31–32 | 3.4–3.6× |

Rows that looked slower in both positions: insert in the middle of 64 MiB +29.1%, append to
64 MiB +23.7%, truncate at the end of 64 MiB +22.1%, write at 4 KiB in 32 MiB +12.9%, put 32 MiB
+7.2%, get 64 MiB +5.8%. The edits inside 32 and 64 MiB files run 32 operations a round on
loopback and vary by 2× between identical runs, as before.

## The rows that looked slower

Focused again, A B B A B A A B, `main` against the branch:

| Scenario | Full runs | Focused | Each pair |
|---|--:|--:|---|
| **At 12 ms**, four times the operations | | | |
| insert 4 KiB, middle of 32 MiB | +10.0% | +7.7% | +18% +10% +2% −7% |
| write at 4 KiB in 64 MiB | +8.3% | +5.2% | +12% +5% −0% −9% |
| write at 4 KiB in 32 MiB | +7.4% | +2.0% | −8% +1% +2% +16% |
| delete 4 KiB, middle of 64 MiB | +6.6% | +1.9% | +29% −2% −4% −10% |
| get 32 MiB | +6.5% | +1.5% | −7% +5% +0% +6% |
| insert 4 KiB, start of 32 MiB | +13.3% | +0.8% | +21% −9% +15% −12% |
| insert 4 KiB, start of 1 MiB | +5.4% | +0.5% | −4% +3% +4% −1% |
| insert 4 KiB, middle of 1 MiB | +4.9% | +0.0% | −1% +2% +4% −4% |
| append 4 KiB to 64 MiB | +5.8% | −1.1% | −5% −20% +4% +0% |
| **On loopback**, eight times the operations | | | |
| append 4 KiB to 64 MiB | +23.7% | +24.5% | +30% +23% +82% −34% |
| truncate 4 KiB, end of 64 MiB | +22.1% | +21.2% | +36% +45% −17% +11% |
| insert 4 KiB, start of 1 MiB | +3.2% | +4.7% | +11% −1% +14% −9% |
| write at 4 KiB in 1 MiB | +3.5% | +4.0% | +1% +0% −3% +15% |
| put 32 MiB | +7.2% | +3.8% | +2% +13% −12% −0% |
| write at 4 KiB in 32 MiB | +12.9% | +3.5% | +4% −3% −31% +83% |
| get 64 MiB | +5.8% | −1.7% | +6% −1% −5% −2% |
| insert 4 KiB, start of 32 MiB | +17.9% | −27.3% | −8% −10% −59% −37% |
| insert 4 KiB, middle of 64 MiB | +29.1% | −35.7% | −30% −31% −10% −48% |

None of these rows patches: they are writes, splices and gets, whose code is `main`'s. The one
change on their path is that `needed_shards` takes its range of extents from a function of its
own, which computes what it did.

`diag` against itself, the switch on (edit by edit) and off (grouped), A B B A B A A B, shows
both what the change does in one binary and what the other rows do with no change at all:

| Scenario | Edit by edit → grouped, one binary | Each pair |
|---|--:|---|
| **At 12 ms**, four times the operations | | |
| patch 16 × 4 KiB in 1 MiB | 1.74× → 1.40×, −19.1% | −21% −16% −17% −21% |
| patch 16 × 4 KiB in 32 MiB | 0.75× → 0.68×, −8.7% | −11% −19% −4% −2% |
| patch 16 × 4 KiB in 64 MiB | 0.52× → 0.50×, −3.2% | −1% −5% +6% −5% |
| insert 4 KiB, middle of 32 MiB | −6.3% | −14% −0% −6% −4% |
| write at 4 KiB in 64 MiB | −5.6% | −17% −4% −5% −2% |
| **On loopback**, eight times the operations | | |
| patch 16 × 4 KiB in 1 MiB | 4.55× → 1.32×, −71.0% | −74% −71% −20% −70% |
| patch 16 × 4 KiB in 32 MiB | +0.6% | −23% −15% +40% +8% |
| append 4 KiB to 64 MiB | +38.6% | −16% +17% +62% +41% |
| truncate 4 KiB, end of 64 MiB | +12.6% | +6% −12% +106% +24% |
| write at 4 KiB in 1 MiB | +2.2% | +2% −5% +1% +487% |
| insert 4 KiB, start of 1 MiB | −1.2% | +4% −4% −2% +8% |

- The switch changes only patches, so the other rows' moves here are what the same code does
  from run to run: about 6% at 12 ms, the same way in all four pairs, and 13–39% on loopback for
  the edits at the end of a 64 MiB file, whose 5–7 ms is a small share of the bare bucket's
  170 ms. Moves of the same size between `main` and the branch on these rows are not read as the
  change's.
- Two of the loopback runs (edit by edit's third and fourth) had stalls: the bare bucket's p50 in
  one round was 17.8 and 22.8 ms, against 3.8 in the others, and some p90s 173–232 ms. That is the
  −20% and the +487%.

## What the counters show

Per operation, in the focused runs at 12 ms:

| Scenario | Shard PUTs, `main` → branch | Shard GETs | Log entries |
|---|---|---|---|
| patch 16 × 4 KiB in 1 MiB | 1.16 → 1.14 | 0 → 0 | 0.47 → 0.37 |
| patch 16 × 4 KiB in 32 MiB | 11.51 → 11.20 | 0.01 → 0.01 | 0.67 → 0.62 |
| patch 16 × 4 KiB in 64 MiB | 15.49 → 15.41 | 5.02 → 4.73 | 0.68 → 0.69 |

(PUTs are shards and the few checkpoint pages; log entries are the conditional PUTs.)

- The requests to the bucket did not fall, and were not expected to once the profile was read:
  `main` already fetched every shard a patch needs in one go and uploaded only the shards the
  result keeps. What fell is CPU.
- Patch in 64 MiB reads about five shards back from the bucket per operation in both builds: the
  benchmark's working set of 64 MiB files overflows the 512 MiB cache, as before.
- Fewer log entries per patch in 1 MiB: patches finish sooner, so more of them share an entry.

## What is left

Patch in 64 MiB, the one patch row behind SpaceFS's ratio at 12 ms (0.51× against 0.47×), now
spends its CPU chunking and hashing each touched shard once: about 16 shards of about 2 MiB, 29 ms a
patch in process, one shard after another on the request's task. Hashing the new shards in
parallel, off the async workers, would take most of that off a patch's time when cores are free;
on this Mac the harness, the bucket, the relay and the server share them. Not tried here.

## Caveats

- One machine: the harness, versitygw, the relay and the server share 15 CPUs. The relay adds its
  delay each way with a timer, about 14–15 ms a round trip in all.
- Patches that write pages beside each other in a zero run can come out with other chunk
  boundaries than edit by edit, and so another ETag for the same bytes (above).
- The bare bucket's large transfers are fast here (no bandwidth limit), so the 32 and 64 MiB rows
  are understated for the bare bucket; only SpaceFS's real setup judges them.
- SpaceFS's figures are from their cloud run, not this setup; the comparison is of each row's ratio
  to the bare bucket.
