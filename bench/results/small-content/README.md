# Small files held in the log (RFC 0003)

*29 September 2026, one Mac (Apple M5 Pro, 15 CPUs, macOS 27). versitygw 1.8.0 as the bucket, 12 ms
away through `BENCH_ONE_WAY_MS=4 bench/scripts/local.sh` and on loopback, with voidfs-server's
512 MiB cache. Step 3, item 3 of the [parity plan](../../../docs/PARITY.md#7-step-by-step-plan),
change 1 ([step-3-performance.md](../../../docs/step-3-performance.md#item-3-fewer-sequential-round-trips-per-write)):
[RFC 0003](../../../rfcs/0003-small-content-in-descriptors.md), content of up to 4 KiB held in its
descriptor, so in the log, instead of in a shard.*

| Files | voidfs-server |
|---|---|
| `rtt12-main-*`, `loopback-main-*` | `main` at `8ce1a94` |
| `rtt12-branch-*`, `loopback-branch-*` | This branch, its pools created with `inline-data` |

Every run recorded voidfs's own requests to the bucket (`BENCH_BUCKET_REQUESTS=1`). Each binary was
built with `cargo build --release -p voidfs-server`, in a target directory of its own (`main` in a
separate worktree), and passed with `BENCH_SERVER_BIN`; their hashes differ, and only the branch's
has `inline-data` in it (`strings`). The harness ran from the `main` worktree with
`VOIDFS_NEW_POOL_FEATURES=inline-data` in its environment: the branch creates each run's new pool
with the feature, and `main`'s binary does not read the variable. The branch's pools listed it
(checked in the bucket's directory) and its small writes uploaded no shard. Every file's `commit`
label says `8ce1a94`; the file names say which server ran. (`local.sh` now has `BENCH_POOL_FEATURES`
for this, and labels the pool's features.)

The focused runs' files are not kept; the tables below have their figures. Besides `main` and the
branch, they used diagnostic builds, none of which is in the branch:

| Name | voidfs-server |
|---|---|
| `maindrain` | `main`, with one task draining each drive's queue ([below](#found-on-the-way-the-commit-lock-starved)) |
| `branch0` | The branch before that fix |
| `hold2` | `branch0`, holding a log entry that follows another until the requests the last one answered are back, at most a quarter of the last write and 2 ms ([below](#why-eight-at-once-still-takes-two-round-trips)) |
| `noswap` | The branch, without putting what a checkpoint spilled into the drive's state |

Run order:
1. Focused small writes, `main` against `branch0`, A B B A B A A B at 12 ms and on loopback, and one
   at a time at 12 ms (A B B A).
2. `branch0` alone at 1, 2, 4 and 8 at once; `branch0` against the first hold, and against `hold2`
   (A B B A B A A B, then A B B A).
3. The checkpoint probe, `main` against `branch0`, which found the starving commit lock; the fix,
   and its test.
4. With the fix: focused small writes, `main` against the branch, A B B A B A A B at 12 ms and on
   loopback; `main` against `maindrain` (A B B A, then A B B A B A A B with more rows); one at a
   time (A B B A); the checkpoint probe (A B B A).
5. Memory: `main`, the branch and `noswap`.
6. The full 49 rows at 12 ms (A B B A), and on loopback (A B B A, then B A A B).

No run had errors.

## One at a time, a small write takes one round trip

At 12 ms, `--concurrency 1`, 400 operations a run, `main` against the branch A B B A:

| Scenario | `main` p50 (ms) | Branch p50 (ms) | `main` / bare | Branch / bare |
|---|--:|--:|--:|--:|
| put 4 KiB | 29.0 | 14.9 | 2.02× | 1.04× |
| overwrite 4 KiB | 29.0 | 14.9 | 2.02× | 1.04× |

p99 went from 32.3–33.1 ms to 16.9–17.9, and the slowest from 34.7–35.6 to 17.6–20.6. The bucket
sees only log entries: per put 4 KiB, 0.25 requests (from 1.30: a shard and 0.30 of a log entry).

## Eight to 64 at once

`main` against the branch, A B B A B A A B, four times the operations. p50 relative to the bare
bucket in the same run, the median of four runs each:

| Scenario | 12 ms: `main` | Branch | SpaceFS | Loopback: `main` | Branch | Bucket requests per op, `main` → branch |
|---|--:|--:|--:|--:|--:|---|
| put 4 KiB | 2.37× | 2.12× | 2.01× | 1.98× | 1.15× | 1.30 → 0.25 |
| overwrite 4 KiB | 2.57× | 2.16× | 3.05× | 1.72× | 0.91× | 1.31 → 0.25 |
| fan-out put 1,000 × 4 KiB, 32 at once | 3.05× | 2.24× | 2.23× | 1.70× | 0.65× | 1.08 → 0.06 |
| fan-out put 1,000 × 4 KiB, 64 at once | 3.05× | 2.28× | 2.41× | 1.84× | 0.57× | 1.04 → 0.03 |

- At 12 ms the branch ran 26.5–30.3 ms (p50) where `main` ran 28.1–37.8: about two round trips
  still, not the one the RFC expected. Why is [below](#why-eight-at-once-still-takes-two-round-trips).
- Against SpaceFS's ratio: the fan-out put at 64 is ahead in three runs of four (2.27–2.29×; the
  fourth 2.415× against 2.414×), the one at 32 in one (2.19×; the others 2.24–2.32× against 2.23×).
  Put 4 KiB stays behind (2.07–2.19× against 2.01×). Overwrite was ahead already.
- On loopback all four are well ahead of SpaceFS's ratio, and the fan-out puts are faster than the
  bare bucket: a put there waits for one log entry shared with others, where the bare bucket
  writes each object.

## Why eight at once still takes two round trips

`branch0` alone at 12 ms, put 4 KiB with every latency kept:

| At once | 1 | 2 | 4 | 8 |
|---|--:|--:|--:|--:|
| p50 (ms) | 14.4 | 27.2 | 27.3 | 28.4 |

Two at once is enough. A drive writes one log entry at a time, and group commit puts in the next
one whatever queued while the last was written. When an entry lands, the requests it answered send
their next ones, but the entry after it has already started with what was waiting, so they wait for
that one and then their own: two alternating groups, each request two round trips. Nothing here is
the new write path's; it shows now because the write before the log entry is gone. Renames, which
never had one, show it already: 25–28 ms at 8 at once, two round trips.

`hold2` holds the next entry until the requests the last one answered are back in the queue, at most
a quarter of the last write (2 ms at 12 ms; no hold at all under 1 ms, so not on loopback). Against
`branch0`, A B B A, at 12 ms:

| Scenario | `branch0` / bare | `hold2` / bare |
|---|--:|--:|
| put 4 KiB | 2.07× | 1.14× |
| overwrite 4 KiB | 2.18× | 1.14× |
| fan-out put 1,000 × 4 KiB, 32 at once | 2.37× | 1.32× |
| fan-out put 1,000 × 4 KiB, 64 at once | 2.25× | 1.36× |
| rename 64 MiB | 0.215× (25.8 ms) | 0.104× (12.7 ms) |

With it, eight share each entry (0.13 log entries per put) and the four rows are at the RFC's
expected 1.1–1.4×, well ahead of SpaceFS's ratios. A first try, holding until as many requests
queued as the last entry had, changed nothing: two alternating groups of one each never grow. The
hold is left out of this change because it moves every write's timing, and needs its own full
measurements and a better answer for loopback than none.

## Checkpoints store the small files as shards

The probe of [write-round-trips](../write-round-trips/README.md#checkpoints-in-the-background):
put 4 KiB at 12 ms, 20,000 in one drive, every latency kept and the server's metrics scraped every
second. `main` against the branch, A B B A:

| Run | p50 (ms) | p90 | p99 | p99.9 | Max | Over 60 ms | Checkpoints | Mean write | Spilled shards | Swap (commit lock held) |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| `main-1` | 30.1 | 44.0 | 49.1 | 51.4 | 57.0 | 0 | 5 | 85 ms | – | – |
| `branch-1` | 29.5 | 32.4 | 34.5 | 41.3 | 44.4 | 0 | 6 | 1,252 ms | 17,189 | 13.7 ms mean, all ≤ 25 ms |
| `branch-2` | 29.4 | 32.3 | 34.8 | 42.8 | 46.3 | 0 | 6 | 1,259 ms | 17,196 | 13.6 ms mean, all ≤ 25 ms |
| `main-2` | 29.8 | 43.8 | 48.5 | 51.6 | 64.3 | 6 | 5 | 83 ms | – | – |

- The branch's pool writes a checkpoint every ~2,870 puts: 16 MiB of log, where each put carries
  its 4 KiB as base64. `main` writes one every 1,000 log entries.
- Each spills ~2,870 shards, 32 uploads at a time through the 12 ms relay, which is most of its
  1.25 s, all in the background. The total of PUTs stays what it was, a shard per distinct small
  file, off the request path: 17,230 in a run against `main`'s 20,037 (the last ~2,800 files were
  in the log when the scenario deleted the drive).
- Foreground writes did better than `main`'s at every quantile from p90 up. Their p50 is two round
  trips, [as above](#why-eight-at-once-still-takes-two-round-trips); the earlier probes' figures for
  `main` were 118–135 ms at p99.9 before checkpoints moved off the commit path, and 49–56 after.
- The swap takes each spilled descriptor into the drive's state wherever the state still has what
  was spilled, under the commit lock: 13.6 ms for ~2,870 files, once per checkpoint.

### Found on the way: the commit lock starved

The first probe of `branch0` wrote only two checkpoints in 20,000 puts. The one-second series showed
why: the first was written 12 s in, and its swap finished 31 s later, still holding the checkpoint
lock, so no other could start; the second then spilled 9,436 shards at once.

Every mutation started a task to drain its drive's queue. Each waited for the commit lock in turn
and, when it got it, wrote whatever had queued, so under steady writes the line of tasks grew by
several with every batch. Whatever else waits for that lock waits behind all of them: a test with
eight clients writing steadily and a log write of 10 ms measured 1.5 s, then 9, then 56, then 360.
That is `main`'s too, for forks and hard deletes. One task drains a drive's queue now, started by a
mutation only when none is running, so whatever else waits for the lock waits for one batch.

On its own (`maindrain` against `main`, A B B A B A A B at 12 ms, relative to bare): rename 64 MiB
−2.4%, append and write-at 4 KiB in 1 MiB −1.4% and −1.7%, put 1 MiB +0.3%. The shard-first small
writes moved −2.5% to +6.8% and not the same way by position (overwrite 4 KiB: equal in the first
two pairs, 8–15% slower with the fix in the last two); their log entries per operation rose about
3%, so batches are a little smaller, which in the lockstep above can move p50 either way. Not
resolved.

## Memory

Put 4 KiB on loopback, 100,000 in one drive (50,000 a round), with a 16 MiB shard cache so that the
drive's state dominates; `vmmap --summary`'s malloc ALLOCATED every 5 seconds. Every put has fresh
data.

| Seconds in | `main` | Branch | `noswap` |
|--:|--:|--:|--:|
| ~6 | 0.53 GB | 0.51 | 0.55 |
| ~11 | 1.2 | 1.2 | 1.3 |
| ~17 | 1.7 | 1.7 | 2.0 |
| ~23 | 2.1 | 2.2 | 2.6 |
| Highest | 2.8 | 2.6 | 3.1 |

- The branch holds what `main` does. Without the swap, memory grows about 5 KB a file faster: the
  4 KiB each drive's state would keep in its data extents until the server restarted.
- With the swap, the state holds data extents for the log since the last checkpoint only: at most
  16 MiB of log, about 12 MB of small files.
- Found, not investigated: a drive's state takes about 26 KB per small file, on `main` too.

## The 49 rows at 12 ms

`rtt12-main-1`, `rtt12-branch-1`, `rtt12-branch-2`, `rtt12-main-2`, compared in the same position
(`main-1` with `branch-2`, `branch-1` with `main-2`): relative to the bare bucket in the same run, the
geometric mean of the p50 ratios over the 49 rows is **0.957** (median 0.977). By family: writes
0.883, edits 0.970, metadata 0.954, reads 1.013.

| Run | Rows at or ahead of SpaceFS's ratio (writes) | Rows faster than the bare bucket | Geometric mean speed-up over bare |
|---|--:|--:|--:|
| `rtt12-main-1` | 22 (3) | 30 | 2.63× (both `main` runs) |
| `rtt12-branch-1` | 26 (6) | 30 | 2.75× (both branch runs) |
| `rtt12-branch-2` | 24 (4) | 30 | |
| `rtt12-main-2` | 23 (3) | 30 | |

SpaceFS's geometric mean is 2.8×. Fan-out put 1,000 × 4 KiB at 64 crossed SpaceFS's ratio in both
branch runs, and overwrite 4 KiB was ahead in all four. The rest of the count is rows near the line,
ahead in some runs of either build: fan-out put 200 × 256 KiB, put 32 MiB, the multipart uploads,
and two edits in 32 and 64 MiB files.

Rows that moved 5% or more relative to the bare bucket:

| Scenario | Change relative to bare (odd / even) |
|---|--:|
| multipart put 256 MiB × 16 MiB | −24.8% (−13.5% / −34.7%) |
| fanout put 1000 × 4 KiB, 32 at once | −24.2% (−20.3% / −28.0%) |
| fanout put 1000 × 4 KiB, 64 at once | −23.8% (−23.5% / −24.1%) |
| overwrite 4 KiB | −22.7% (−20.5% / −24.8%) |
| put 4 KiB | −21.2% (−28.3% / −13.4%) |
| append 4 KiB to 64 MiB | −14.8% (+6.1% / −31.6%) |
| insert 4 KiB, middle of 64 MiB | −14.6% (−7.9% / −20.9%) |
| patch 16 × 4 KiB in 32 MiB | −13.4% (−9.9% / −16.7%) |
| write at 4 KiB in 32 MiB | −9.2% (−16.2% / −1.6%) |
| delete 4 KiB, start of 64 MiB | −9.0% (−13.8% / −4.0%) |
| write at 4 KiB in 64 MiB | −6.6% (−3.8% / −9.2%) |
| move dir 200 × 64 KiB | −6.4% (−1.5% / −11.0%) |
| append 4 KiB to 32 MiB | −5.6% (+8.0% / −17.4%) |
| patch 16 × 4 KiB in 64 MiB | −5.4% (+13.4% / −21.1%) |
| fanout get 1000 × 4 KiB, 64 at once | −5.4% (−7.7% / −3.1%) |
| put 32 MiB | −5.2% (−0.7% / −9.4%) |
| insert 4 KiB, middle of 1 MiB | −5.0% (−2.2% / −7.7%) |
| insert 4 KiB, start of 1 MiB | +6.2% (+6.2% / +6.3%) |
| delete 4 KiB, start of 32 MiB | +7.8% (+13.6% / +2.3%) |
| get 1 MiB | +15.8% (+16.1% / +15.5%) |

- The four small writes moved as in the focused runs. Multipart put 256 MiB moves by itself, mostly
  with the bare bucket's own time; the edits inside 32 and 64 MiB files move 6–15% between
  identical builds here.
- The three rows that looked slower had focused runs, A B B A B A A B at four times the operations:
  get 1 MiB +0.8% relative to bare (0.7 ms in both builds), insert 4 KiB at the start of 1 MiB
  −0.3%, delete 4 KiB at the start of 32 MiB +5.7% (+1.1% raw; the bare bucket moved). Get 1 MiB
  read 1.13–1.18 ms in both branch runs of the full sequence against `main`'s 0.93–0.97, and 16–17%
  slower relative to bare on loopback too; the scenarios before it are edits in 32 MiB files, which
  the branch changes in nothing that shows here. Not explained, and not seen alone.

## On loopback

Two sets, A B B A then B A A B, compared by position over all eight runs: the geometric mean of the
p50 ratios is **0.938** relative to the bare bucket (median 0.993), 0.942 raw. By family, relative to
bare: writes 0.756, edits 0.996, metadata 0.977, reads 1.012.

| Runs | Rows at or ahead of SpaceFS's ratio | Rows faster than the bare bucket | Geometric mean speed-up over bare |
|---|--:|--:|--:|
| `loopback-main-1` to `-4` | 34–36 (writes 8–10) | 28 | 3.24× |
| `loopback-branch-1` to `-4` | 34–35 (writes 9–10) | 31–32 | 3.42× |

On loopback the small writes were at SpaceFS's ratio already, so the count moves little; what moved
is how far ahead they are, and three of them (the fan-out puts and overwrite 4 KiB) are now faster
than the bare bucket. Rows that moved 10% or more relative to bare: the fan-out puts −69% and −60%,
put and overwrite 4 KiB −41% and −39%, insert 4 KiB at the start of 32 MiB −18%, truncate at the end
of 64 MiB −16% (+26% / −44%), delete at the start of 32 MiB −14%, listing −10%; and get 1 MiB +17%,
append to 32 MiB +21% (+54% / −5%), write at 4 KiB in 64 MiB +17% (−11% / +54%), append to 64 MiB
+27%. The edits inside 32 and 64 MiB files have 32 operations a round on loopback and vary by
2.5× between identical runs.

## What the counters show

Every run counted voidfs's requests to the bucket. Per operation, over both 12 ms runs of each build:

| Scenario | `main` | Branch |
|---|--:|--:|
| put 4 KiB | 1.00 shard PUT, 0.31 log entry | 0.25 log entry |
| overwrite 4 KiB | 1.00, 0.32 | 0.25 |
| fanout put 1,000 × 4 KiB, 32 at once | 1.00, 0.08 | 0.06 |
| fanout put 1,000 × 4 KiB, 64 at once | 1.00, 0.04 | 0.03 |

Every other row makes the same requests within what group commit's batching varies. The findings
from earlier stand: patch 16 × 4 KiB in 64 MiB reads 5.0–5.5 shards back per operation, because
eight 64 MiB working sets overflow the 512 MiB cache (item 4, or the cache), and a multipart upload
reads its part records one after another when it completes (item 5); this change touches neither.

## Caveats

- One machine: the harness, versitygw, the relay and the server share 15 CPUs. The relay adds its
  delay each way with a timer, about 14–15 ms a round trip in all.
- SpaceFS's figures are from their cloud run, not this setup; the comparison is of each row's ratio
  to the bare bucket.
- The small-write rows at 8 at once and more fall into step with group commit, so their p50 moves a
  few percent between identical runs, and by position; each figure above is a median of four.
