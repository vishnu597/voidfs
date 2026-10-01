# Benchmarks: voidfs against the bare bucket

SpaceFS publishes [49 benchmark scenarios](https://docs.spacefs.com/benchmarks/), each run on
one bucket twice: through their layer, and against the bare bucket. This directory ports them to
voidfs, which is step 1 of the [parity plan](../docs/PARITY.md#7-step-by-step-plan) and checklist
item O5.

| Path | What |
|---|---|
| [`crates/voidfs-bench`](../crates/voidfs-bench/) | The harness: the 49 scenarios, both targets, JSON and Markdown results |
| [`scripts/local.sh`](scripts/local.sh) | One machine, no cloud account: a local S3 server, voidfs-server over it, the harness |
| [`scripts/serve.sh`](scripts/serve.sh), [`scripts/cloud-run.sh`](scripts/cloud-run.sh) | The real run: voidfs-server, and the harness on the client VM |
| [`cloud/iam-policy.json`](cloud/iam-policy.json) | The one-bucket IAM policy for the real run |
| [`results/`](results/) | Result files, one JSON and one Markdown table per run; [`results/probes/`](results/probes/) holds the runs that isolate the causes in the findings |

## Run it on one machine

You need Rust and a local S3 server. [versitygw](https://github.com/versity/versitygw) is the
default (`brew install versitygw`). MinIO also works where it runs (`BENCH_S3=minio`); the
Homebrew build of 2025-10-15 crashes at startup on an Apple M5 under macOS 27, in its
CPU-detection dependency.

```bash
bench/scripts/local.sh
```

That builds release binaries, starts the S3 server and voidfs-server on loopback with throwaway
keys, runs all 49 scenarios (about 90 seconds on an M5 Pro), writes
`bench/results/<name>.{json,md}` and removes the data. Extra arguments go to the harness:

```bash
bench/scripts/local.sh --scenario append --ops-scale 0.25
```

On loopback the bucket answers in well under a millisecond, which hides every cost that comes
from talking to a real bucket. `BENCH_ONE_WAY_MS` puts a delay between the bucket and both
voidfs-server and the harness's bare target. The harness still reaches voidfs-server directly,
as SpaceFS's reached its layer on the client host.

```bash
BENCH_ONE_WAY_MS=4 bench/scripts/local.sh
```

A setting of 4 gives a 12 ms round trip, which is the bare bucket's `head` time in SpaceFS's
run. The relay's timer adds about 2 ms each way on macOS, so the setting is not the round trip;
the bare `head` row of each run shows the real figure.

The relay adds no bandwidth limit unless `BENCH_BANDWIDTH` sets one, so on its own the bare
bucket reads 64 MiB in about 120 ms where S3 took 779 ms in SpaceFS's run. `BENCH_BANDWIDTH=s3`
caps it as S3 was in their run, for voidfs-server's traffic and the bare target's alike, in both
directions, on top of the delay: 95 MB/s down and 68 MB/s up per connection, and 1,000 MB/s in
all each way. S3 limits one stream well below what a client machine takes in all, hence the two
levels. `DOWN/UP/TOTAL` in MB/s sets other rates (`-` for no limit), for example `95/68/-`.

```bash
BENCH_ONE_WAY_MS=4 BENCH_BANDWIDTH=s3 bench/scripts/local.sh
```

The `s3` rates are fitted to SpaceFS's bare-bucket figures for the rows that move the most
data, with the bucket 12 ms away: within −7% to +13% of each, and within −1% to +5% on the edits
in 32 and 64 MiB files, which were not used to fit them
([fit, row by row](results/cold-reads/README.md#the-bandwidth-cap)). The total is fitted to the
multipart uploads alone, and that it holds for downloads too is an assumption. The cap does not
emulate S3's own time per request: small requests (get and put 4 KiB, put 1 MiB), the bare
target's and voidfs-server's alike, and server-side copies (rename) stay faster here than in
their run.

The harness counts its own requests, not the ones voidfs-server sends the bucket for them.
`BENCH_BUCKET_REQUESTS=1` starts the server with its admin listener, and the harness reads the
server's metrics before and after each scenario's measured rounds: the results then have
voidfs's requests to the bucket per operation, by kind (`--voidfs-metrics <url>` does the same
for a server of your own).

`BENCH_COLD=1` measures cold reads (`--cold`; [How it measures](#how-it-measures), reading 10):
voidfs-server's caches are dropped before every wave of operations with `SIGUSR1`, and each drop
is confirmed from its metrics, so it also starts the admin listener.

```bash
BENCH_ONE_WAY_MS=4 BENCH_BANDWIDTH=s3 BENCH_COLD=1 bench/scripts/local.sh --scenario get- --scenario range-64k --scenario fanout-get
```

## The harness

```bash
cargo run --release -p voidfs-bench -- list                 # the scenarios, with SpaceFS's figures
cargo run --release -p voidfs-bench -- run --help           # targets, filters and settings
cargo run --release -p voidfs-bench -- report results.json  # re-render a table
cargo run --release -p voidfs-bench -- delay --one-way-ms 4 # the relay local.sh uses
```

`run` needs two targets, given as flags or environment variables:
- **voidfs:** the server's endpoint and an admin key (`VOIDFS_ENDPOINT`, `VOIDFS_ACCESS_KEY_ID`,
  `VOIDFS_SECRET_ACCESS_KEY`). Each scenario gets a fresh drive, which is hard-deleted after it.
- **bare:** the bucket the server's pool lives in, reached directly (`VOIDFS_S3_BUCKET`,
  `VOIDFS_S3_ACCESS_KEY_ID`, `VOIDFS_S3_SECRET_ACCESS_KEY`, `VOIDFS_S3_REGION`, and
  `VOIDFS_S3_ENDPOINT` for anything but AWS). These are the variables voidfs-server itself reads,
  so one environment describes both. Keys go under `voidfs-bench/bare/<run>/<scenario>/` and are
  deleted after each scenario.

The harness never prints values it read from the environment, not even in `--help`. With
`--redact` it also leaves endpoints and bucket names out of the results and the log.

Each run writes `<name>.json`, with every round's p50, p90, p99, mean, extremes, errors,
throughput and the requests and bytes each target moved, and `<name>.md`, a table in SpaceFS's
format with their published result on every row. The JSON is rewritten after every scenario, so
an interrupted run keeps what it measured. `--samples` also records every latency.

## How it measures

### What follows SpaceFS

| | SpaceFS's run (20 September 2026) | This harness |
|---|---|---|
| Comparison | One bucket, through the layer and directly, back to back from one machine | The same: voidfs's pool is `voidfs-bench/pool/` of the bucket the bare side writes to |
| Client library | Rust `aws-sdk-s3` 1.148.0, 64 connections, keep-alive | `aws-sdk-s3` 1.148.0 (pinned) for every standard call, on both targets; at most 64 requests in flight per target. voidfs's extensions are SigV4-signed requests, as the [conformance runner](../crates/voidfs-conformance/src/runner.rs) sends them |
| Shape | 3 warm-up operations, 2 rounds, 8 at once unless the scenario says 32 or 64 | The same |
| Figure | Median of each round's p50, in milliseconds | The same |
| Sizes | 4 KiB, 1 MiB, 32 MiB, 64 MiB; 256 MiB for multipart and streaming | The same |
| Bare-bucket edits | "Get the object, change it, put it back" | The same, the change made in memory, inside the timing |
| Bare-bucket rename | "Copy then delete" | CopyObject, then DeleteObject |
| Layer placement | "On the client host over that same bucket, 512 MB shard cache" | `local.sh` and the `client-host` real run do the same, with voidfs-server's default 512 MiB cache |

### How each operation is sent

| Scenarios | voidfs | Bare bucket |
|---|---|---|
| get, stream get, range, head, list | GetObject (body collected, or streamed and dropped), GetObject with Range, HeadObject, ListObjectsV2 | The same |
| put, overwrite, fan-out put | PutObject | The same |
| multipart put | CreateMultipartUpload, every UploadPart at once, CompleteMultipartUpload | The same |
| write at 4 KiB | `PUT ?x-voidfs-write`, 4 KiB at offset 4 KiB | Get, change, put |
| append 4 KiB | `PUT ?x-voidfs-write` at offset = size | Get, change, put |
| insert or delete 4 KiB, start or middle | `PUT ?x-voidfs-splice` | Get, change, put |
| truncate 4 KiB from the end | `PUT ?x-voidfs-write`, empty body, `x-voidfs-size` = size − 4 KiB | Get, change, put |
| patch 16 × 4 KiB | `POST ?x-voidfs-patch` with 16 edits | Get, change, put |
| rename 64 MiB | `PUT ?x-voidfs-rename` | CopyObject, DeleteObject |
| move a folder of 200 files | `PUT ?x-voidfs-rename` of the folder | ListObjectsV2, 200 CopyObjects 8 at a time, one DeleteObjects |

### Where SpaceFS's description leaves a choice

SpaceFS has not published its harness. The MIT SDK packages (`@spacefs/s3sdk`, `spacefs-s3sdk`
on PyPI and crates.io, all 0.3.0) contain no benchmark code, and their source repository is
private. These are this harness's readings, and each is in
[`scenarios.rs`](../crates/voidfs-bench/src/scenarios.rs) or [`run.rs`](../crates/voidfs-bench/src/run.rs):

1. **Operations per round** are not published. The defaults are 200 for small reads, writes and
   metadata; 64 for edits of 1 MiB files; 32 for 32 and 64 MiB objects; 16 for 256 MiB objects,
   multipart uploads and folder moves; and one per object for fan-outs (1,000 or 200).
   `voidfs-bench list` shows them, and `--ops-scale` scales them.
2. **"write at 4 KiB in 64 MiB"** is read as 4 KiB written at offset 4 KiB. Their timings for it
   match their start-of-file edits, not their middle ones.
3. **"Middle"** is half the current size, rounded down to 4 KiB. **The patch's 16 edits** are
   spread evenly, one per sixteenth of the file.
4. **Objects.** Each worker edits, overwrites, renames or moves only its own object, so no two
   operations race on a key. Reads share one object per scenario. SpaceFS's warm figure for
   streaming 256 MiB only fits their 512 MB cache if the eight readers share one object.
5. **Ranges** start at random 4 KiB-aligned offsets.
6. **Content.** Every object and every write gets fresh pseudo-random bytes. voidfs deduplicates
   by content, so repeated or barely changed bodies would skip uploads a real workload pays for.
7. **Parallelism inside one operation.** A bare folder move copies 8 objects at a time. A
   multipart upload sends all its parts at once. Both are bounded by the 64-request cap.
8. **Isolation.** Each scenario runs in a fresh drive (voidfs) or prefix (bare), deleted
   afterwards. Round 1 runs voidfs first, round 2 the bare bucket first.
9. **Checksums and retries.** Both targets use the SDK's defaults: a CRC32 on uploads, and the
   standard retry policy. `--checksums when-required` matches SpaceFS's SDK configuration
   instead. Extension requests are never retried.
10. **Warm and cold.** voidfs's reads are warm by default: the setup upload fills its shard
    cache, as SpaceFS's main figures are warm. SpaceFS also gives figures with their cache
    cleared for four read rows, which their page describes as the same reads with the cache
    cleared, for workloads that read each object once. It does not say when they cleared it. Since a get row's eight readers share
    one object, clearing it once per round would leave all but the first operations warm, and
    their cold p50s (299 ms for 64 MiB, against 47 warm) are not. So `--cold` makes every
    measured operation start with voidfs-server's caches empty: each round runs in waves of one
    operation per worker, a wave starting once the one before has ended, and the shard and page
    caches are dropped (`SIGUSR1`, confirmed from the server's metrics) before each of voidfs's
    waves. The eight readers of a wave read the one object together, as in the warm rows. The
    bare target runs the same waves, with nothing dropped: the S3 server's own disk cache stays
    warm for both targets. What stays in voidfs-server's memory: each drive's state (its
    namespace, and with `inline-data` the content of files up to 4 KiB, which is the drive's
    log, not a cache), the record of which shards are known to be stored (it only spares
    writes an upload), and its connections to the bucket. Cold runs are scored against
    SpaceFS's cache-cleared figures where they give one.
11. **Timing.** From the first request of an operation to the last byte of its last response.
    Test data is generated before the clock starts.
12. **Verification.** After every edit, overwrite, rename and move scenario, the harness checks
    each object's size and compares worker 0's object with a model of what it should hold. A
    failed check marks the row.

## Results so far

Three runs on 27 September 2026, all from one MacBook Pro (Apple M5 Pro, 15 CPUs, 48 GB,
macOS 27): two with versitygw 1.8.0 on the internal SSD as the bucket, and one against
Cloudflare R2 (below). voidfs-server was the release build of this checkout
(commit `c434fcb` plus this work). Full tables:
[loopback](results/local-versitygw-loopback.md) and
[12 ms bucket](results/local-versitygw-rtt12.md).

> **These are not comparable with SpaceFS's figures.** One machine, a local disk for a bucket,
> no bandwidth limit, and no S3 service time. SpaceFS's column is there for orientation. What
> the runs can show is how voidfs's costs are shaped, and which rows will fall short in the
> real run.

Speed-up over the bare bucket, grouped as in [PARITY.md §6](../docs/PARITY.md#6-performance):

| Scenarios | Rows | Loopback | Bucket 12 ms away | SpaceFS (their run) |
|---|--:|---|---|---|
| Small, ranged and cached reads, `head`, fan-out gets | 7 | 1.1× slower to 4.6× faster | 1.1× slower to 48× faster | 2.6–34× faster |
| Large gets and streams (32–256 MiB) | 4 | 1.1–1.7× slower | 1.1–3.0× faster | 12–17× faster |
| Edits inside 32 and 64 MiB files | 16 | parity to 24× faster | 1.1× slower to 3.3× faster | 1.4–15× faster |
| Rename and folder move | 2 | 44–138× faster | 1.5–4.8× faster | 7.9–18× faster |
| Listing | 1 | 31× faster | 34× faster | 9.1× faster |
| Edits inside 1 MiB files | 8 | 1.6–4.7× slower | 3.4–3.6× slower | 2.1× slower to parity |
| Whole-object puts and overwrites, fan-out puts | 9 | 1.7–4.5× slower | 2.1–70× slower | 1.1–3.1× slower |
| Multipart uploads | 2 | parity to 1.2× faster | 1.6–1.9× slower | 1.8–2.4× slower |
| **All 49**: faster in / geometric mean | | 26 / 2.1× | 27 / 1.0× | 31 / 2.8× |

- **Loopback** hides every cost of talking to a bucket: versitygw answers in about 0.3 ms.
- **Bucket 12 ms away** (`BENCH_ONE_WAY_MS=4`): the bare bucket's `head` took 12.1 ms, the same
  as in SpaceFS's run. It is the better guide to the real run, except for large transfers: the
  relay adds latency but no bandwidth limit, so the bare bucket's large reads and writes are much
  faster than S3's.
- **Repeatability.** Between two loopback runs the median row moved 8% in speed-up. Rows whose
  p50 is around 10 ms moved by up to half.
- **Correctness.** No operation failed, and every edit, overwrite, rename and move row passed
  verification on both targets.

### Against Cloudflare R2

The 23 scenarios whose objects are 1 MiB or smaller, at a quarter of the operations
(`--exclude 32m --exclude 64m --exclude 256m --ops-scale 0.25`), on 27 September 2026. The
harness and voidfs-server ran on the same Mac, SpaceFS's topology, against an R2 bucket over a
home internet connection. That R2 bucket was far away: the bare bucket's `head` took 77 ms and
a 4 KiB put 218 ms. Full table: [r2-small-objects](results/r2-small-objects.md).

| Scenarios | voidfs | Bare R2 bucket | Result |
|---|--:|--:|---|
| get 4 KiB, `head`, get 1 MiB | 0.4–1.9 ms | 77–166 ms | 89–324× faster |
| fan-out gets (3 rows) | 1.2–3.1 ms | 114–204 ms | 50–94× faster |
| list 200 keys | 2.4 ms | 176 ms | 73× faster |
| move a folder of 200 files | 571 ms | 8,616 ms | 15× faster |
| edits in 1 MiB files (8 rows) | 1,537–1,720 ms | 540–674 ms | 2.4–3.2× slower |
| puts and overwrites, 4 KiB and 1 MiB | 1,588–1,791 ms | 213–315 ms | 5.4–8.2× slower |
| fan-out puts at 32 and 64 at once | 6.5–13.0 s | 207–319 ms | 20–62× slower |

This is finding 1 on a real bucket. A conditional PUT to this R2 bucket took about 205 ms, and
every voidfs write row sits at its concurrency times that: 1.6 s at 8 at once, 6.7 s at 32,
13 s at 64. Everything that only reads metadata or warm shards is two orders of magnitude
ahead. The run took 28 minutes and sent about 10,000 requests to the bucket directly and 7,600
to voidfs-server. Afterwards the voidfs pool held 5,992 objects, which were deleted
(`voidfs-bench purge`). A run's drives are hard-deleted at the end, so now garbage collection
reclaims what they held: with the server stopped, `voidfs-server gc --offline --grace 0` does it
at once.

Run again on 29 September with group commit, voidfs only and a 64 MiB cache so that it fills
([results](results/shard-cache/README.md#against-cloudflare-r2)): edits and small puts took
0.6–1.0 s instead of 1.5–1.8, and the fan-out puts 0.6–0.7 s instead of 6.5–13. With the cache
full, `main`'s fan-out gets went to the bucket (75–116 ms) and the shard cache's fix brought them
to 0.6–2.2 ms.

### Against AWS S3

The same 23 scenarios on 29 September 2026, against a bucket in us-east-1, with voidfs-server on
this Mac beside the harness and the default 512 MiB cache: SpaceFS's topology, but over a home
internet connection rather than from us-east4, so the bare bucket's `head` took 36 ms and a
4 KiB put 56 ms. Group commit and the shard cache were in. Full table:
[aws-small-objects](results/aws-small-objects.md).

| Scenarios | voidfs | Bare S3 bucket | Result |
|---|--:|--:|---|
| get 4 KiB, `head`, get 1 MiB | 0.3–1.7 ms | 36–341 ms | 99–199× faster |
| fan-out gets (3 rows) | 1.0–3.5 ms | 53–394 ms | 31–114× faster |
| list 200 keys | 2.3 ms | 61 ms | 27× faster |
| move a folder of 200 files | 126 ms | 3,216 ms | 26× faster |
| edits in 1 MiB files (8 rows) | 313–466 ms | 552–877 ms | 1.3–2.4× faster |
| puts and overwrites, 4 KiB and 1 MiB | 154–401 ms | 55–320 ms | 1.3–3.0× slower |
| fan-out puts (3 rows) | 169–349 ms | 56–281 ms | 1.2–3.5× slower |

20 of the 23 rows are at or ahead of SpaceFS's ratio to the bare bucket; put 4 KiB and the two
fan-out puts of 4 KiB are behind, held back by finding 2. The edits in 1 MiB files beat the bare
bucket here: the bare side downloads the file and uploads it again over the home connection,
where voidfs reads it from its cache. No operation failed. The run took 5 minutes; afterwards
everything under `voidfs-bench/` was deleted.

## Findings: where voidfs is far from parity, and why

The biggest first. The file references point at the code at `c434fcb`. The step-3 plan that
turns these into work items is [docs/step-3-performance.md](../docs/step-3-performance.md).

1. **A drive committed one mutation per bucket round trip.** `Pool::commit`
   ([`pool.rs`](../crates/voidfs-server/src/pool.rs)) held the drive's commit lock while it
   wrote the log entry with a conditional PUT. Operations on one drive therefore queued, and the
   p50 of any write row was its concurrency times one conditional PUT.
   - With the bucket 12 ms away, renaming a 64 MiB file took 12.8, 24.5, 48.4 and 99.2 ms at 1,
     2, 4 and 8 at once ([probes](results/probes/)). Fan-out puts took 412 ms at 32 at once and 859 ms at 64 (SpaceFS: 60 and
     67 ms). On loopback the ceiling is about 2,500 commits a second. Against R2, with about
     205 ms per conditional PUT, the same rows took 6.7 and 13 s.
   - It sets the floor of 37 rows: every edit, put, overwrite, multipart upload, rename and move.
     With S3's conditional PUTs taking tens of milliseconds, the real run will show hundreds of
     milliseconds at 8 at once, and seconds at 32 and 64.
   - **Fixed by group commit** (28 September 2026,
     [results](results/group-commit/README.md)): the mutations that wait while a log entry is
     written share the next one (`Commit.txns`). At 12 ms, renames at 1, 2, 4 and 8 at once now
     take 13.9, 20.7, 26.1 and 26.3 ms, and fan-out puts 37 and 38 ms at 32 and 64 at once. A
     small write's p50 is now two or three round trips at any concurrency, and finding 2 is what
     is left. Since 30 September a log entry also waits, at most 2 ms, for the requests the one
     before it answered, so small writes and renames 2 to 64 at once take one round trip, not two
     ([results](results/group-commit-hold/README.md)).
2. **Every write takes two or more bucket round trips, one after another.** A 4 KiB put at
   concurrency 1 took 27.3 ms where a rename took 12.8: shards first, then the log entry. The
   format requires that order (a commit may only reference stored shards, format §7.4), where
   SpaceFS commits in "one wave" (their words). Files of more than 1,024 extents (about 2 GiB)
   add a round trip for manifest pages; none of the benchmark's files are that large. `ingest`
   ([`object.rs`](../crates/voidfs-server/src/s3/object.rs)) also stops reading the body while
   each batch of four shards uploads, on top of chunking and hashing it on the server, and the
   chunker copies up to 16 MiB for each shard it cuts (found with finding 3; not timed). 32 and
   64 MiB puts run 2× slower than the bare bucket even on loopback; which of these costs
   dominates is not yet measured. Direction (step 3): the small-file path (E9, a format change)
   for tiny objects, and shard uploads overlapped with receiving the body.
3. **The shard cache stopped admitting new shards.** It was a moka cache with moka's default
   TinyLFU admission, which lets a new entry in only if it looks more popular than the entries it
   would evict. Shards that earlier scenarios read dozens of times (their drives long deleted)
   kept winning, so new objects were never cached.
   - In the 12 ms run, the second round of the 1,000 × 4 KiB fan-out read took 12.3 ms at p50,
     and its fastest read took 9.6 ms: not one hit. get 32 MiB had a p90 of 318 ms.
   - The same sequence of scenarios with the cache switched to LRU (a one-line change, tried in
     a throwaway build and not committed): get 32 MiB 75 → 27 ms, fan-out get 12.8 → 0.54 ms
     ([probes](results/probes/)).
   - On loopback a miss costs about a millisecond, so only the emulated distance shows this.
   - **Fixed** (28 September 2026, [results](results/shard-cache/README.md)): both caches keep
     what was used last, and keep their own copy of each shard, since a shard cut from an upload
     otherwise holds the chunker's whole buffer. At 12 ms the three fan-out gets take 0.5–1.1 ms
     instead of 12–15, get 32 MiB 25 ms instead of 56–88, and 12 of the 16 edits inside 32 and
     64 MiB files, which read the shard they rewrite, are 11–57% faster. Rows at or ahead of SpaceFS's
     ratio went from 16–19 to 21–22. The disk tier (S5) comes later.
4. **Patch applies its edits one at a time.** `content::apply_edits`
   ([`content.rs`](../crates/voidfs-core/src/content.rs)) runs each edit as a full `write_at`,
   which re-chunks and re-hashes the shard every time. Sixteen edits in a 1 MiB file took 20–27 ms
   on loopback, against about 7 ms for one edit, and 4.7–6.5× the bare bucket's download, change
   and upload. Direction: group the edits by shard and rewrite each shard once.
   - **Fixed** (30 September 2026, [results](results/patch-once/README.md)): the edits that share
     a shard are applied to it together, and it is chunked and hashed once. Patch in 1 MiB takes
     5.1 ms on loopback, what one edit takes (from 18.2), and 1.34× the bare bucket's time at
     12 ms (from 1.71×; SpaceFS 1.45×); patch in 32 MiB 0.69× (from 0.81×; SpaceFS 0.70×).
5. **Completing a multipart upload is a chain of round trips.** `complete_upload` reads the
   upload record, lists the parts, reads each part's record one after another, commits, and
   deletes the upload's records before it answers: about twenty sequential round trips for
   16 parts. On par on loopback, 1.6–1.9× slower at 12 ms. Direction: read the
   part records concurrently (or keep them in one record), and clean up after answering.
   - **Fixed** (30 September 2026, [results](results/multipart-complete/README.md)): completion
     reads the upload's record and the listed parts' together and answers once it commits, 32 ms
     at 12 ms where it took 157 ms for 8 parts and 258 ms for 16; the records are deleted after.
     Each part looks its upload up while its body is read. Multipart put 64 MiB × 8 MiB takes
     254 ms instead of 343, and 256 MiB × 16 MiB 1,008 instead of 1,147 (focused, 12 ms).
6. **Edits in 1 MiB files cost more than rewriting the file**, on loopback too: a shard read
   (usually from the cache), re-chunking, a shard write and a log write, against one GET and one
   PUT. SpaceFS is also
   behind on these rows (up to 2.1×). With findings 1 and 2 fixed, both sides are two round
   trips, which is parity.
7. **Large reads cannot be judged locally,** without a bandwidth limit. Without one, the bare
   bucket reads 64 MiB in 40–120 ms, where S3 took 779 ms in SpaceFS's run. voidfs's warm reads
   (45–55 ms for 64 MiB, 8 at once) are near what SpaceFS reported (47 ms), so the real run
   should show the gap. Cold reads fetch at most 8 shards at a time per request.
   - **Measured since 1 October** with S3's bandwidth emulated (`BENCH_BANDWIDTH=s3`) and cold
     (`BENCH_COLD=1`), at 12 ms ([results](results/cold-reads/README.md)). With the cap, the
     bare bucket reads 64 MiB in 728 ms (S3: 779), and voidfs's warm reads are 12–14× faster
     than it, behind SpaceFS's 15.5–16.5× for get 64 MiB and stream get 64 and 256 MiB (0.79–0.91
     of their ratio): voidfs serves 64 MiB from memory in 50–57 ms, eight at once, where SpaceFS
     took 45–47. Those three are the only rows of the 49 behind SpaceFS's ratio with the cap.
   - Cold, against SpaceFS's cache-cleared figures: get 4 KiB and 1 MiB are ahead (one shard
     GET, or none for a file held in the log). Get 32 and 64 MiB take 0.69–0.74× the bare
     bucket's time, behind SpaceFS's 0.50× and 0.38×: each of the eight readers of one cold
     object fetches every shard, 537 MB for a wave of 64 MiB reads, and the cap's 1,000 MB/s
     total, an assumption for downloads, binds. Without the total they take 0.29–0.39×, ahead.
     A cold 64 KiB range takes about 3× the bare bucket's time: it fetches its whole shard.
   - Direction (step 3, item 6): coalesce concurrent fetches of a shard, then ranged shard
     reads, then a wider window. The real run's cold figures will settle the download total.

What the runs confirm from [PARITY.md §6](../docs/PARITY.md#where-voidfs-stands): metadata-only
work (listing, `head`, small warm reads) is well ahead of the bare bucket at any distance, and
further ahead than SpaceFS. Renames, moves and edits in large files were ahead on loopback, but
behind SpaceFS's ratios once the bucket was far away, because of finding 1. With group commit,
the folder move is at SpaceFS's ratio at 12 ms and 9 or 10 of the 24 edits are ahead of it.
Overwrite 4 KiB is ahead too, and the other small puts and fan-out puts are within 2–28% of it,
held back by finding 2. With finding 3 fixed, the fan-out gets are ahead as well, and 9 to 12 of
the edits.

Two problems outside performance turned up on the way:
- voidfs-server answered `501 NotImplemented` to the `x-id=PutObject` query parameter that
  `aws-sdk-s3` 1.148 adds to its requests, so stock Rust SDK uploads failed. Fixed: the server
  now ignores `x-id`, as S3 does.
- MinIO's Homebrew build crashes at startup on this Mac, so the local runs use versitygw.

## The real run

SpaceFS ran from a Google Cloud n2-standard-8 in us-east4 against an AWS S3 bucket in
us-east-1, with their layer on the client host. Nothing here creates cloud resources: the
commands below are for whoever holds the accounts, and they cost money.

### Where voidfs-server runs

| Topology | voidfs-server | Compares with SpaceFS's figures? |
|---|---|---|
| `client-host` | On the client VM, beside the harness, over the bucket in us-east-1 | **Yes.** This is SpaceFS's setup, so the table gets a column that sets voidfs's speed-up against theirs (`--like-spacefs`) |
| `server-us-east-1` | On a VM in AWS us-east-1 beside the bucket, as [PARITY.md §7](../docs/PARITY.md#7-step-by-step-plan) planned | Not directly: voidfs pays a cross-cloud hop to the server where SpaceFS's layer paid none. It is the shape of a deployed server, so it is worth running as well |

### 1. The bucket and a key (AWS, us-east-1)

```bash
export BUCKET=voidfs-bench-$(openssl rand -hex 4)
aws s3api create-bucket --bucket "$BUCKET" --region us-east-1
# Clears multipart uploads an interrupted run leaves behind.
aws s3api put-bucket-lifecycle-configuration --bucket "$BUCKET" --lifecycle-configuration \
  '{"Rules":[{"ID":"abort-mpu","Status":"Enabled","Filter":{},"AbortIncompleteMultipartUpload":{"DaysAfterInitiation":1}}]}'
aws iam create-user --user-name voidfs-bench
sed "s/BUCKET/$BUCKET/g" bench/cloud/iam-policy.json > /tmp/voidfs-bench-policy.json
aws iam put-user-policy --user-name voidfs-bench --policy-name voidfs-bench-bucket \
  --policy-document file:///tmp/voidfs-bench-policy.json
aws iam create-access-key --user-name voidfs-bench   # keep the output private
```

S3 has native conditional writes (`If-None-Match: *`), which voidfs's commit log needs, and
which SpaceFS's run named.

### 2. The client VM (Google Cloud, us-east4)

```bash
gcloud compute instances create voidfs-bench-client --zone us-east4-a \
  --machine-type n2-standard-8 --image-family debian-12 --image-project debian-cloud \
  --boot-disk-size 100GB
gcloud compute ssh voidfs-bench-client --zone us-east4-a
```

On the VM:

```bash
sudo apt-get update && sudo apt-get install -y build-essential pkg-config git curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.98.1
. "$HOME/.cargo/env"
git clone https://github.com/vishnu597/voidfs.git && cd voidfs
```

Put the bucket key and a voidfs admin key in the environment, for example in a file only you can
read that you `source`:

```bash
export VOIDFS_S3_BUCKET=<bucket> VOIDFS_S3_REGION=us-east-1
export VOIDFS_S3_ACCESS_KEY_ID=<from create-access-key> VOIDFS_S3_SECRET_ACCESS_KEY=<...>
export VOIDFS_ACCESS_KEY_ID=VF$(LC_ALL=C tr -dc 'A-Z2-7' < /dev/urandom | head -c 18)
export VOIDFS_SECRET_ACCESS_KEY=$(LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 40)
```

### 3a. Run it as SpaceFS did (`client-host`)

```bash
bench/scripts/serve.sh > serve.log 2>&1 &
BENCH_TOPOLOGY=client-host bench/scripts/cloud-run.sh
```

### 3b. Run it with the server in us-east-1 (`server-us-east-1`)

Start an 8-vCPU VM in us-east-1 (for example an EC2 m7i.2xlarge with Debian 12). Allow TCP 9000
from the client VM's external address only: the traffic is plain HTTP, signed but not
encrypted. Prepare it as in step 2, with the same environment, then:

```bash
BENCH_LISTEN=0.0.0.0:9000 bench/scripts/serve.sh           # on the us-east-1 VM
```

```bash
VOIDFS_ENDPOINT=http://<us-east-1 VM address>:9000 BENCH_SERVER_HOST="EC2 m7i.2xlarge" \
  BENCH_TOPOLOGY=server-us-east-1 bench/scripts/cloud-run.sh  # on the client VM
```

Results land in `bench/results/cloud-<topology>-<time>.{json,md}`, without the bucket's name.
Copy them back with `gcloud compute scp` and commit them.

### Clean up

```bash
aws s3 rm "s3://$BUCKET" --recursive     # everything: the pool and the bare target's objects
aws s3api delete-bucket --bucket "$BUCKET"
aws iam delete-access-key --user-name voidfs-bench --access-key-id <id>
aws iam delete-user-policy --user-name voidfs-bench --policy-name voidfs-bench-bucket
aws iam delete-user --user-name voidfs-bench
gcloud compute instances delete voidfs-bench-client --zone us-east4-a
```

### What it moves

A full run on loopback moved, as the harness counts it:

| Target | Requests | Uploaded | Downloaded |
|---|--:|--:|--:|
| Bare bucket | 29,800 | 77 GiB | 72 GiB |
| voidfs, between the harness and the server | 20,600 | 25 GiB | 21 GiB |

In the `client-host` topology the bare side's traffic crosses from Google Cloud to AWS, and so
does voidfs-server's own traffic to the bucket: at least the 25 GiB it stores, plus its cache
misses. That is roughly 100 GiB leaving Google Cloud and 75 GiB leaving AWS, billed as internet
egress on both sides, plus tens of thousands of S3 requests. Check
current prices before running; the transfer, not the VM, is the larger part of the bill.
`--ops-scale 0.25` cuts the per-operation traffic to a quarter, but not the setup uploads.

### Later: from six regions

SpaceFS's [global run](https://docs.spacefs.com/benchmarks/global/) repeats the workload from
six regions against S3 in us-east-1 and reports each region's geometric mean speed-up. The
harness already prints that geometric mean, and `--label region=…` records where a run came
from. A global run is the same `cloud-run.sh` from a VM in each region, one region at a time;
what is missing is a report that sets the regions' result files side by side, and the
multi-region server itself (plan step 10).
