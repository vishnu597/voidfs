# Shard cache admission: before and after

*28 September 2026, one Mac (Apple M5 Pro, 15 CPUs, macOS 27). The bucket (versitygw 1.8.0) 12 ms
away through `BENCH_ONE_WAY_MS=4 bench/scripts/local.sh`, with voidfs-server's 512 MiB cache.
Step 3, item 2 of the [parity plan](../../../docs/step-3-performance.md#item-2-shard-cache-admission).*

| Files | voidfs-server |
|---|---|
| `probe-main-*`, `full-main-*` | `main` at `634267e`: moka's default admission (TinyLFU) for the shard and page caches |
| `probe-lru-*`, `full-lru-*` | This branch: least recently used first, and the caches keep their own copy of what they hold |
| `r2-main`, `r2-lru` | The same two builds against Cloudflare R2, with a 64 MiB cache ([below](#against-cloudflare-r2)) |

Both binaries were built with `cargo build --release -p voidfs-server` and passed with
`BENCH_SERVER_BIN`; the files' `commit` and `voidfs server` labels say which is which. The change
adds no string to the binary, so they were told apart by hash and by behaviour (the fan-out gets
below).

Run order:
1. The probe sequence of [cache-tinylfu](../probes/cache-tinylfu.md) and
   [cache-lru](../probes/cache-lru.md): the first ten rows of the full run, then fan-out get
   1,000 × 4 KiB, voidfs only. `main`, branch, branch, `main`.
2. The full 49 rows: `full-main-1`, `full-lru-1`, `full-lru-2`, `full-main-2` (A B B A, because
   consecutive full runs alternate by position; see [checkpoints](../checkpoints/README.md)).
3. Focused runs of the cache and write rows after a fill, alternating builds, and the small writes
   one at a time.
4. Fan-out put 1,000 × 4 KiB at 64 at once with the cache full, rotating `main`, the branch and
   diagnostic builds (below).

The files of 3 and 4 are not kept; the tables below have their figures.

An earlier round of the same probe and full runs, on a build without the copies, is described
under [Memory](#memory); its files are not kept either.

## What to expect

The shard cache is a moka cache weighed in bytes. moka's default admission, TinyLFU, lets a new
entry in only if it has been used more often than all the entries it would evict put together
(`BaseCache::admit` in moka 0.12). A shard just written has been used no times, and a 2 MiB shard
displaces many smaller ones, so once the cache was full it turned every new shard away. Shards
read often by earlier scenarios, of drives long deleted, stayed. Reads of anything newer went to
the bucket, and nothing said so.

With least recently used first, what was written or read last stays. The rows that should move
are the warm reads after the cache fills (get 32 MiB, get 1 MiB's p90, the three fan-out gets),
and any edit, which reads the shard it rewrites.

## The probe

p50 in ms, and each round's p50 and p90:

| Scenario | main | branch | branch | main |
|---|--:|--:|--:|--:|
| get 64 MiB | 47.4 | 49.7 | 49.8 | 51.8 |
| truncate 4 KiB, end of 64 MiB | 46.9 | 36.6 | 36.6 | 43.8 |
| get 32 MiB | 79.4 (78.0 / 83.8, 80.8 / 85.1) | 27.3 (28.8 / 35.1, 25.7 / 26.9) | 26.0 (26.0 / 31.6, 26.1 / 28.9) | 54.3 (64.7 / 73.4, 43.8 / 268.8) |
| fanout get 1000 × 4 KiB, 32 at once | 13.7 (13.6 / 14.9, 13.7 / 15.4) | 0.47 (0.47 / 0.7, 0.47 / 0.7) | 0.57 (0.66 / 1.3, 0.48 / 0.7) | 13.1 (13.1 / 14.6, 13.1 / 14.4) |

- Get 32 MiB now takes the same time per byte as get 64 MiB, and the fan-out get is under a
  millisecond. The trial build of 27 September measured 27 and 0.54 ms.
- Truncate in 64 MiB, an edit, is faster for the reason below. Move dir took 28.7 ms in both of
  the branch's probes, against 26.4 and 26.9 in `main`'s; it only commits, and in the full runs
  both builds took 25.5 ms. The other rows did not move beyond the spread between runs.

## Result at 12 ms

Comparing runs in the same position (`full-main-1` with `full-lru-2`, `full-lru-1` with
`full-main-2`), the geometric mean of the p50 ratios over the 49 rows is **0.766** and the median
0.973. By family: reads 0.378, edits 0.881, writes 0.979, metadata 0.987. No run had errors.

Against SpaceFS's ratios to the bare bucket (a row is at or ahead if `bare_p50 / voidfs_p50 >=
spacefs.bare / spacefs.layer`):

| Run | Rows at or ahead of SpaceFS | Edits (24) | Writes (11) | Reads (10) | Metadata (4) | Rows faster than the bare bucket |
|---|--:|--:|--:|--:|--:|--:|
| `full-main-1` | 16 | 8 | 3 | 3 | 2 | 25 |
| `full-lru-1` | 22 | 12 | 2 | 6 | 2 | 30 |
| `full-lru-2` | 21 | 9 | 3 | 6 | 3 | 30 |
| `full-main-2` | 19 | 10 | 3 | 3 | 3 | 25 |

19 rows are ahead in both of the branch's runs, against 15 in both of `main`'s. The three fan-out
gets and patch 16 × 4 KiB in 32 MiB are ahead in both runs of the branch and in neither of
`main`'s. Delete 4 KiB in the middle of 1 MiB went the other way, ahead in one run of two instead
of both, with a p50 2% lower: it sits at SpaceFS's ratio, and the bare bucket's time moved between
runs. Over the 49 rows, the geometric mean speed-up over the bare bucket is 2.6× (`main`: 2.0×;
SpaceFS: 2.8×).

The five rows this item was for (p50 in ms, and the highest round p90 of the run):

| Scenario | main | branch | Bare bucket | SpaceFS's ratio needs |
|---|--:|--:|--:|--:|
| get 32 MiB | 56.4, 88.2 (310, 98.3) | 25.6, 25.2 (28.9, 31.0) | 66–68 | ≤ 5.6–5.8 |
| fanout get 1000 × 4 KiB, 32 at once | 13.3, 12.5 (14.6, 14.3) | 0.48, 0.49 (0.7, 0.7) | 12.0–12.6 | ≤ 2.3–2.4 |
| fanout get 1000 × 4 KiB, 64 at once | 13.3, 12.4 (16.0, 14.6) | 0.99, 0.93 (1.7, 1.8) | 12.0–12.2 | ≤ 4.6 |
| fanout get 200 × 256 KiB, 32 at once | 15.1, 14.4 (18.5, 19.8) | 1.06, 1.10 (2.4, 2.7) | 12.9–13.9 | ≤ 2.1–2.3 |
| get 1 MiB | 1.19, 1.35 (16.3, 15.1) | 1.17, 1.09 (1.6, 1.6) | 13.1–14.7 | ≤ 2.9–3.2 |

- The fan-out gets are ahead of SpaceFS's ratio with room to spare, and get 1 MiB's p90 is a hit.
- Get 32 MiB (25 ms) is where get 64 MiB (53–56 ms) puts it, but like get 64 MiB it can't reach
  SpaceFS's ratio here: the emulated bucket adds latency but no bandwidth limit, so the bare
  bucket reads 32 MiB in 66 ms where S3 took 356 ms in SpaceFS's run. Only the real run can
  judge the large reads.

Rows that moved by 3% or more:

| Scenario | main (ms) | branch (ms) | Change | odd / even |
|---|--:|--:|--:|--:|
| fanout get 1000 × 4 KiB, 32 at once | 12.9 | 0.5 | −96.2% | −96.3% / −96.1% |
| fanout get 200 × 256 KiB, 32 at once | 14.7 | 1.1 | −92.7% | −92.7% / −92.6% |
| fanout get 1000 × 4 KiB, 64 at once | 12.8 | 1.0 | −92.5% | −93.0% / −92.1% |
| get 32 MiB | 72.3 | 25.4 | −64.0% | −55.4% / −71.0% |
| patch 16 × 4 KiB in 64 MiB | 320.9 | 137.4 | −57.2% | −55.3% / −59.1% |
| patch 16 × 4 KiB in 32 MiB | 137.5 | 95.8 | −30.3% | −28.6% / −32.0% |
| truncate 4 KiB, end of 32 MiB | 47.4 | 36.6 | −22.7% | −28.8% / −16.1% |
| delete 4 KiB, middle of 64 MiB | 49.1 | 38.2 | −22.1% | −21.4% / −22.8% |
| insert 4 KiB, middle of 32 MiB | 50.9 | 40.0 | −21.5% | −30.8% / −11.0% |
| delete 4 KiB, middle of 32 MiB | 51.3 | 40.7 | −20.8% | −14.2% / −26.9% |
| delete 4 KiB, start of 32 MiB | 44.6 | 37.7 | −15.6% | −11.1% / −19.9% |
| write at 4 KiB in 32 MiB | 44.7 | 38.1 | −14.8% | −7.4% / −21.6% |
| write at 4 KiB in 64 MiB | 43.8 | 37.6 | −14.0% | −12.3% / −15.8% |
| delete 4 KiB, start of 64 MiB | 44.0 | 38.0 | −13.4% | −23.8% / −1.5% |
| insert 4 KiB, middle of 64 MiB | 48.4 | 42.4 | −12.4% | −7.3% / −17.3% |
| truncate 4 KiB, end of 64 MiB | 41.0 | 36.3 | −11.3% | −15.5% / −7.0% |
| get 1 MiB | 1.3 | 1.1 | −10.9% | −8.3% / −13.4% |
| range 64 KiB of 64 MiB | 0.3 | 0.3 | −10.4% | −24.6% / +6.6% |
| overwrite 4 KiB | 32.3 | 29.0 | −10.0% | −7.3% / −12.5% |
| put 4 KiB | 31.8 | 29.2 | −8.0% | −5.4% / −10.5% |
| stream get 64 MiB | 53.2 | 49.8 | −6.2% | +1.9% / −13.7% |
| stream get 256 MiB | 203.5 | 196.0 | −3.4% | +9.5% / −14.9% |
| insert 4 KiB, start of 32 MiB | 37.1 | 39.2 | +5.6% | +7.5% / +3.8% |
| get 4 KiB | 0.4 | 0.4 | +8.0% | +12.5% / +3.7% |
| append 4 KiB to 32 MiB | 33.7 | 36.8 | +9.1% | +9.4% / +8.8% |

- **Reads.** The fan-out gets are hits now: 0.5–1.1 ms, where every read went to the bucket
  before. In `main`'s runs, fan-out get 1,000 × 4 KiB at 32 had a fastest read of 0.7–1.5 ms and a
  p50 of 12.5–13.3: a few hits among the misses.
- **12 of the 16 edits inside 32 and 64 MiB files, 11–57% faster.** An edit reads the shard it
  rewrites (and patch, the shards of its 16 edits). The file was written in the scenario's setup;
  `main` had turned its shards away and fetched them from the bucket, a round trip before the
  edit's own two. Patch in 64 MiB, which was bimodal at about 210 or 380 ms, took 132 and 143 ms.
  The appends and the inserts at the start moved within the spread between runs.
- **Slower in both positions:** append 4 KiB to 32 MiB and insert 4 KiB at the start of 32 MiB,
  and get 4 KiB (sub-millisecond). In the focused runs below, the two edits came out at +1.8%
  and −3.5% (−7.2% and −7.0% relative to the bare bucket), and in the first round they were
  −1.4% and 0.0%: the spread between runs, not the build.

## Against Cloudflare R2

*29 September 2026 (UTC), after the merge.* The 23 scenarios whose objects are 1 MiB or smaller,
at a quarter of the operations, as in the [27 September run](../r2-small-objects.md) (`--exclude
32m --exclude 64m --exclude 256m --ops-scale 0.25`), voidfs target only. voidfs-server ran on the
Mac beside the harness, each build on a pool of its own in the bucket, with `--cache-mib 64`: the
scenarios write about 0.5 GB, which never filled the default 512 MiB cache on 27 September. One
run of each build, `main` (`634267e`) first, then the branch (`32d8216`). No errors.

p50 in ms, and the highest round p90 of the run. The bare bucket was not run again; its column is
from 27 September, when a 1 MiB GET took 166 ms and a 4 KiB PUT 218 ms:

| Scenario | main | branch | Change | Bare bucket (27 Sep) |
|---|--:|--:|--:|--:|
| fanout get 200 × 256 KiB, 32 at once | 116.1 (390.2) | 2.2 (3.5) | −98% | 204.2 |
| fanout get 1000 × 4 KiB, 32 at once | 75.7 (228.3) | 0.6 (1.1) | −99% | 114.3 |
| fanout get 1000 × 4 KiB, 64 at once | 75.2 (238.7) | 1.8 (3.2) | −98% | 120.6 |
| get 1 MiB | 1.0 (356.6) | 1.6 (2.5) | p90 −99% | 166.4 |
| delete 4 KiB, middle of 1 MiB | 979.8 | 738.4 | −25% | 540.4 |
| patch 16 × 4 KiB in 1 MiB | 1030.1 | 767.3 | −26% | 545.8 |
| insert 4 KiB, middle of 1 MiB | 956.1 | 757.5 | −21% | 550.1 |
| write at 4 KiB in 1 MiB | 955.4 | 759.4 | −21% | 551.6 |
| delete 4 KiB, start of 1 MiB | 944.2 | 743.0 | −21% | 578.9 |
| insert 4 KiB, start of 1 MiB | 942.8 | 767.0 | −19% | 540.9 |
| truncate 4 KiB, end of 1 MiB | 878.4 | 799.3 | −9% | 592.7 |
| append 4 KiB to 1 MiB | 731.2 | 760.9 | +4% | 673.8 |
| puts, overwrites and fan-out puts (7 rows) | 593–750 | 588–758 | −10% to +6% | 207–320 |

- **Reads.** With the cache full, `main` sent the fan-out gets to R2, 75–116 ms each; the branch
  serves them in 0.6–2.2 ms. Get 1 MiB's p50 was a hit in both builds, but in one of `main`'s
  rounds at least a tenth of the reads went to R2 (p90 357 ms).
- **Edits in 1 MiB files, 19–26% faster** for six of the eight: an edit reads the file's shard,
  which `main` had turned away and fetched from R2 again, one 1 MiB GET. Truncate at the end moved
  less, and append not at all; at 12 ms, the appends in 32 and 64 MiB files did not move either.
- **Writes** moved by −10% to +6% with no pattern, from one run of each build.
- **Group commit on a real bucket.** Against the 27 September run of `c434fcb`, before group
  commit: edits and small puts went from 1.5–1.8 s to 0.6–1.0 s, and the fan-out puts from 6.5–13 s
  to 0.6–0.7 s. Relative to that day's bare bucket (not run again, so only a guide), the branch's
  small puts are 2.2–3.0× slower (SpaceFS: 1.7–3.1× in its setup): the shards and then the log
  entry, each a PUT of about 200 ms here (step 3, item 3). The edits in 1 MiB files are 1.1–1.4×
  slower, within SpaceFS's ratio for six of the eight; delete in the middle is just short (1.37×
  against 1.36×), and append, which SpaceFS does at parity, is not.
- Scored the same way against that day's bare bucket, 16 of the 23 rows are at or ahead of
  SpaceFS's ratio with the branch, and 8 with `main`: the three fan-out gets and five edits in
  1 MiB files are the difference.
- The runs took 8 and 7 minutes and sent 7,558 requests each to voidfs-server. Requests from
  voidfs-server to R2 were not counted. The two pools held 11,875 objects afterwards, which were
  deleted (`voidfs-bench purge`); nothing is left under the benchmark's prefix.

## Writes: the cost of inserts

A write inserts its shards into the cache, and moka's upkeep (applying pending inserts, evicting)
runs inside inserts and reads, on whichever request's thread reaches it. Garbage collection's
first reuse set, a second moka cache on the write path, made small writes 2–4% slower
([gc](../gc/README.md)). With the cache full, `main` turned each new shard away; the branch keeps
it and evicts the oldest, and copies it first.

With the cache full whenever the writes ran, the p50 change from `main` to the branch:

| | put 4 KiB | overwrite 4 KiB | fan-out put 1,000 × 4 KiB, 32 | the same, 64 | fan-out put 200 × 256 KiB | put 1 MiB | overwrite 1 MiB |
|---|--:|--:|--:|--:|--:|--:|--:|
| Full runs, by position | −8.0% | −10.0% | +1.4% | +0.8% | −2.9% | −1.1% | −0.6% |
| Focused runs, three of each build, relative to the bare bucket | −0.8% | −1.0% | +1.9% | +5.5% | −1.1% | +0.5% | +1.3% |
| One at a time, two of each, relative to the bare bucket | +0.4% | −0.4% | – | – | – | −1.2% | +0.8% |

The focused runs filled the cache with get 64 MiB, the streams and append in 64 MiB first; the
runs one at a time with every 64 and 256 MiB row. Only fan-out put 1,000 × 4 KiB at 64 at once
showed anything, so it was run again after 1 GiB of edits' setup, rotating the builds (p50 in ms,
and the mean time of a round of 1,000 puts):

| | main | branch | LRU without copies | no inserts on writes | inserts on a blocking thread |
|---|--:|--:|--:|--:|--:|
| First set, three runs each: p50 | 35.9 | 37.0 | 36.5 | – | – |
| round | 590 | 605 | 599 | – | – |
| Second set, three runs each: p50 | 39.0 | 39.4 | – | 37.3 | 39.1 |
| round | 622 | 633 | – | 625 | 630 |

- At 64 at once, LRU costs that row 1–3% in p50 and 1.6–2.6% in round time, with or without the
  copies. One at a time and at 8 at once, nothing shows.
- Without inserts on writes, the row's p50 was 4% under `main`'s, with the same round time. But
  then an edit fetches its shard from the bucket again: in the same runs, append and truncate
  4 KiB in 64 MiB took 50 ms instead of 36.
- Inserting from a blocking thread took as long as inserting inline. So the inserts stay on the
  request path. An async cache would not move the upkeep either: moka 0.12 does it in the calling
  task too.

## Memory

The cache weighs a shard by its length. But a shard cut from an upload by `StreamChunker` is a
slice of the chunker's buffer, and a slice keeps its whole allocation alive. For a 16 MiB put,
all its shards share one buffer of about 32 MiB. With macOS's allocation logging
(`MallocStackLogging=1`, `malloc_history`), `main`'s live allocations after 48 such puts were
buffers from `StreamChunker::push` (`BytesMut::reserve_inner`) of 33 MB on average, each kept by
the shards it had given.

So the branch copies each shard and page as it goes into a cache (`cached` in `pool.rs`). The copy
is one memcpy of the bytes the cache takes in, much less than hashing them, which the write
already does. Measured with one voidfs-server on a local-disk store (`--store fs:`), the default
512 MiB cache, and 16 MiB puts one after another into one drive, idle afterwards (`vmmap
--summary`):

| | Allocated | Footprint |
|---|--:|--:|
| 48 puts of fresh random data: `main` | 1.1 GB | 0.80 GB |
| the same, LRU without copies | 1.2 GB | 0.80 GB |
| the same, this branch | 0.63 GB | 1.4 GB |
| the same, cache off (`--cache-mib 0`) | 0.002 GB | 0.84 GB |
| 96 puts, each the same 16 MiB but for its first bytes: LRU without copies | 3.0 GB | 1.5 GB, growing 16 MB a put |
| the same, this branch | 0.25 GB | 0.97 GB, the same after 64 puts and 96 |

- **Fresh data.** Without copies, the cache's 512 MiB of shards held twice that. With them, what
  is allocated is the cache and little else.
- **Versions of one file.** Each put shares all its shards but the first with the others (the
  chunking is content-defined), so it adds one shard to the cache, and without copies that shard
  keeps its put's whole buffer. Allocated memory grew by a buffer a put, towards 256 × 33 MB for a
  full cache. This is what saving a large file over and over does. With copies it stays flat.
- **Footprint** also counts freed memory that the allocator keeps: the run with the cache off
  had 0.84 GB with nothing allocated. With copies, the chunker's buffers are freed after each put
  and join that memory, which is why the first case's footprint is higher on the branch. It is
  the allocator's, not the cache's, and it did not grow from 64 to 96 puts.
- Peak resident size of the server during the full runs: 3.59 and 3.57 GB for `main`, 2.49 and
  2.28 GB for this branch, and 3.33 GB for a build without copies (the first round, below).
  These include request bodies in flight.
- The chunker's buffers themselves are an ingest cost, for step 3, item 3: after each shard it
  cuts, the next `reserve` finds the buffer shared and moves the rest of it (up to 16 MiB) to a
  new allocation. Not measured here.

The first round of the probe and full runs used the build without copies: rows at or ahead of
SpaceFS's ratio went from 16 and 19 to 21 and 22, with the same fan-out and edit rows moving by
the same amounts.
