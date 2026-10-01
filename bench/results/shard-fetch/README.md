# Coalesced shard fetches, and read-ahead from a shared budget

*1 October 2026, one Mac (Apple M5 Pro, 15 CPUs, macOS 27). versitygw 1.8.0 as the bucket, 12 ms
away through `BENCH_ONE_WAY_MS=4 bench/scripts/local.sh`, with voidfs-server's 512 MiB cache.
Step 3, item 6 of the [parity plan](../../../docs/PARITY.md#7-step-by-step-plan)
([step-3-performance.md](../../../docs/step-3-performance.md#item-6-the-read-path-for-cold-and-large-reads)):
cold large reads were behind SpaceFS's cache-cleared figures with S3's bandwidth emulated, because
the eight readers of a cold object each fetched every shard of it
([cold-reads](../cold-reads/README.md)). Now concurrent fetches of a shard are coalesced, and a GET
reads further ahead when the pool has room.*

| Files | voidfs-server | Bucket |
|---|---|---|
| `cold-s3-{main,branch}-{1..4}` | `main` at `12b68e9`, and this branch | 12 ms, `BENCH_BANDWIDTH=s3`, cold: the ten read rows, A B B A B A A B |
| `cold-pertotal-*` | The same | 12 ms, `BENCH_BANDWIDTH=95/68/-` (the cap without its total), cold, the same |
| `cold-nocap-*` | The same | 12 ms, no cap, cold, the same |
| `warm12-*` | The same | 12 ms, no cap, warm: all 49 rows, A B B A |
| `capped-branch-1`, `-2` | This branch | 12 ms, `BENCH_BANDWIDTH=s3`, warm, all 49 rows |
| `warmconc-*` | The same, on loopback | No delay or cap, warm: get 64 MiB at 1, 2, 4 and 8 at once (`--concurrency`), against versitygw |

Each server binary was built with `cargo build --release -p voidfs-server`, in a target directory of
its own (`main` in a `git worktree` of `origin/main`), and passed with `BENCH_SERVER_BIN`; their
hashes differ (`main` 62b8ac5d, the branch 9c4ae46f, which the branch's code builds to again), and
only the branch's has `voidfs_cache_coalesced_total` in it (`strings`). The harness ran from the
worktree of `main` (it did not change), with `BENCH_POOL_FEATURES=inline-data` and
`BENCH_BUCKET_REQUESTS=1`, and the server's metrics were scraped every 2 seconds. A diagnostic build
of the branch, whose read-ahead an environment variable sets (`VOIDFS_DIAG_READ_AHEAD`,
`VOIDFS_DIAG_READ_AHEAD_SHARED`; nothing else differs), served the probe of concurrent reads
([below](#the-window)).

Run order: the three cold sets, then the warm A B B A, the two capped runs, the probes, focused runs
of the rows the warm runs showed slower (A B B A B A A B, four times the operations), the loopback
reads, and the diagnostic build's warm A/B of the large reads with the cap; the focused and
diagnostic runs' files are not kept, and their figures are below. No run had errors. Earlier the
same day, with coalescing alone (a window of 8), a warm A B B A and a cold capped A B B A B A A B
ran, a diagnostic build swept fixed windows, and both processes were profiled; the Mac then
restarted, and their files, kept in a scratch directory, were lost. Their figures below are from the
session's notes, and say so.

## What was built

**Coalesced fetches** ([pool.rs](../../../crates/voidfs-server/src/pool.rs),
[metrics.rs](../../../crates/voidfs-server/src/metrics.rs)). `Pool::shard` and `Pool::page` share
one path, `Pool::read`:
- A read that misses the cache looks in a map of the fetches in flight, by hash. If one is there, it
  waits for it; otherwise it starts one and puts it there. However many readers miss a shard at
  once, the bucket is asked for it once. The map is a plain `HashMap` under a `std` mutex, never
  held across an await. moka's own coalescing (`get_with`) on its synchronous cache would block the
  thread that waits, and a moka cache's housekeeping blocks the async worker that runs it (the
  header of [guard.rs](../../../crates/voidfs-server/src/gc/guard.rs)).
- **The fetch runs on a task of its own,** which every reader waits for through a shared future. A
  reader that goes away (a client that disconnects drops its body, and the reads in its window with
  it) leaves the fetch to the others; if every reader goes, the fetch still ends and fills the
  cache. Before, a fetch was part of its reader's future and was cancelled with it. At most a GET's
  window of fetches outlives a request that goes.
- **A failure reaches every reader waiting for that fetch, and is not kept:** the fetch leaves the
  map when its task ends, however it ends, and the next read starts another.
- **The checks are made once, in the fetch.** What is fetched must match its hash, or every reader
  gets `shard … is corrupt` (or `page …`), and nothing is cached or recorded. The guard records what
  was fetched as checked (format §12.4, option 2) under the view of garbage collection the fetch
  started in, taken by the reader that started it, whoever joins later.
- A fetch puts what it read in the cache before it leaves the map, so a read that missed the cache
  just as a fetch ended finds the bytes there: it looks again under the map's lock, with moka's
  `contains_key`, which does no housekeeping. The cache and every reader share one copy of the
  bytes; before, each reader kept the fetched buffer and the cache a copy.
- **Metrics:** every read is a hit, a miss or coalesced. A new series,
  `voidfs_cache_coalesced_total{cache}`, counts the reads that waited for another read's fetch, and
  `voidfs_cache_misses_total` still counts one per request to the bucket. The admin port's table in
  the [README](../../../README.md#health-checks-and-metrics) lists it.
- Edits read shards through the same path (`Pool::fetch`), and drives load their checkpoints'
  segments through it, so concurrent edits of one file and concurrent loads share fetches too.

**Read-ahead from a shared budget** ([object.rs](../../../crates/voidfs-server/src/s3/object.rs),
`ReadAhead`). A GET kept at most 8 shard reads in flight. Once readers share fetches, that window
bounds a cold read, and a fixed wider one costs many concurrent reads ([below](#the-window)). Now:
- Each GET keeps 8 in flight, as before, and may borrow up to 24 more, to 32, from a budget of 32
  that every GET of the server shares. It borrows what is free when it starts, never waits for
  more, and gives it back when its body ends or is dropped.
- So a read alone reads 32 ahead, and many at once read 8 each and 32 between them besides: at most
  32 more shard reads in flight than before, and 32 shards' more memory (about 80 MB at the average
  2.5 MB shard; at most 512 MiB at the format's 16 MiB limit), whatever the number of requests.
- A read of 8 pieces or fewer borrows nothing.

**Pages checked against their hashes** (pool.rs,
[gc/mod.rs](../../../crates/voidfs-server/src/gc/mod.rs)). voidfs-server never checked a checkpoint
segment's bytes against its name: a segment that still parsed but was not what had been written
loaded the drive in a state it never had. Manifest pages were checked by `manifest::flatten`, after
they were cached. Now:
- the fetch checks pages as it checks shards, so a drive whose checkpoint has a corrupt segment is
  not loaded (it is logged, as any checkpoint that cannot be read is) instead of being served wrong;
- garbage collection's marking checks every page it reads. A page that parses but lies would hide
  the shards it references from the marking, and they would be collected while a file still needs
  them; now the step fails instead and deletes nothing.

Nothing an S3 client sees changed, and the format did not.

## Cold reads

Cold (`BENCH_COLD=1`: voidfs-server's caches dropped before each wave of one read per worker), the
ten read rows, `main` against this branch A B B A B A A B, in three setups. voidfs's time as a
fraction of the bare bucket's in the same run, the range over each build's four runs (lower is
better; SpaceFS's fraction is theirs to their bare bucket):

| Row | Capped (`s3`), `main` | Capped, branch | No total (`95/68/-`), `main` | No total, branch | No cap, `main` | No cap, branch | SpaceFS cold |
|---|--:|--:|--:|--:|--:|--:|--:|
| get 4 KiB | 0.02 | 0.02 | 0.02 | 0.02 | 0.02 | 0.02 | 1.08 |
| get 1 MiB | 1.03–1.04 | 1.02–1.05 | 1.02–1.03 | 0.94–1.05 | 1.03–1.09 | 1.00–1.01 | 1.18 |
| **get 32 MiB** | 0.71–0.76 | **0.19–0.31** | 0.31–0.36 | **0.24–0.30** | 1.18–1.32 | 0.66–0.68 | 0.50 |
| **get 64 MiB** | 0.71–0.73 | **0.18–0.24** | 0.27–0.31 | **0.14–0.15** | 1.17–1.24 | 0.55–0.56 | 0.38 |

voidfs's own p50 in milliseconds, the median of the four runs (each run's in brackets for the
branch), and the bare bucket's:

| Row | Setup | `main` | Branch | Bare |
|---|---|--:|--:|--:|
| get 64 MiB | Capped | 523 | 148 (145, 128, 179, 151) | 729 |
| | No total | 222 | 109 (104, 109, 113, 110) | 731 |
| | No cap | 161 | 74 (73, 74, 75, 73) | 134 |
| get 32 MiB | Capped | 277 | 82 (87, 78, 116, 69) | 371 |
| | No total | 121 | 102 (105, 89, 111, 99) | 373 |
| | No cap | 80 | 43 (42, 42, 43, 43) | 64 |

- **With the cap, both large rows are ahead of SpaceFS's cache-cleared figures in every run:** get
  64 MiB at 0.18–0.24 of the bare bucket's time (SpaceFS 0.38), get 32 MiB at 0.19–0.31 (0.50),
  from 0.71–0.76 for `main`. voidfs's own time fell 59–78% in every adjacent pair. A wave of eight
  readers now fetches each shard once: 3.6 GETs per read of 64 MiB instead of 24.
- **Without the cap's total they are ahead too,** and further: 0.14–0.15 and 0.24–0.30. Get 32 MiB
  moved least (121 to 102 ms). Probably, not confirmed: without the total, `main` was already near
  what one connection's rate allows for the file's largest shard, which bounds any read of it
  (shards are cut by content, from 256 KiB to 16 MiB; one of 8 MiB takes 88 ms at 95 MB/s).
- **Without a cap,** the bare bucket reads 64 MiB in 134 ms, not S3's 779, so its ratio cannot be
  set against SpaceFS's; voidfs's large cold reads went from 1.17–1.32× the bare bucket's time to
  0.55–0.68×.
- **The small rows are as before:** get 4 KiB is in the drive's state (`inline-data`), and get
  1 MiB is one shard, which the eight readers of a wave now fetch once (0.13 GETs per read instead
  of 1) but each still waits for: 1.00–1.05× the bare bucket, ahead of SpaceFS's 1.18×.
- The spread between the branch's capped runs (get 32 MiB 69–116 ms) is probably the content, not
  confirmed: each run writes fresh random bytes, so its shards differ in number and size. The
  slowest run made the fewest GETs per read (1.50, so about 12 shards for 32 MiB, against 14–19 in
  the others), and its GETs took longest on average (43.5 ms, against 34–39).

The other read rows:

| Row | Capped, `main` | Capped, branch | No total, `main` | No total, branch | No cap, `main` | No cap, branch |
|---|--:|--:|--:|--:|--:|--:|
| stream get 64 MiB | 0.70–0.74 | 0.19–0.26 | 0.25–0.31 | 0.14–0.23 | 1.15–1.76 | 0.52–0.54 |
| stream get 256 MiB | 0.64–0.67 | 0.15–0.19 | 0.25–0.29 | 0.13–0.15 | 0.91–1.16 | 0.46–0.49 |
| range 64 KiB of 64 MiB | 3.05–3.21 | 2.98–3.95 | 2.90–3.67 | 2.89–3.62 | 1.27–1.34 | 1.25–1.27 |
| fan-out get 200 × 256 KiB, 32 at once | 1.03–1.05 | 0.99–1.04 | 1.02–1.06 | 1.03–1.07 | 1.01–1.06 | 1.02–1.05 |
| fan-out get 1,000 × 4 KiB, 32 at once | 0.03 | 0.03 | 0.03 | 0.03 | 0.03 | 0.03 |
| fan-out get 1,000 × 4 KiB, 64 at once | 0.04 | 0.04 | 0.04 | 0.04 | 0.04 | 0.04 |

The streams move as the gets. **A cold 64 KiB range is still about 3× the bare bucket's time with
the cap:** it fetches its whole shard ([below](#ranged-shard-reads-options-not-built)). The fan-outs
read one object each, so nothing coalesces or reads ahead.

**Coalescing alone,** before the read-ahead budget, in an earlier A B B A B A A B with the cap on
the same day (its files were lost when the Mac restarted, with the scratch directory they were in;
the figures are from the session's notes): get 64 MiB 0.67–0.72 for `main` and 0.29–0.42 for the
branch, get 32 MiB 0.71–0.74 and 0.31–0.46, 3.3–3.9 GETs per read of 64 MiB, and about 4 in flight,
where the budget now allows about 10 ([below](#requests-to-the-bucket-per-cold-read)). Get 64 MiB
was behind SpaceFS's 0.38 in two runs of four; the budget is what put it ahead in all of them.

## The window

A GET kept at most 8 shard reads in flight (`.buffered(8)`). Once the eight readers of a cold object
share their fetches, a wave reads like one reader, and the window bounds it.

**It limits one read.** With coalescing alone, a diagnostic build switched a fixed window with an
environment variable, cold, at 12 ms with the cap, the harness's eight readers of one object, one
run each (from the session's notes):

| Fixed window | get 64 MiB (ms) | voidfs / bare | get 32 MiB (ms) | voidfs / bare |
|--:|--:|--:|--:|--:|
| 8 | 226 | 0.31 | 126 | 0.34 |
| 16 | 165 | 0.23 | 81 | 0.22 |
| 32 | 132 | 0.18 | 79 | 0.21 |
| 64 | 126 | 0.17 | 83 | 0.22 |

A window of 8 kept about 5 GETs in flight, not 8: it counts shards fetched but not yet sent, which
go out in order, so a slow one holds the others' places.

**What a wider window costs many concurrent reads,** which the harness cannot show (its readers
share one object). A probe: versitygw, the same relay, and the diagnostic build of this branch
(`VOIDFS_DIAG_READ_AHEAD`, `VOIDFS_DIAG_READ_AHEAD_SHARED`), sixteen 64 MiB objects of random
bytes, then `N` GETs at once with `curl`, one per object, each measurement on a server started for
it (so cold, and its memory its own), the three settings taking turns, three times each: a fixed
window of 8 (coalescing alone), the shared budget as built (8, and up to 32 from a budget of 32),
and a fixed window of 32. Medians of the three: each read's time (ms), the server's peak resident
memory (MiB, sampled every 20 ms) and the mean number of bucket GETs in flight.

| Setup | `N` | Fixed 8 | As built | Fixed 32 |
|---|--:|---|---|---|
| The cap (`s3`) | 1 | 215 ms | 110 ms | 110 ms |
| | 4 | 294 ms, 360 MiB, 19 | 279 ms, 390 MiB, 24 | 290 ms, 501 MiB, 69 |
| | 8 | 545 ms, 701 MiB, 39 | 545 ms, 720 MiB, 45 | 546 ms, 953 MiB, 148 |
| | 16 | 1,066 ms, 1,111 MiB, 81 | 1,042 ms, 1,146 MiB, 82 | 972 ms, 1,540 MiB, 279 |
| No total (`95/68/-`) | 1 | 195 ms | 67 ms | 65 ms |
| | 4 | 204 ms, 377 MiB, 17 | 147 ms, 416 MiB, 19 | 111 ms, 514 MiB, 54 |
| | 8 | 199 ms, 732 MiB, 32 | 218 ms, 752 MiB, 36 | 394 ms, 811 MiB, 86 |
| | 16 | 365 ms, 1,001 MiB, 58 | 372 ms, 1,003 MiB, 61 | 504 ms (slowest 1.0–1.3 s), 1,273 MiB, 125 |
| No cap | 1 | 77 ms | 44 ms | 45 ms |
| | 8 | 170 ms, 650 MiB, 33 | 169 ms, 743 MiB, 41 | 448 ms (slowest 0.7–1.1 s), 846 MiB, 60 |
| | 16 | 384 ms, 940 MiB, 52 | 362 ms, 947 MiB, 59 | 540 ms (slowest 0.7–1.4 s), 1,175 MiB, 145 |
| No relay (loopback) | 1 | 27 ms | 26 ms | 26 ms |
| | 8 | 106 ms, 643 MiB, 16 | 88 ms, 674 MiB, 15 | 108 ms, 797 MiB, 50 |
| | 16 | 223 ms, 955 MiB, 27 | 188 ms, 948 MiB, 24 | 283 ms, 1,273 MiB, 52 |

- **Alone, a read gains** from reading further ahead wherever one connection's rate is the limit:
  twice as fast with the cap, three times without its total.
- **Many at once, a fixed window of 32 gains nothing and costs:** 17–40% more peak memory, two to
  four times the GETs in flight, and, without the cap's total or without a cap, reads up to 2.6×
  slower, with some taking over a second. Earlier the same day, before the Mac restarted, fixed
  windows of 16 to 64 with sixteen reads at once also left some reads waiting 4–6 seconds through
  the relay, with or without the cap; this session's runs did not. How much of the slowdown is the
  relay's, and how much a real bucket would show, is not known.
- **The shared budget keeps the first and avoids the second:** alone it reads as fast as a fixed
  window of 32, and many at once it behaves as a window of 8: as fast, with at most 15% more peak
  memory and 30% more GETs in flight. With four reads at once and no total, the reads that got the
  budget finished first (147 ms at the median, against 204).

That is what was built ([above](#what-was-built)). In the harness's cold rows, the budget took get
64 MiB from 0.29–0.42 of the bare bucket's time with coalescing alone to 0.18–0.24.

## Requests to the bucket per cold read

From the result files (`BENCH_BUCKET_REQUESTS=1`, from the server's metrics over the measured
rounds), the median of each build's four runs. "In flight" is the time voidfs's GETs to the bucket
took, added up, over the wall-clock time of its rounds (drops included): the mean number at once.

| Row | GETs per read, `main` → branch: capped | No total | No cap | GETs in flight, `main` → branch: capped | No total | No cap |
|---|---|---|---|---|---|---|
| get 64 MiB | 23.8 → 3.6 | 29.3 → 3.4 | 25.8 → 3.6 | 39 → 10 | 40 → 10 | 40 → 7 |
| get 32 MiB | 13.9 → 1.8 | 14.0 → 1.9 | 13.9 → 1.8 | 40 → 6 | 37 → 5 | 35 → 5 |
| stream get 64 MiB | 26.4 → 3.6 | 29.7 → 3.1 | 23.6 → 3.8 | 41 → 10 | 41 → 8 | 34 → 8 |
| stream get 256 MiB | 84.8 → 14.1 | 100.1 → 13.8 | 86.7 → 13.6 | 39 → 14 | 42 → 12 | 38 → 8 |
| get 1 MiB | 1.00 → 0.13 | 1.00 → 0.13 | 2.00 → 0.12 | 6.7 → 0.8 | 6.6 → 0.8 | 12.2 → 0.7 |
| range 64 KiB of 64 MiB | 1.03 → 0.91 | 1.02 → 0.85 | 1.02 → 0.87 | 5.2 → 4.7 | 5.1 → 3.8 | 6.2 → 5.2 |
| fan-out get 200 × 256 KiB | 1.00 → 1.00 | 1.00 → 1.00 | 1.00 → 1.00 | 22 → 22 | 22 → 22 | 21 → 22 |
| get 4 KiB, fan-out gets of 4 KiB | 0 → 0 | 0 → 0 | 0 → 0 | – | – | – |

- **The eight readers of a wave fetch each shard once:** a 64 MiB object is 25–30 shards, so about
  3.5 GETs per read, where each read used to fetch most of them. The bytes fetched per wave of get
  64 MiB fell from about 510 MB to about 75.
- **Fewer GETs are in flight, not more,** even though a lone read may now keep 32: 5–14 on the large
  rows against `main`'s 34–42, since eight readers share them. The first reader of a wave borrows 24
  of the budget and the second the 8 left; the others read 8 ahead, mostly waiting for fetches
  already started.
- **Ranges** coalesce when two readers' random offsets fall in the same shard: 0.85–0.91 GETs per
  read.

## The 49 rows with the cap

`capped-branch-1` and `-2`: warm, all 49 rows, at 12 ms with `BENCH_BANDWIDTH=s3`. **45 of the 49
rows are at or ahead of SpaceFS's ratio in both runs**: all 24 edits, all 11 writes, 6 of the 10
reads and the 4 metadata rows. 40 and 41 are faster than the bare bucket, and the geometric mean
speed-up over it is 5.8× (SpaceFS's: 2.8×). The earlier session's two runs, before this change, had
46 ([cold-reads](../cold-reads/README.md#the-49-rows-with-the-cap)).

**Behind,** in one run or both:

| Row | voidfs (ms) | Bare (ms) | Speed-up | SpaceFS | Of SpaceFS's ratio |
|---|---|---|---|--:|---|
| get 64 MiB | 55.1, 57.0 | 728, 733 | 13.2×, 12.8× | 16.5× (47.2 / 779) | 0.80, 0.78 |
| stream get 256 MiB | 222, 219 | 2,840, 2,841 | 12.8×, 13.0× | 15.6× (178 / 2,772) | 0.82, 0.83 |
| stream get 64 MiB | 52.8, 49.8 | 722, 722 | 13.7×, 14.5× | 15.5× (45.1 / 697) | 0.88, 0.94 |
| range 64 KiB of 64 MiB | 0.34, 0.41 | 12.1, 13.2 | 35×, 33× | 34× (1.3 / 44.1) | 1.04, 0.96 |

- **The three large reads are the same three as before,** at about the same fraction of SpaceFS's
  ratio: warm reads served from memory, which this change does not touch. Where their time goes is
  [below](#where-the-warm-large-reads-time-goes). The read-ahead budget does not change them either:
  one diagnostic binary switched between a fixed window of 8 and the budget, A B B A B A A B, warm
  with the cap, gave get 64 MiB +6.3% in voidfs's own time (pairs +11% −3% +6% +4%), stream get 64
  MiB +3.2% and stream get 256 MiB −2.8%; get 64 MiB again in the reverse order (B A A B A B B A) at
  16 times the operations, +0.1% (+3% −0% +2% −6%).
- **The range is new among them, and noise:** served from memory in 0.3–0.4 ms, it is 33–35× faster
  than the bare bucket against SpaceFS's 34×, so a tenth of a millisecond decides it. In the earlier
  session it was 37× and 41×. A warm one-piece read borrows nothing and takes the same path as
  before.
- Get 32 MiB sits on SpaceFS's ratio (1.00 and 1.01 of it), as before (1.03).

By group, as in [PARITY.md §6](../../../docs/PARITY.md#where-voidfs-stands), the medians of each row
over the two runs:

| Scenarios | Rows | Capped, 1 October, before this change | Capped, with it | SpaceFS |
|---|--:|---|---|---|
| Small, ranged and cached reads, `head`, fan-out gets | 7 | 12–53× faster | 12–48× faster | 2.6–34× faster |
| Large gets and streams | 4 | 12–14× faster | 12–14× faster | 12–17× faster |
| Edits inside 32 and 64 MiB files | 16 | 3.4–37× faster | 3.3–38× faster | 1.4–15× faster |
| Rename and folder move | 2 | 9.5–28× faster | 9.4–31× faster | 7.9–18× faster |
| Listing | 1 | 42× faster | 41× faster | 9.1× faster |
| Edits inside 1 MiB files | 8 | 1.1–1.2× faster | 1.1–1.2× faster | 2.1× slower to parity |
| Whole-object puts and overwrites, fan-out puts | 9 | 2.1× slower to 1.8× faster | 2.2× slower to 1.8× faster | 1.1–3.1× slower |
| Multipart uploads | 2 | 1.0× faster | 1.1× slower to 1.0× faster | 1.8–2.4× slower |
| **All 49**: faster in / geometric mean | | 42 / 5.9× | 41 / 5.8× | 31 / 2.8× |

## Warm: what the change costs

`warm12-main-1`, `-branch-1`, `-branch-2`, `-main-2`: the 49 rows at 12 ms without the cap, warm,
compared in the same position (`main-1` with `branch-2`, `main-2` with `branch-1`). Relative to the
bare bucket in the same run, the geometric mean of the p50 ratios over the 49 rows is **0.980**
(median 1.010). By family: edits 0.993, metadata 1.136, reads 1.009, writes 0.877.

| Run | Rows at or ahead of SpaceFS's ratio (edits, writes) | Rows faster than the bare bucket |
|---|--:|--:|
| `warm12-main-1` | 29 (12, 7) | 30 |
| `warm12-branch-1` | 30 (13, 7) | 30 |
| `warm12-branch-2` | 31 (12, 9) | 31 |
| `warm12-main-2` | 29 (12, 7) | 30 |

Warm, every read but a cold one is a hit, which takes the same path as before; only edits that
miss and loads of checkpoints fetch. Rows that looked slower in both positions, and the two the
earlier session's warm runs had shown slower, were run focused again: A B B A B A A B, `main`
against the branch, four times the operations, at 12 ms without the cap (files not kept).

| Scenario | Full runs, relative to bare | Focused, voidfs's own p50, `main` → branch | Each pair, own |
|---|--:|---|---|
| rename 64 MiB | +25.6% | 12.5 → 12.6 ms (+1.0%) | −5% +2% −0% −0% |
| fan-out get 1,000 × 4 KiB, 64 at once | +12.2% | 0.91 → 0.91 ms (+0.2%) | +3% −2% −2% +1% |
| list 200 keys | +10.6% | 0.89 → 0.92 ms (+2.9%) | +1% +1% +7% +5% |
| insert 4 KiB, middle of 32 MiB | +7.6% | 37.5 → 36.9 ms (−1.6%) | +14% −5% +1% −10% |
| get 1 MiB | (+12.6%, one position) | 0.73 → 0.72 ms (−2.2%) | −2% +1% −0% −3% |
| delete 4 KiB, start of 64 MiB | (+7.7%, one position) | 34.5 → 34.1 ms (−1.1%) | −8% +1% −2% +0% |
| write at 4 KiB in 64 MiB | +4.7% (earlier session +22.8%) | 34.9 → 37.5 ms (+7.3%) | +5% +8% +6% +9% |
| delete 4 KiB, middle of 32 MiB | −13.8% (earlier session +9.7%) | 38.6 → 41.0 ms (+6.3%) | +6% +6% +15% +1% |

- **The rows the full runs showed slower were not,** run focused: within −2.2% to +2.9% of `main`.
- **Two edits took 6–7% longer in all or most pairs,** and are not explained. Neither reads from
  the bucket in either build (no GETs, the same PUTs and log entries per operation); of their extra
  2.5 ms, about 1 ms is the upload of the shard they rewrite (17.0 and 19.0 ms at the median on the
  branch, 16.3 and 18.1 on `main`), which this change does not touch. Two edits that run the same
  code (delete at the start of 64 MiB, insert in the middle of 32 MiB) moved −1.1% and −1.6% in the
  same session. No diagnostic switch could tell the code apart, since for these requests it is the
  same; it is within what one binary does against another here (6% at 12 ms, #16), but its sign
  held.
- The writes' 0.877 is multipart put 256 MiB × 16 MiB's bare bucket, which varied 2× again
  (−82% and −30% by position), and the fan-out puts; neither runs this code.

## Where the warm large reads' time goes

With the cap, warm, three rows were behind SpaceFS's ratio: get 64 MiB and stream get 64 and
256 MiB. voidfs serves 64 MiB from memory in 50–57 ms, eight at once, where SpaceFS took 45–47.

**Profiled** with `sample <pid> 600 1 -mayDie` on voidfs-server and on the harness at once, during a
focused run of get 64 MiB at 12 ms with the cap, voidfs alone (`--targets voidfs`) at 16 times the
operations: 1,027 GETs, which the sampling slowed to 80 ms each. The build had coalescing alone;
the read-ahead budget does not change warm reads ([above](#the-49-rows-with-the-cap)). The sample
files are not kept.

| Process | Samples outside waiting | Where |
|---|--:|---|
| voidfs-server | 9,138 | `writev` 9,076 (99.3%): the kernel copying the bodies into the sockets, about 8.8 ms of a core per GET. voidfs's own work (the cache lookup, the read plan, slicing) is in the other 62 |
| Harness | 5,712 | `memmove` 3,063, all in `AggregatedBytes::into_bytes`, which copies each collected body into one buffer (3.0 ms per GET); `recvfrom` 2,068 (2.0 ms per GET) |

Neither process was busy: the server used about 0.9 of a core and the harness 0.6, of 15. Socket
buffers grow to 4 MiB on their own (`net.inet.tcp.autosndbufmax`).

**The floor is the machine's.** The same read on loopback, warm, against versitygw serving the
same bytes from the page cache, at 1, 2, 4 and 8 at once (`--concurrency`, one run each,
`warmconc-*`):

| At once | voidfs, this branch (ms) | voidfs, `main` (ms) | versitygw (ms) | All readers together, this branch |
|--:|--:|--:|--:|--:|
| 1 | 7.9 | 8.1 | 7.1–7.4 | 8.5 GB/s |
| 2 | 13.4 | 14.0 | 9.1 | 10.0 GB/s |
| 4 | 22.7 | 24.8 | 18.3–21.3 | 11.8 GB/s |
| 8 | 56.1 | 52.7 | 49.6–50.4 | 9.6 GB/s |

(The last column is the readers' bytes over the p50: 67 MB each.) Eight readers take 10–11 GB/s
through loopback whoever serves them: versitygw, serving from the page cache, takes 50 ms for what
voidfs serves in 53–56. On this Mac that is the floor for both, and SpaceFS's 45–47 ms is near it.

So the time is accounted for: the transfer over loopback, which eight readers of anyone's bytes take
about as long over; the harness's own copy of each body (about 3 ms of the 50, inside the timing for
both targets); and 6–12% over versitygw for voidfs at eight at once. **No cheap and safe fix was
found on the server,** whose CPU is the kernel's copy, and the harness was not changed. SpaceFS's
figure was taken on an 8-vCPU VM, and their bare bucket was 7% slower than the cap's (779 ms against
728), which counts against voidfs here; the real run (item 7.2) will judge these rows.

## Ranged shard reads: options, not built

A cold 64 KiB range fetches the whole shard it falls in, about 2.5 MB, and checks it against the
shard's hash before it sends a byte: with the cap, about 3× the bare bucket's time (below). A ranged
GET of the shard would fetch 64 KiB, but a SHA-256 of the whole shard cannot check a part of it.
The format already allows the read ([format §4](../../../spec/format.md#4-shards): a shard's object
is exactly its bytes, "which lets a reader fetch part of a shard without downloading it all";
readers SHOULD verify every *whole* shard they fetch). Every option below either weakens the check
voidfs-server makes today or changes the format, so none was built; the decision, after looking at
what SpaceFS does, is at the end of this section.

| Option | A cold 64 KiB range would cost | What it gives up or adds |
|---|---|---|
| 1. A ranged GET, served unchecked | One GET of 64 KiB: about the bare bucket's time | **Weakens the hash check:** a corrupt or substituted shard object reaches the client. The format allows it (its SHOULD covers whole shards only) |
| 2. A ranged GET for the client, and the whole shard behind it to check and cache | The same, plus the whole shard's download | No better than 1: the 64 KiB is sent long before the 2.5 MB arrives to be checked, so a mismatch can only be logged, or the connection cut after the fact. Twice the bytes |
| 3. The bucket's own checksums (S3's `x-amz-checksum-mode`) | One GET, but every shard uploaded in parts, since S3 checks a range only as one of an object's parts | Trusts the bucket's checksum rather than the content address, so a weakening too; three requests per shard upload; S3-compatible stores differ |
| 4. Smaller shards in new pools (the pool's `chunking`, fixed when it is created: no format change) | At a 512 KiB average, one GET of about 0.5 MB: roughly 1.4× the bare bucket's time instead of 3× | Four times the shards, requests and extents: a large read or write makes four times the GETs or PUTs, and pays for them, and a descriptor needs a tree from 512 MiB instead of 2 GiB. Existing pools keep theirs |
| 5. **Block hashes, by an RFC.** Each shard also gets hashes of fixed blocks (say 64 KiB), bound to its name so that any block can be checked alone: a sidecar object per shard whose hash the extent carries (about 1 KiB for 2 MiB), or a tree hash as the shard's name (BLAKE3 with a Bao outboard; `voidfs.json`'s `hash` names the function) | One ranged GET of the blocks the range covers, and the matching part of the sidecar or outboard, fetched together: about the bare bucket's time | An incompatible feature and an RFC; another object per shard, or another hash function, for every reader and writer; dedup only between pools that hash alike. In return, a cold large read could also send each block as it arrives and checks out, rather than wait for whole shards |

- **No row of SpaceFS's needs it.** They publish no cold figure for the range; warm it is about
  0.3 ms here, 33–41× faster than the bare bucket with the cap, against their 34×.
- **The mount will.** Random reads in large files (video, disk images, databases) are the case.
  Read-ahead (checklist D6) and the disk tier (S5) serve sequential and repeated reads, not cold
  random ones.
- If cold random reads come to matter before the mount does: option 5 through an RFC, or option 1
  behind a pool setting for buckets the operator trusts.

**What SpaceFS does** (read on 1 October 2026):
- **Its S3 layer, which the benchmark measures, reads whole shards for a range.** Their operation
  docs (`docs.spacefs.com/llms-full.txt`, "Read a byte range") say it "Fetches only the shards the
  range touches", at a cost "proportional to the range, not the object": shard granularity, as
  voidfs does, served from their edge cache when it holds them.
- **Its Mac client appears to read parts of shards, checked only for length.** Inferred from the
  strings of the Rust daemon in Space 0.2.300 (`spacefs-fskitd`), not from its code. Its read path
  logs two kinds of fetch:
  - whole shards ("shard: served from cache", "shard: waited for a fetch in flight", "shard:
    fetched whole"), checked against their hash ("immutable shard checksum mismatch"; through their
    edge, "edge worker returned bytes that do not match the content-addressed shard key; retry
    direct origin");
  - pieces ("piece: served from cache", "piece: waited for a fetch in flight", "piece: fetched"):
    bounded range GETs of a shard object, of at most 1 MiB ("hydration request exceeds 1MiB"),
    whose only checks in the strings are of length ("short bounded shard response", "bounded shard
    response exceeded requested range", "provider did not honor exact bounded range"). No string
    names BLAKE3, Bao or a Merkle tree.

  That is option 1 above, in the client. It also coalesces fetches as voidfs now does, and its
  read-ahead takes slots ("block-ahead: issued", "queued", "no slot"), much as the shared budget.

**Decided (1 October 2026): no change to the gateway; revisit with the mount.** The gateway
already reads ranges as SpaceFS's S3 layer documents, every cold row SpaceFS publishes is ahead, and
the gateway keeps its whole-shard hash check. When the writable mount is built (parity plan step 5,
checklist D6), its client should read pieces as SpaceFS's appears to: bounded range GETs of up to
1 MiB, cached and coalesced apart from whole shards, while every whole shard stays checked. How a
piece is checked is to be decided then: by its length alone, as SpaceFS appears to (no format
change; it trusts the bucket for partial reads, as voidfs already trusts it with the log), or by
block hashes (option 5, an RFC), which would check every piece.
## Tests

Nine new tests, each seen to fail with the code it guards broken, running only that test, with a
timeout (19 ways); a test that waits for something gives up after five seconds rather than hang.

In [pool.rs](../../../crates/voidfs-server/src/pool.rs):
- **One fetch for many readers:** eight reads of a shard, and of a page, whose fetch the bucket
  holds back, make one request to the bucket, and all get the bytes; one counts as a miss and seven
  as coalesced, and the next read is a hit. Broken: no coalescing; a coalesced read counted as a
  miss.
- **A failure reaches every reader and is not kept:** the held fetch fails, and all eight readers
  get the bucket's error, after one request; nothing is cached; the next read asks again and gets
  the bytes. Broken: the failed fetch left in the map; each waiter trying again on its own.
- **A reader that goes away:** the read that started a fetch is cancelled while seven others wait
  for it; they all get the bytes, from one request. Then all three readers of another shard are
  cancelled, and the fetch still ends and fills the cache: the next read makes no request. Broken:
  the fetch run inside the readers' shared future instead of on a task of its own (the first part
  still passes; the cache is never filled).
- **A corrupt shard or page:** bytes that do not match the hash are refused to all eight readers,
  after one request; nothing is cached; the next read asks again and is refused again; a write of
  it uploads it, so it was not recorded as stored. Broken: no hash check; the guard told before the
  check; the bytes cached before the check; pages left unchecked.
- **The view the fetch started in:** a shard read while garbage collection's view stays is not
  uploaded again by a write; one whose fetch was in flight when a run began, and which a second
  reader joined after, is. Broken: the view taken when the fetch ends; each reader recording the
  shard under its own view.
- **A checkpoint with a corrupt segment:** a segment changed so that it still parses (a file
  renamed) is refused, with the page named, and the drive is not loaded. Broken: pages left
  unchecked.

In [gc/tests.rs](../../../crates/voidfs-server/src/gc/tests.rs):
- **A corrupt page stops collection:** a manifest leaf changed to list another shard in place of one
  of its own (it still parses) makes the step fail, and the shard the file needs stays. Broken: the
  marking not checking pages, which then collected that shard.

In [object.rs](../../../crates/voidfs-server/src/s3/object.rs):
- **GETs share the budget:** three GETs of three cold objects of about 160 shards each, the bucket
  holding back their shard reads: the first reads 32 ahead, the second 16 (the 8 the first left),
  the third 8; all get their bytes, and the budget is whole again after. A read of one shard borrows
  nothing. Broken: no borrowing; a wide window without budget; the budget given back as the GET
  starts; a read of one piece borrowing.
- **A GET that goes away gives its budget back,** whether its body was never read or read in part;
  while the body is open it keeps it. Broken: the budget given back as the GET starts; never given
  back.

Not covered by a test: the second look in the cache under the map's lock, for a read that missed
just as a fetch ended. The window between the two is a few instructions with no await, which a test
cannot hold open; without that look, such a read makes one more request, not a wrong one.

`cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass, and
`tests/interop/run.sh` over `memory`, `fs` and `versitygw`: 38 conformance cases in both addressing
styles, the rclone checks, aws-chunked, the admin listener and a graceful SIGTERM. The boto3 checks
were skipped: boto3 is not installed on this Mac (CI runs them).

## What is left

- **Ranged shard reads,** deferred to the mount (step 5, checklist D6), where its client should read
  pieces of shards as SpaceFS's appears to; how a piece is checked is decided then
  ([above](#ranged-shard-reads-options-not-built)).
- **The three warm large reads** behind SpaceFS's ratio with the cap, at this Mac's loopback limit:
  the real run in SpaceFS's setup (item 7.2) will judge them, and settle S3's download total, cold.
- **Read-ahead across requests** for the mount (checklist D6), and **the disk tier** (S5).
- The read-ahead budget's sizes (8, 32, 32) were chosen from these measurements on one machine;
  a server with many clients reading large files at once may want them set, and nothing exposes
  them yet.

## Caveats

- One machine: the harness, versitygw, the relay and the server share 15 CPUs and one disk.
- The cap is a model of S3 fitted to one published run of SpaceFS's; its total for downloads is an
  assumption ([cold-reads](../cold-reads/README.md#the-bandwidth-cap)).
- The relay adds its delay with a timer, about 14–15 ms a round trip in all, and is a model of a
  network: how many of its slowdowns with hundreds of connections a real bucket would show is not
  known.
- Content is fresh random bytes in every run, so the number and sizes of a file's shards differ from
  run to run, and with them a cold read's time.
- The Mac restarted during the work, and the files of the first runs (coalescing alone, the fixed
  window sweep, the profile) were lost with the scratch directory they were in; their figures are
  from the session's notes. Every result file here is from the final build.
- SpaceFS's figures are from their cloud run, not this setup; the comparison is of each row's ratio
  to the bare bucket.
