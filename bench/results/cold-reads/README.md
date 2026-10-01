# Cold reads, and a bucket with S3's bandwidth

*1 October 2026, one Mac (Apple M5 Pro, 15 CPUs, macOS 27). versitygw 1.8.0 as the bucket, 12 ms
away through `BENCH_ONE_WAY_MS=4 bench/scripts/local.sh`, with voidfs-server's 512 MiB cache.
Step 3, item 7.1 of the [parity plan](../../../docs/PARITY.md#7-step-by-step-plan)
([step-3-performance.md](../../../docs/step-3-performance.md#item-7-measurement-gaps-to-close-first)):
the harness only measured warm reads, because voidfs-server could not drop its caches; SpaceFS
publishes cache-cleared figures for four read rows. Now `SIGUSR1` empties the server's caches,
and the harness's `--cold` drops them before every wave of operations. And item 7.4, new: the
relay that puts the bucket 12 ms away added no bandwidth limit, so the bare bucket read 64 MiB in
about 120 ms where S3 took 779 ms in SpaceFS's run. `BENCH_BANDWIDTH=s3` caps it as S3 was
there.*

| Files | voidfs-server | Harness | Bucket |
|---|---|---|---|
| `warm12-main-*` | `main` at `202c38a` | `main` | 12 ms, no cap, warm |
| `warm12-branch-*` | This branch | `main` | 12 ms, no cap, warm |
| `cold-nocap-*` | This branch | This branch | 12 ms, no cap, cold (the read rows) |
| `cold-s3-*` | This branch | This branch | 12 ms, `BENCH_BANDWIDTH=s3`, cold |
| `cold-pertotal-*` | This branch | This branch | 12 ms, `BENCH_BANDWIDTH=95/68/-` (no total), cold |
| `cold-s3-noinline-*` | This branch | This branch | As `cold-s3-*`, in a pool without `inline-data` (the 4 KiB rows) |
| `capped-branch-*` | This branch | This branch | 12 ms, `BENCH_BANDWIDTH=s3`, warm, all 49 rows |
| `fit-s3` | – | This branch | 12 ms, `BENCH_BANDWIDTH=s3`, the bare bucket alone (`--targets bare`) on the rows that move the most data |

Every run recorded voidfs's own requests to the bucket (`BENCH_BUCKET_REQUESTS=1`), and the
server's metrics were scraped every 2 seconds. Each server binary was built with `cargo build
--release -p voidfs-server`, in a target directory of its own (`main` in a separate worktree), and
passed with `BENCH_SERVER_BIN`; their hashes differ, and only the branch's has
`voidfs_cache_drops_total` and `SIGUSR1: dropped the caches` in it (`strings`). All runs set
`BENCH_POOL_FEATURES=inline-data`, except `cold-s3-noinline-*`. The `main` harness ran from a
worktree of `main`; this branch's from a worktree of `202c38a` with the branch's changes applied,
so those files' `commit` label says `202c38a (with uncommitted changes)`.

Run order:
1. The bare bucket alone with the cap, on the rows that move the most data, to fit the cap.
   The first such run, with a relay that lost about 10% of its rate once eight connections shared
   the total (fixed, below), is not kept.
2. The 49 rows at 12 ms without the cap, warm, `main` against the branch: A B B A.
3. The ten read rows cold at 12 ms: no cap, the cap, the cap, no cap; then the cap without its
   total, twice; then the two 4 KiB rows in a pool without `inline-data`, twice.
4. Focused, A B B A B A A B, `main` against the branch, warm, on the rows the full runs showed
   slower: four at four times the operations, then multipart put 256 MiB × 16 MiB.
5. The 49 rows at 12 ms with the cap, warm, twice.

No run had errors. The focused runs' files are not kept; the tables below have their figures.

## What was built

**voidfs-server** ([pool.rs](../../../crates/voidfs-server/src/pool.rs),
[main.rs](../../../crates/voidfs-server/src/main.rs)):
- `Pool::drop_caches` empties the shard cache and the page cache (manifest pages and checkpoint
  segments), and counts the drop in `voidfs_cache_drops_total`.
- `SIGUSR1` calls it. The handler is installed before the pool opens, so that a signal sent while
  it opens does not stop the process, which is what SIGUSR1 does by default.
- Nothing on the S3 port or the admin port can drop the caches: an endpoint on the
  unauthenticated admin listener would let anyone who reaches it make every read go to the bucket.
  Only someone who may signal the process can.

What stays in memory, and why:
- **Each drive's state:** its namespace, versions and content descriptors, and with `inline-data`
  the bytes of files up to 4 KiB, which live in the drive's log and its descriptors, not in a
  cache. A server that starts loads the same state from its checkpoints and log before it serves.
  So a cold read of a 4 KiB file in an `inline-data` pool reads nothing from the bucket.
- **The guard's record of which shards are known to be stored** (format §12.4): writes consult
  it, to skip an upload; reads never do.
- **The connections to the bucket**, as a running server keeps them.
- **The S3 server's own disk cache** (macOS's page cache under versitygw), for both targets.
  Dropping it needs `sudo purge`, which was not run. With the cap, a read's time is set by the
  relay, not the disk.

**The harness** ([run.rs](../../../crates/voidfs-bench/src/run.rs),
[report.rs](../../../crates/voidfs-bench/src/report.rs)): `--cold`, with `--voidfs-pid` and
`--voidfs-metrics`; `BENCH_COLD=1` in `local.sh`, and in `cloud-run.sh` for the `client-host`
topology.
- Each round runs in **waves of one operation per worker**, a wave starting once the one before
  has ended. Before each of voidfs's waves the harness sends `SIGUSR1` and waits until the
  server's `voidfs_cache_drops_total` goes up; a drop not confirmed within 5 seconds ends the
  round with an error. If the server's metrics have no such series (an older server, which
  SIGUSR1 would stop), nothing is sent.
- The bare target runs the same waves, with nothing to drop.
- Each round records its waves and drops; the scenario records the seconds voidfs's requests to
  the bucket took, by kind, from which the mean number in flight follows.
- A cold run's table sets voidfs against SpaceFS's cache-cleared figures, and has none where they
  give none. Files without these fields read as warm runs.

**The relay** ([delay.rs](../../../crates/voidfs-bench/src/delay.rs)): `voidfs-bench delay
--bandwidth`, `BENCH_BANDWIDTH` in `local.sh`. Off by default, so runs without it are as before.
- Each chunk is sent once a link at the rate would have carried it, after the chunks before it,
  on top of the delay: each connection has its own link each way, and all connections share one
  more of the total rate each way. Each connection's time is reckoned from when its link was
  free, not from when the timer fired, so the timer's late wake-ups do not slow it down.
- The shared link takes whole chunks (up to 256 KiB) in the order connections ask. In the first
  version a small chunk then waited for other connections' large ones: eight connections that
  together asked for 760 MB/s of a 1,000 MB/s total each got 87 MB/s of their 95. It now may run
  up to 5 ms ahead of its rate. Measured against Python's `http.server` serving a 64 MiB file:
  one connection 0.718–0.721 s (8 ms of delay plus 64 MiB at 95 MB/s: 0.714 s); eight at once
  0.717–0.719 s each; sixteen at once 1.04–1.10 s, where 1,074 MB at 1,000 MB/s takes 1.07 s.

## What "cold" means here

SpaceFS's benchmark page describes its cache-cleared figure as the same read with the cache
cleared, for workloads that read each object once (read again on 1 October 2026); neither it nor
the methodology page says when the cache was cleared. This repository had not recorded it
either: `bench/README.md` reading 10 only said the figures were not reproduced. A get row's
eight readers share one object, so clearing once per round would leave all but its first eight
operations warm, and SpaceFS's cold p50s are not warm (get 64 MiB: 299 ms cold, 47.2 warm). The
definition chosen: **every measured operation starts with voidfs-server's caches empty**, and
otherwise runs as in the warm rows: the eight readers of a wave read the one object together.
Whether SpaceFS's eight readers shared one object when cold is not published either. If they
did, coalescing would have spared them the eight-fold fetch that holds voidfs back here (below);
if each read its own, they fetched as much as voidfs does here.

## The bandwidth cap

`BENCH_BANDWIDTH=s3`: **95 MB/s down and 68 MB/s up per connection, 1,000 MB/s in all each way.**
Fitted by hand to SpaceFS's bare-bucket figures, never to voidfs's: downloads to get and stream
get, uploads to put 32 and 64 MiB, the total to the two multipart uploads, which are the only
bare rows that send more than eight streams at once (up to 64). The edits in 32 and 64 MiB files,
a download and an upload each, were not used to fit it, and check it.

The bare bucket's p50 at 12 ms with the cap, the median of three runs (`fit-s3`,
`capped-branch-1` and `-2`), against SpaceFS's bare bucket:

| Row | SpaceFS's bare bucket (ms) | Here, capped (ms) | Here / SpaceFS |
|---|--:|--:|--:|
| **Fitted: downloads** | | | |
| get 32 MiB | 356 | 370 | 1.04 |
| get 64 MiB | 779 | 728 | 0.93 |
| stream get 64 MiB | 697 | 721 | 1.03 |
| stream get 256 MiB | 2,772 | 2,840 | 1.02 |
| **Fitted: uploads** | | | |
| put 32 MiB | 523 | 519 | 0.99 |
| put 64 MiB | 978 | 1,024 | 1.05 |
| **Fitted: the total** | | | |
| multipart put 64 MiB × 8 MiB | 660 | 634 | 0.96 |
| multipart put 256 MiB × 16 MiB | 1,790 | 2,022 | 1.13 |
| **Not fitted: edits, a download and an upload** | | | |
| append 4 KiB to 64 MiB | 1,668 | 1,751 | 1.05 |
| truncate 4 KiB, end of 64 MiB | 1,677 | 1,752 | 1.04 |
| delete 4 KiB, middle of 64 MiB | 1,718 | 1,751 | 1.02 |
| insert 4 KiB, middle of 64 MiB | 1,667 | 1,753 | 1.05 |
| append 4 KiB to 32 MiB | 874 | 891 | 1.02 |
| delete 4 KiB, start of 64 MiB | 1,685 | 1,754 | 1.04 |
| insert 4 KiB, start of 64 MiB | 1,690 | 1,752 | 1.04 |
| write at 4 KiB in 64 MiB | 1,667 | 1,751 | 1.05 |
| truncate 4 KiB, end of 32 MiB | 860 | 890 | 1.04 |
| write at 4 KiB in 32 MiB | 873 | 890 | 1.02 |
| delete 4 KiB, middle of 32 MiB | 861 | 892 | 1.04 |
| delete 4 KiB, start of 32 MiB | 859 | 891 | 1.04 |
| insert 4 KiB, middle of 32 MiB | 883 | 891 | 1.01 |
| insert 4 KiB, start of 32 MiB | 890 | 892 | 1.00 |
| patch 16 × 4 KiB in 64 MiB | 1,662 | 1,752 | 1.05 |
| patch 16 × 4 KiB in 32 MiB | 899 | 892 | 0.99 |
| **Not modelled: S3's time per request, server-side copy** | | | |
| get 4 KiB | 25.5 | 13.9 | 0.54 |
| head | 12.1 | 13.5 | 1.11 |
| get 1 MiB | 37.5 | 23.2 | 0.62 |
| range 64 KiB of 64 MiB | 44.1 | 13.0 | 0.30 |
| put 4 KiB | 30.1 | 14.1 | 0.47 |
| put 1 MiB | 72.8 | 28.0 | 0.38 |
| rename 64 MiB | 858 | 123 | 0.14 |

- Within −7% to +13% on every row the cap was fitted to, and −1% to +5% on the edits it was not.
- Get 64 MiB and stream get 64 MiB move the same bytes, and SpaceFS's bare bucket took 779 and
  697 ms for them: their own runs differ by 12% there, so no single rate fits both closer.
- Multipart: 64 MiB in 8 MiB parts fits (64 parts at once, 537 MB at 1,000 MB/s), and 256 MiB in
  16 MiB parts is 13% slow. A total of about 1,200 MB/s would fit the second and make the first
  25% fast. **Not confirmed:** whether S3's limit there was the client machine's, the network's
  or S3's, and whether SpaceFS's harness sent all parts at once as this one does.
- **Not confirmed either: the total for downloads.** No bare row of SpaceFS's downloads more
  than eight streams at once, so the 1,000 MB/s is the uploads' figure assumed for downloads.
  What their client machine could take in from S3 in another cloud is not published. It matters
  for cold reads (below).
- **What the cap does not emulate:** S3's own time per request, which the last rows show: the
  bare bucket's small requests take about half as long here as in SpaceFS's run, and so do
  voidfs-server's own requests to the bucket. Nor server-side copies: rename 64 MiB (CopyObject
  and DeleteObject) takes 123 ms here and took 858 ms on S3. `head` is 1.4 ms slower here: the
  relay's timer.

## The four rows SpaceFS gives cold figures for

Cold, at 12 ms, two runs each; p50 in milliseconds, and voidfs's time as a fraction of the bare
bucket's in the same run (lower is better; SpaceFS's fraction is theirs to their bare bucket):

| Row | voidfs, capped | Bare, capped | voidfs / bare, capped | Without the total | No cap | SpaceFS cold / bare | SpaceFS warm / bare |
|---|--:|--:|--:|--:|--:|--:|--:|
| get 4 KiB | 0.3, 0.3 | 13.9, 14.0 | 0.02, 0.02 | 0.02, 0.02 | 0.02, 0.02 | 1.08 (27.5 / 25.5) | 0.04 |
| get 4 KiB, no `inline-data` | 12.9, 13.1 | 13.5, 13.4 | 0.96, 0.98 | | | 1.08 | 0.04 |
| get 1 MiB | 25.8, 25.9 | 25.0, 24.8 | 1.03, 1.04 | 1.04, 1.03 | 1.05, 1.04 | 1.18 (44.1 / 37.5) | 0.22 |
| get 32 MiB | 268.7, 276.0 | 371.5, 372.9 | 0.72, 0.74 | 0.32, 0.39 | 1.16, 1.18 | 0.50 (179 / 356) | 0.09 |
| get 64 MiB | 516.6, 505.7 | 728.4, 730.8 | 0.71, 0.69 | 0.30, 0.29 | 1.12, 1.16 | 0.38 (299 / 779) | 0.06 |

- **The small rows are ahead of SpaceFS's cold figures.** A 4 KiB file in an `inline-data` pool
  is in the drive's state, so a cold read is as fast as a warm one; in a pool without it, a cold
  read is one shard GET, 0.96–0.98× the bare bucket's time (SpaceFS 1.08×). Get 1 MiB is one
  shard GET too, 1.03–1.05× (SpaceFS 1.18×).
- **The large rows are behind with the cap,** 0.69–0.74× the bare bucket's time against
  SpaceFS's 0.38× and 0.50×, **and ahead of SpaceFS without the cap's total,** 0.29–0.39×. The
  total is the cap's one unconfirmed figure, and it binds because of how voidfs fetches (next
  section): the eight readers of a wave each fetch every shard of the object, 8 × 64 MiB = 537 MB,
  which at 1,000 MB/s takes 537 ms. voidfs took 506–517.
- **With no cap,** the bare bucket reads 64 MiB in 138–141 ms, and voidfs's cold reads take
  1.12–1.18× that: there is no per-stream limit for parallel fetches to beat, and voidfs adds the
  hop through the server. This is why finding 7 said large reads cannot be judged locally.

## The other reads, cold

| Row | voidfs / bare, capped | Without the total | No cap |
|---|--:|--:|--:|
| stream get 64 MiB | 0.66, 0.70 | 0.29, 0.29 | 1.03, 1.06 |
| stream get 256 MiB | 0.68, 0.63 | 0.26, 0.27 | 0.90, 0.91 |
| range 64 KiB of 64 MiB | 2.98, 2.77 | 3.04, 3.32 | 1.29, 1.25 |
| fan-out get 200 × 256 KiB, 32 at once | 1.05, 1.06 | 1.08, 1.03 | 1.12, 1.04 |
| fan-out get 1,000 × 4 KiB, 32 at once | 0.03, 0.03 (0.97, 1.00 without `inline-data`) | 0.03, 0.03 | 0.03, 0.03 |
| fan-out get 1,000 × 4 KiB, 64 at once | 0.04, 0.04 (1.00, 1.00 without `inline-data`) | 0.04, 0.04 | 0.04, 0.04 |

SpaceFS gives no cold figure for these. The streams behave as the gets. **A cold 64 KiB range
takes about three times the bare bucket's time with the cap** (41–48 ms against 14–15): it
fetches the whole shard the range falls in, about 2.5 MiB, at 95 MB/s, where the bare bucket
sends 64 KiB.

## Requests to the bucket per cold read

From the server's metrics over the measured rounds (`cold-s3-*`, `cold-nocap-*`,
`cold-pertotal-*`). "In flight" is the time voidfs's GETs to the bucket took, added up, over the
wall-clock time of its rounds (drops included): the mean number at once.

| Row | Shard GETs per read | In flight, mean (at most) |
|---|--:|--:|
| get 64 MiB | 22.5–31.6 | 34–41 (64) |
| get 32 MiB | 11.2–15.8 | 34–42 (64) |
| stream get 64 MiB | 18.6–29.9 | 29–41 (64) |
| stream get 256 MiB | 70.8–108.5 | 34–43 (64) |
| get 1 MiB, range 64 KiB, fan-out 256 KiB | 1.00–1.03 | 5–7 (8); 22 at 32 at once |
| get 4 KiB, fan-out 4 KiB | 0 (1 without `inline-data`) | – |

- A 64 MiB object is about 25 shards of 2–3 MiB (content-defined, so the count differs from run
  to run). **Each read fetches about every shard of the object, so the eight readers of a wave
  fetch it eight times over:** `Pool::shard` fetches on every miss, and nothing coalesces
  concurrent misses of one shard.
- **Each read has at most 8 shard GETs in flight** (`object.rs`, `.buffered(8)`): 64 across the
  wave, and 34–43 on average over a round, since a wave's last reads finish alone.
- At 95 MB/s per connection, eight GETs in flight give one read about 760 MB/s, eight times one
  stream's: that is what puts voidfs ahead of the bare bucket on large reads under S3's per-stream
  limit, and ahead of SpaceFS where the total does not bind.

What this says for item 6, in order:
1. **Coalesce concurrent misses** of one shard. The eight readers of a cold object would fetch it
   once, not eight times: 67 MB instead of 537 per wave of get 64 MiB, so the total would no
   longer bind. At eight GETs in flight and 95 MB/s each, one 64 MiB read would take roughly
   100–130 ms, about 0.15× the bare bucket's time, against SpaceFS's 0.38×. An estimate, not
   measured.
2. **Fetch only what a range needs:** a ranged GET of the shard object, rather than the whole
   shard, for a cold range: the range row is at about 3× the bare bucket's time.
3. **A wider window per request** matters once reads are coalesced: one read is then held to
   8 × 95 MB/s, and S3's per-request limit, not the total, would set it.
4. Read-ahead and the disk tier: nothing here measures them.

## The 49 rows with the cap

`capped-branch-1` and `-2`: warm, all 49 rows, at 12 ms with `BENCH_BANDWIDTH=s3`. **46 of the
49 rows are at or ahead of SpaceFS's ratio in both runs**: all 24 edits, all 11 writes, 7 of the
10 reads and the 4 metadata rows. 41 and 42 are faster than the bare bucket, and the geometric
mean speed-up over it is 5.9× (SpaceFS's: 2.8×). Without the cap, in the same session, 27–28
rows were at or ahead (below).

**Still behind,** in both runs:

| Row | voidfs (ms) | Bare (ms) | Speed-up | SpaceFS | Of SpaceFS's ratio |
|---|---|---|---|--:|--:|
| get 64 MiB | 56.5, 55.5 | 728, 727 | 12.9×, 13.1× | 16.5× (47.2 / 779) | 0.79 |
| stream get 256 MiB | 203, 212 | 2,840, 2,840 | 14.0×, 13.4× | 15.6× (178 / 2,772) | 0.88 |
| stream get 64 MiB | 49.5, 52.9 | 721, 720 | 14.6×, 13.6× | 15.5× (45.1 / 697) | 0.91 |
| get 32 MiB, level | 30.9, 30.1 | 369, 370 | 11.9×, 12.3× | 11.8× (30.3 / 356) | 1.03 |

These are warm reads, served from voidfs-server's memory: 64 MiB in 50–57 ms, eight at once,
where SpaceFS's took 45–47 ms. Their bare bucket is also 7% slower than the cap's for get 64 MiB
(779 ms against 728), which counts against voidfs here. What is left is the server's own
throughput from its cache to the client, about 9.5 GB/s for eight readers on this Mac. The real
run, on an 8-vCPU VM, will judge it.

**What the cap moved,** against the same branch without it:

| Scenarios | Rows | Without the cap | With the cap | SpaceFS |
|---|--:|---|---|---|
| Small, ranged and cached reads, `head`, fan-out gets | 7 | 12–52× faster | 12–53× faster | 2.6–34× faster |
| Large gets and streams | 4 | 2.3–3.0× faster | 12–14× faster | 12–17× faster |
| Edits inside 32 and 64 MiB files | 16 | 1.5–8.3× faster | 3.4–37× faster | 1.4–15× faster |
| Rename and folder move | 2 | 8.3–31× faster | 9.5–28× faster | 7.9–18× faster |
| Listing | 1 | 32× faster | 42× faster | 9.1× faster |
| Edits inside 1 MiB files | 8 | 1.3–1.4× slower | 1.1–1.2× faster | 2.1× slower to parity |
| Whole-object puts and overwrites, fan-out puts | 9 | 1.1–2.8× slower | 2.1× slower to 1.8× faster | 1.1–3.1× slower |
| Multipart uploads | 2 | 1.3× slower to 1.3× faster | 1.0× faster | 1.8–2.4× slower |
| **All 49**: faster in / geometric mean | | 31 / 3.0× | 42 / 5.9× | 31 / 2.8× |

(The medians of each row over the two runs, grouped as in
[PARITY.md §6](../../../docs/PARITY.md#6-performance); "without the cap" is `warm12-branch-*`.)

- **The large reads** are 12–14× faster than the bare bucket, from 2.3–3.0×: the bare bucket
  now reads at S3's rate, and voidfs's warm reads do not touch it.
- **The edits in 32 and 64 MiB files** are 3.4–37× faster: the bare bucket downloads and uploads
  the whole file at S3's rates (0.9 and 1.75 s), and voidfs uploads the one or two shards it
  rewrites.
- **The edits in 1 MiB files** went from 1.3–1.4× slower to 1.1–1.2× faster: the bare bucket now
  moves the file down at 95 MB/s and up at 68 (about 26 ms of the 50), where voidfs uploads one
  shard of about 1 MiB (45 ms in all, from about 36 without the cap).
- **Put 32 and 64 MiB** are 1.7–1.8× faster than the bare bucket, where SpaceFS's were
  1.1–1.2× slower: voidfs uploads a body's shards up to 16 at a time, so one put is not held to
  one stream's 68 MB/s as the bare bucket's single PUT is. Eight 64 MiB puts at once then share
  the 1,000 MB/s total: 575 ms. Multipart uploads are at parity, where SpaceFS's were 1.8–2.4×
  slower.
- **Put and overwrite 1 MiB** crossed SpaceFS's ratio: 0.63–0.64× the bare bucket's speed, from
  0.41–0.43× (SpaceFS 0.52× and 0.58×). Both sides now add the same 15 ms to upload 1 MiB, which
  shrinks the share of voidfs's extra round trip. The 4 KiB puts and the fan-out puts were
  already ahead, and stayed ahead; the fan-out puts slowed down a little (16 to 18–21 ms), since a
  log entry holding 64 small files is a 256 KiB upload.
- The emulation favours neither side on small requests: S3's time per request is missing for
  both, the bare bucket's and voidfs-server's own. It does favour the bare bucket on rename, whose
  CopyObject is far faster here than on S3.

## Warm: what the server change costs

`warm12-main-1`, `warm12-branch-1`, `warm12-branch-2`, `warm12-main-2`, without the cap,
compared in the same position (`main-1` with `branch-2`, `branch-1` with `main-2`): relative to
the bare bucket in the same run, the geometric mean of the p50 ratios over the 49 rows is
**0.999** (median 0.998). By family: edits 0.993, metadata 1.109, reads 0.993, writes 0.979.

| Run | Rows at or ahead of SpaceFS's ratio (edits) | Rows faster than the bare bucket |
|---|--:|--:|
| `warm12-main-1` | 28 (12) | 32 |
| `warm12-branch-1` | 27 (11) | 31 |
| `warm12-branch-2` | 28 (12) | 31 |
| `warm12-main-2` | 26 (10) | 30 |

Rows that looked slower in both positions, run focused again (A B B A B A A B, `main` against
the branch, four times the operations, multipart at the default):

| Scenario | Full runs, relative | Focused, relative | voidfs's own p50, `main` → branch | Each pair, own |
|---|--:|--:|---|---|
| list 200 keys | +23.1% | −4.2% | 0.9 → 0.9 ms | +1% +3% −6% −6% |
| insert 4 KiB, middle of 32 MiB | +13.3% | +4.8% | 37.7 → 39.2 ms | +22% −1% +2% −10% |
| delete 4 KiB, start of 32 MiB | +7.6% | +4.2% | 34.4 → 34.8 ms | −1% +14% −0% +3% |
| patch 16 × 4 KiB in 1 MiB | +5.8% | −1.5% | 35.5 → 34.3 ms | −1% −2% −5% −2% |
| multipart put 256 MiB × 16 MiB | +38.4% | +6.6% | 953 → 948 ms | +8% −5% +1% −5% |

- In the full runs, list's own p50 was 1.0–1.1 ms in all four; the bare bucket's moved (38.3 and
  40.8 ms in `main`'s runs, 33.3 and 33.9 in the branch's). Multipart's bare bucket took 680–1,051
  ms, and 2,118 in one focused run.
- Focused, voidfs's own times moved −3.1% to +4.1%, with the pairs disagreeing in sign: within
  what one binary does against itself here (6% at 12 ms, #16). The change adds no code to the
  request path: a task waiting for a signal, and a counter. No diagnostic binary was built.

## Tests

Each seen to fail with the code it guards broken, running only that test, with a timeout (21
ways):
- **The drop** (`pool.rs`): a file of 1,100 shards with manifest pages, read warm with no request
  to the bucket; after the drop both caches hold nothing, the drop is counted, and reads return
  the same bytes with each shard and page fetched once and nothing else; the drive's state stays,
  and a shard stored already is not uploaded again. Broken: nothing invalidated, the pages left,
  the drop not counted.
- **SIGUSR1** (`main.rs`): sent to the test's own process, it empties the caches of the pool the
  handler serves. Broken: the handler not dropping.
- **Waves** (`run.rs`): each wave starts after a drop, once the one before has ended, with one
  operation per worker; every operation runs once; a failed drop stops the round; without
  `--cold` nothing is dropped. Cold rounds run in waves for every target and drop only voidfs's
  caches. Broken: no drop before a wave, waves run together, a worker running two in a wave, a
  failed drop ignored, cold rounds run steady, the bare target dropped too.
- **The drop's handshake** (`run.rs`, against a stand-in for the server that counts each
  SIGUSR1 50 ms late): a drop returns only once counted, and nothing is sent to a server whose
  metrics cannot confirm it. Broken: returning before it is counted, signalling first.
- **The report** (`report.rs`): a cold run is set against SpaceFS's cache-cleared figure where
  they give one, and has none elsewhere; files written before read as warm runs. Broken: the cold
  figure ignored, `cold` written into warm files.
- **The relay** (`delay.rs`): the delay alone adds 2 × 20 ms to a round trip and nothing to the
  rate; one connection is held to 40 MB/s down and 20 MB/s up within −10% and +25%, and its first
  byte still waits a round trip; three connections share a 60 MB/s total. Broken: no per-connection
  rate, up and down swapped, no delay under a cap, no delay, no total, a total per connection.

`cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass, and
`tests/interop/run.sh` over `memory`, `fs` and `versitygw`: 38 conformance cases in both
addressing styles, the rclone checks, aws-chunked, the admin listener and a graceful SIGTERM. The
boto3 checks were skipped: boto3 is not installed on this Mac (CI runs them).

Not covered by a test: the relay's 5 ms burst on the shared link, which was measured by hand
(above) rather than in a unit test, since the loss it fixes depends on chunk sizes that a test's
upstream does not reliably produce.

## What is left

- **Item 6,** now with evidence: coalesced shard fetches first, then ranged shard reads, then the
  window.
- **The download total** of the real setup. The real run in SpaceFS's setup (item 7.2), cold
  (`BENCH_COLD=1 BENCH_TOPOLOGY=client-host bench/scripts/cloud-run.sh`), would settle whether
  voidfs's cold large reads are ahead of SpaceFS's or behind.
- **S3's time per request** is not emulated: small requests and server-side copies are faster
  here than on S3, for both targets.

## Caveats

- One machine: the harness, versitygw, the relay and the server share 15 CPUs and one disk.
- The cap is a model of S3 fitted to one published run of SpaceFS's; its total is fitted to the
  two multipart rows alone, and assumed for downloads.
- The relay adds its delay each way with a timer, about 14–15 ms a round trip in all.
- versitygw's disk cache stayed warm in cold runs, for both targets.
- SpaceFS's figures are from their cloud run, not this setup; the comparison is of each row's
  ratio to the bare bucket.
