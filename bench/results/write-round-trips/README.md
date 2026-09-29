# Fewer round trips per write: checkpoints in the background, ingest that uploads while it reads

*29 September 2026, one Mac (Apple M5 Pro, 15 CPUs, macOS 27). versitygw 1.8.0 as the bucket, 12 ms
away through `BENCH_ONE_WAY_MS=4 bench/scripts/local.sh` and on loopback, with voidfs-server's
512 MiB cache. Step 3, item 3 of the [parity plan](../../../docs/PARITY.md#7-step-by-step-plan),
changes 2 and 3 ([step-3-performance.md](../../../docs/step-3-performance.md#item-3-fewer-sequential-round-trips-per-write));
change 1 is [RFC 0003](../../../rfcs/0003-small-content-in-descriptors.md), accepted and not yet implemented.*

| Files | voidfs-server |
|---|---|
| `rtt12-main-*`, `loopback-main-*` | `main` at `41d489e` |
| `rtt12-branch-*`, `loopback-branch-*` | This branch: checkpoints written in the background, and an ingest that keeps reading the body while its shards hash and upload |
| `requests-rtt12` | This branch with `BENCH_BUCKET_REQUESTS=1` ([below](#what-the-counters-show)) |

Every binary was built with `cargo build --release -p voidfs-server`, each in a target directory of
its own (`main` in a separate worktree), and passed with `BENCH_SERVER_BIN`. Their hashes differ,
and only the branch's has `voidfs_checkpoint_write_seconds` in it (`strings`). The harness ran from
the `main` worktree, so that editing this checkout could not change it mid-run; it is the same
harness as the branch's, and every file's `commit` label says `41d489e`. The file names say which
server ran.

The focused runs used builds of each step, to tell the changes apart. Their files are not kept;
the tables below have their figures:

| Name | voidfs-server |
|---|---|
| `main` | `main` at `41d489e` |
| `ckpt` | Checkpoints in the background (the branch's first commit) |
| `crc` | `ckpt`, and upload checksums computed with sixteen tables |
| `ingest` | `crc`, the chunker's copy, and a window of uploads while the body is read |
| `branch` | `ingest`, and shards hashed on blocking threads (the branch's second commit) |

Run order:
1. Profiles of `put 64 MiB` on loopback with `sample`, `main`, 8 at once and one at a time.
2. The checkpoint probe on `main` alone, then `main` against `ckpt` A B B A B A A B.
3. Focused large puts and multipart uploads, `ckpt` against `ingest` A B B A B A A B on loopback
   and at 12 ms; `crc` against `ingest` on loopback (A B B A).
4. `ingest` against `branch` A B B A B A A B at 12 ms and on loopback; `crc` against `ingest` at
   12 ms (A B B A); one at a time at 12 ms (`ckpt`, `ingest`, `branch`, `branch`, `ingest`,
   `ckpt`); put 1 MiB at 4 times the operations and one at a time.
5. The full 49 rows at 12 ms: `rtt12-main-1`, `rtt12-branch-1`, `rtt12-branch-2`, `rtt12-main-2`.
6. The full 49 rows on loopback, A B B A then B A A B.
7. `requests-rtt12`.
8. Focused runs of the rows that looked slower at 12 ms: small writes and five edits (four times
   the operations), put and overwrite 4 KiB one at a time, and the 1 MiB rows on loopback, each
   `main` against the branch.

No run had errors.

## Measured first

### Where a large put spends its time

`put 64 MiB` on loopback against `main`, sampled every millisecond (`sample`), counting samples
where a thread was running, divided by the operations:

| Per 64 MiB put | 8 at once (p50 385 ms) | One at a time (p50 300 ms) |
|---|--:|--:|
| Upload checksum (`s3::chunked::Checksum::update`, CRC32) | 110 ms | 93 ms |
| SHA-256 (`sha2::compress256`) | 39 ms | 32 ms |
| FastCDC (`cut_gear`) | 23 ms | 18 ms |
| Sending to the bucket (`writev`) | 13 ms | 16 ms |
| Copies (`memmove`) | 7 ms | 4.5 ms |
| Reading the body (`recvfrom`) | 6 ms | 4 ms |

- One at a time, about 150 ms of the 300 is CPU in the request's own task, and about 150 is
  waiting for seven batches of four shard uploads: both of the doc's suspects, about equally.
- The largest cost was neither: it was the upload checksum the SDK sends
  (`x-amz-checksum-crc32`). The `crc` crate's default, one table and a byte at a time, runs at
  0.5 GB/s; with sixteen tables, 16 bytes at a time, 5.6 GB/s (CRC32) and 4.7 GB/s (CRC64/NVME) on
  one core. SHA-256 is 3.5 GB/s and FastCDC 2.9 GB/s there.
- SHA-256 runs twice over each byte: once for the payload hash the harness signs
  (`x-amz-content-sha256` over plain HTTP, which versitygw checks too on the bare side) and once
  for the shard's name. Neither can stand in for the other. Over HTTPS, SDKs normally send
  unsigned payloads with a checksum, and the first goes away.
- The chunker's copies, which the doc had not timed, came to 7 ms of the 385.

### What a checkpoint costs a commit

A checkpoint is due every 1,000 commits and was written under the commit lock, before the commit
that made it due was answered. The probe: `put 4 KiB` at 12 ms, 8 at once, 10,000 operations a
round (`--ops-scale 50 --samples`), so one drive grows to 20,000 objects and crosses five or six
checkpoints. On `main`, p50 was 28.5–30.6 ms and p99 44–49 ms, but 40–48 operations of the 20,000
took over 60 ms and 32–37 over 100: about eight writers for each checkpoint, waiting for its
three round trips and its encoding. So it shows at p99.9 (118–135 ms), not p99: at 8 at once,
group commit puts about 3.3 puts in a log entry, so about 3,300 puts come between checkpoints.

## Checkpoints in the background

The same probe, `main` against `ckpt`, A B B A B A A B (20,000 operations a run):

| Run | p50 (ms) | p99 | p99.9 | Max | Over 60 ms | Over 100 ms | Checkpoints (mean write) |
|---|--:|--:|--:|--:|--:|--:|---|
| `main-1` | 28.5 | 44.3 | 124.5 | 145.4 | 48 | 37 | 5 |
| `ckpt-1` | 30.6 | 49.7 | 56.0 | 88.1 | 17 | 0 | 5 (87 ms) |
| `ckpt-2` | 29.9 | 47.7 | 51.5 | 58.6 | 0 | 0 | 5 (81 ms) |
| `main-2` | 30.3 | 48.2 | 118.2 | 161.2 | 40 | 32 | 5 |
| `ckpt-3` | 30.2 | 48.0 | 52.1 | 61.3 | 2 | 0 | 5 (86 ms) |
| `main-3` | 30.6 | 48.6 | 134.6 | 165.8 | 40 | 37 | 5 |
| `main-4` | 28.9 | 44.8 | 125.1 | 150.6 | 48 | 32 | 6 |
| `ckpt-4` | 28.7 | 44.5 | 48.8 | 54.6 | 0 | 0 | 5 (79 ms) |

- The writes that landed on a checkpoint no longer wait for it: p99.9 went from 118–135 ms to
  49–56, the slowest write from 145–166 ms to 55–88, and none took over 100 ms.
- p50 and p99 did not move: p50 was 28.5–30.6 ms for both builds. By position, `ckpt` was 0.7–1.3%
  faster in three pairs and 7% slower in the first.
- The checkpoints themselves took 79–87 ms on average (`voidfs_checkpoint_write_seconds`), now
  off every request's path. The first `ckpt` run's 17 writes over 60 ms were no worse than 88 ms;
  the other three runs had 0–2.

## Ingest that uploads while it reads

Focused runs of the large puts and the multipart uploads. Each cell is the range over the runs of
that build; "change" is of the medians.

### On loopback

| Scenario | `ckpt` p50, over bare | `ingest` p50, over bare | Change |
|---|--:|--:|--:|
| put 64 MiB | 297–301 ms, 1.86–2.05× | 176–194 ms, 1.19–1.30× | −38% |
| put 32 MiB | 150–154 ms, 1.85–2.05× | 95–97 ms, 1.18–1.31× | −37% |
| put 1 MiB | 7.5–7.7 ms, 2.61–2.69× | 5.5–5.7 ms, 1.89–2.03× | −25% |
| multipart put 256 MiB × 16 MiB | 924–1,034 ms, 0.75–0.92× | 773–785 ms, 0.69–0.75× | −21% |
| multipart put 64 MiB × 8 MiB | 253–270 ms, 0.93–1.04× | 216–225 ms, 0.81–0.91× | −17% |

Of that, the checksum alone (`crc` against `ingest`, A B B A): `crc` took put 64 MiB to 200 ms
and put 32 MiB to 98–103, so the window and the chunker's copy added 7.5% and 6% on loopback, where
8 large puts at once keep the CPU busy (the harness, versitygw and the server share 15 cores).

### At 12 ms

| Scenario | `ckpt` p50, over bare | `ingest` p50, over bare | Change |
|---|--:|--:|--:|
| put 64 MiB | 371–376 ms, 2.31–2.37× | 210–221 ms, 1.32–1.36× | −43% |
| put 32 MiB | 189–192 ms, 2.12–2.21× | 117–120 ms, 1.32–1.35× | −38% |
| put 1 MiB | 31.9–32.5 ms, 2.30–2.38× | 32.3–34.0 ms, 2.36–2.43× | +3% ([below](#put-1-mib)) |
| multipart put 256 MiB × 16 MiB | 1,188–1,304 ms | 948–1,152 ms | −11% |
| multipart put 64 MiB × 8 MiB | 438–451 ms | 359–464 ms | +2% |

- At 12 ms the window matters more: `crc` alone took put 64 MiB to 267–275 ms (1.64–1.67×), and
  the rest of `ingest` to 227–228 (1.32–1.41×, −16%).
- The multipart rows' bare side swung from 559 to 2,212 ms for 256 MiB, so their ratios say
  little. Their parts are 8 and 16 MiB, four to eight shards each, so the window saves at most a
  batch of uploads per part; at 12 ms most of what is left is completion (item 5).

### Hashing shards on other threads

With the window, the request's own task still hashes each shard. `branch` hands shards of 256 KiB
or more to blocking threads, so that one body's shards hash in parallel:

| Scenario | `ingest` | `branch` | Change |
|---|--:|--:|--:|
| put 64 MiB, one at a time, 12 ms | 141–143 ms, 1.02–1.03× | 122–123 ms, 0.88–0.89× | −14% |
| put 32 MiB, one at a time, 12 ms | 87–88 ms, 1.17× | 78–79 ms, 1.05× | −10% |
| put 64 MiB, 8 at once, 12 ms | 208–220 ms, 1.29–1.33× | 204–220 ms, 1.22–1.39× | −1% (+1% over bare) |
| put 32 MiB, 8 at once, 12 ms | 119–123 ms | 112–116 ms | −4% |
| put 64 MiB, 8 at once, loopback | 180–187 ms | 180–187 ms | +1% |

One at a time, a 64 MiB put now takes less than the bare bucket's single PUT (`ckpt`: 345 ms,
2.47×). Eight at once, the machine has no core to spare, and nothing changes.

After the full runs, the hashing threads were capped at one per CPU across requests (past that, a
shard is hashed where its upload runs, as in `ingest`), so that many large uploads at once can't
grow tokio's blocking pool towards its 512 threads. The branch as committed has the cap; the full
runs had the build before it. At 12 ms, alternating the two: put 64 MiB one at a time
123.0–123.5 ms capped against 123.0–123.6 (eight runs), put 32 MiB 78.6–79.3 against 78.2–79.7;
eight at once, put 64 MiB 202.8–203.9 against 198.2–203.7 and put 32 MiB 109.8–111.8 against
113.5–114.0 (four runs).

### Put 1 MiB

Put 1 MiB at 12 ms, 8 at once, was 0.5–1.5 ms slower (2–4%) from `crc` on, in every run, with
round wall times 1–3% longer. One at a time, the same builds were about 1 ms faster than `ckpt` (p50
31.5–32.6 ms against 33.0–33.4, 200 operations a run). A 1 MiB body is one shard, so the window
does nothing for it; the checksum makes each put a little faster, and eight closed-loop clients
then fall into step with group commit a little worse. On loopback, the same row is 25% faster.

## The 49 rows at 12 ms

Comparing runs in the same position (`main-1` with `branch-2`, `branch-1` with `main-2`), the
geometric mean of the p50 ratios over the 49 rows is **0.976** (median 0.998); relative to the bare
bucket in the same run, **0.987** (median 0.992). By family, relative to bare: writes 0.922, edits
1.005, reads 1.022, metadata 0.982.

| Run | Rows at or ahead of SpaceFS's ratio | Rows faster than the bare bucket |
|---|--:|--:|
| `rtt12-main-1` | 24 | 30 |
| `rtt12-branch-1` | 20 | 30 |
| `rtt12-branch-2` | 23 | 31 |
| `rtt12-main-2` | 22 | 30 |

No row crossed SpaceFS's ratio because of these changes: the large puts moved most, to 1.3× the
bare bucket, and SpaceFS's are 1.1× and 1.2×. 19 rows are ahead in all four runs; the rest of the
count is rows near the line, ahead in some runs of either build: the folder move, insert at the
start of 64 MiB and in the middle of 32 MiB, both patches in large files, delete in the middle of
1 MiB, multipart put 256 MiB, and (in `branch-1` only) fan-out put 200 × 256 KiB.

Rows that moved 5% or more relative to the bare bucket:

| Scenario | main p50 (ms) | branch p50 (ms) | Raw change | Change relative to bare (odd / even) |
|---|--:|--:|--:|--:|
| put 64 MiB | 372.8 | 210.3 | −43.6% | −43.1% (−44.1% / −42.1%) |
| put 32 MiB | 192.2 | 115.3 | −40.0% | −39.7% (−39.1% / −40.4%) |
| multipart put 256 MiB × 16 MiB | 1,234 | 1,027 | −17.2% | −7.5% (−42.5% / +48.9%) |
| insert 4 KiB, middle of 32 MiB | 39.1 | 39.3 | +0.2% | −6.0% (−7.9% / −4.2%) |
| get 32 MiB | 26.3 | 25.6 | −2.9% | −6.0% (−8.3% / −3.8%) |
| truncate 4 KiB, end of 64 MiB | 38.1 | 37.5 | −1.6% | −5.4% (−15.1% / +5.5%) |
| head | 0.3 | 0.3 | −5.2% | −5.2% (+10.7% / −18.9%) |
| range 64 KiB of 64 MiB | 0.3 | 0.3 | +0.4% | +6.6% (+21.2% / −6.3%) |
| insert 4 KiB, start of 64 MiB | 39.0 | 41.0 | +5.1% | +7.3% (+1.5% / +13.3%) |
| get 4 KiB | 0.3 | 0.4 | +9.7% | +8.2% (+14.4% / +2.3%) |
| write at 4 KiB in 64 MiB | 39.0 | 39.8 | +2.0% | +8.6% (+3.9% / +13.5%) |
| stream get 256 MiB | 185.7 | 201.3 | +8.4% | +9.4% (−5.5% / +26.8%) |
| write at 4 KiB in 32 MiB | 35.5 | 39.4 | +10.7% | +10.7% (+18.8% / +3.1%) |
| overwrite 4 KiB | 31.5 | 33.4 | +6.0% | +14.8% (+33.0% / −1.0%) |
| delete 4 KiB, middle of 32 MiB | 38.3 | 41.4 | +8.0% | +17.7% (+9.9% / +26.0%) |

- The reads cannot have moved: nothing on the read path changed. They show how far rows move by
  themselves here.
- The edits inside 32 and 64 MiB files and overwrite 4 KiB, which moved 7–18%, were given focused
  runs ([below](#focused-runs-of-the-rows-that-moved)).

## On loopback

Two sets, A B B A then B A A B, compared by position over all eight runs: the geometric mean of the
p50 ratios is **0.984** raw and **0.965** relative to the bare bucket (medians 0.998 and 0.998). By
family, relative to bare: writes 0.822, edits 1.024, reads 0.992, metadata 0.985. The two sets
alone: 0.947 and 0.980 relative to bare.

| Runs | Rows at or ahead of SpaceFS's ratio | Rows faster than the bare bucket |
|---|--:|--:|
| `loopback-main-1` to `-4` | 31–32 (writes 7) | 27–28 |
| `loopback-branch-1` to `-4` | 34–35 (writes 8–10) | 28 |

The three write rows that crossed: put 32 MiB (0.84–0.91 of the bare bucket's speed in every branch
run, against 0.48–0.53 on `main`; SpaceFS's ratio is 0.83), overwrite 1 MiB (0.54–0.58 against
0.41–0.45; SpaceFS 0.52), and put 64 MiB in three of the four (0.90–0.99, and 0.86 in the fourth,
against 0.50–0.53; SpaceFS 0.88). Put 1 MiB went from 0.38–0.40 to 0.53–0.54, short of SpaceFS's 0.58.

Rows that moved 5% or more relative to the bare bucket, by position over the eight runs:

| Scenario | Change relative to bare (odd / even) |
|---|--:|
| put 64 MiB | −43.4% (−45.7% / −40.9%) |
| put 32 MiB | −42.8% (−45.3% / −40.2%) |
| put 1 MiB | −27.5% (−27.4% / −27.7%) |
| overwrite 1 MiB | −25.5% (−28.5% / −22.3%) |
| multipart put 256 MiB × 16 MiB | −16.3% (−16.3% / −16.4%) |
| fanout put 200 × 256 KiB, 32 at once | −12.5% (−14.1% / −10.8%) |
| multipart put 64 MiB × 8 MiB | −11.6% (−23.5% / +2.1%) |
| fanout get 1000 × 4 KiB, 64 at once | −7.1% (−10.5% / −3.5%) |
| delete 4 KiB, middle of 1 MiB | −8.8% (−2.2% / −15.0%) |
| write at 4 KiB in 1 MiB | +6.0% (+0.6% / +11.8%) |
| append 4 KiB to 1 MiB | +6.4% (+2.5% / +10.4%) |
| insert 4 KiB, middle of 1 MiB | +8.0% (+6.4% / +9.6%) |
| truncate 4 KiB, end of 1 MiB | +12.4% (+22.4% / +3.3%) |

- The edits inside 32 and 64 MiB files moved by −28% to +27% either way, as on loopback they do
  between identical builds (32 operations a round, which vary 2.5×); they are left out above.
- The edits inside 1 MiB files that moved did so both ways. Of the code that changed, they go
  through only the checksum, which is faster, and the committer's check for a due checkpoint; they
  had focused runs
  ([below](#focused-runs-of-the-rows-that-moved)).

## What the counters show

`requests-rtt12` counts the branch's own requests to the bucket in each scenario's measured rounds,
as [health-metrics](../health-metrics/README.md#what-the-counters-show) did for `main`. Per
operation, the two agree within what group commit's batching varies:

| Scenario | `main` | Branch |
|---|--:|--:|
| put 64 MiB | 28.45 shard PUTs, 0.77 log entry | 28.31, 0.55 |
| put 32 MiB | 14.72, 0.53 | 14.77, 0.67 |
| put 1 MiB | 1.24, 0.38 | 1.17, 0.37 |
| put 4 KiB | 1.00, 0.31 | 1.00, 0.32 |
| multipart put 256 MiB × 16 MiB | 138.9 PUTs, 33 GETs, 0.91 log entry | 137.3, 33.0, 0.78 |
| patch 16 × 4 KiB in 64 MiB | 15.8 PUTs, 5.4 GETs | 15.3, 4.9 |

These changes overlap requests; they don't remove any. A put still makes one shard PUT per shard
and then its share of a log entry, which is what RFC 0003 would change for small files. The two
findings from `main` stand: patch in 64 MiB reads shards back because eight 64 MiB working sets
overflow the 512 MiB cache (item 4, or the cache), and a multipart upload reads its part records
one after another when it completes (item 5).

## Focused runs of the rows that moved

The rows the full runs showed slower, again with four times the operations, `main` against the
branch A B B A B A A B at 12 ms:

| Scenario | `main` p50 (ms) | Branch p50 (ms) | Change of medians | Over bare |
|---|--:|--:|--:|--:|
| put 4 KiB | 29.8–32.8 | 30.0–34.1 | −1.3% | +3.4% |
| overwrite 4 KiB | 32.6–35.9 | 31.1–35.6 | −2.3% | +5.9% |
| fanout put 1000 × 4 KiB, 32 at once | 35.8–37.2 | 36.7–38.7 | +3.4% | +4.2% |
| fanout put 1000 × 4 KiB, 64 at once | 37.0–38.7 | 37.8–38.3 | +1.7% | +2.1% |
| put 1 MiB | 32.0 | 32.3–34.0 | +1.5% | +1.7% |
| overwrite 1 MiB | 31.9–32.1 | 32.4–34.1 | +1.1% | +1.9% |
| insert 4 KiB, start of 64 MiB | 34.8–41.5 | 34.4–38.5 | +0.9% | −1.0% |
| write at 4 KiB in 64 MiB | 35.9–42.3 | 34.6–38.2 | −4.3% | −4.5% |
| write at 4 KiB in 32 MiB | 34.4–40.3 | 36.4–39.8 | +5.9% | +8.1% |
| delete 4 KiB, middle of 32 MiB | 39.1–45.2 | 41.5–46.2 | +2.4% | +6.8% |
| insert 4 KiB, start of 32 MiB | 35.4–38.3 | 36.5–40.2 | +1.4% | +6.3% |

And one at a time (200 operations a run, A B B A):

| Scenario | `main` p50 (ms) | Branch p50 (ms) |
|---|--:|--:|
| put 4 KiB | 29.2, 29.5 | 29.4, 29.7 |
| overwrite 4 KiB | 29.0, 29.9 | 29.2, 29.1 |

On loopback, the 1 MiB rows and put 4 KiB with four times the operations (A B B A B A A B):

| Scenario | `main` p50 (ms) | Branch p50 (ms) | Change of medians |
|---|--:|--:|--:|
| put 1 MiB | 7.3–7.4 | 5.3–5.5 | −26.5% |
| overwrite 1 MiB | 7.3–7.4 | 5.3–5.4 | −26.6% |
| put 4 KiB | 1.5–1.6 | 1.5 | +0.8% |
| six other edits in 1 MiB: append, both inserts, delete at the start, write at, truncate | 3.9–5.5 | 4.2–5.6 | −2.3% to +2.3% |
| delete 4 KiB, middle of 1 MiB | 3.8–4.4 | 4.3–4.5 | +7.2% |
| patch 16 × 4 KiB in 1 MiB | 16.7–18.8 | 19.2–20.4 | +12.1% |

Patch in 1 MiB then had ten runs of its own, one of each build in turn and back (`main`, `ckpt`,
`crc`, `ingest`, `branch`, then in reverse): 16.4–18.4 ms for `main`, 18.7–19.7 for `ckpt`,
14.5–19.4 for `crc`, 17.5–19.0 for `ingest` and 16.7–20.3 for the branch. It moves as much within
a build as between them, and its path (read the small body whole, edit the shard, store it, commit)
has no code that changed but the faster checksum and the committer's check for a due checkpoint.

- The edits inside 32 and 64 MiB files move 6–15% between runs of the same build, more than
  between builds; they go through none of the new ingest code. No change can be called.
- One at a time, a 4 KiB put or overwrite takes the same time in both builds (within 1%), so
  nothing on its path costs more. Eight to 64 at once, put 1 MiB and the fan-out puts of 4 KiB
  are 1–3% slower in the branch, consistently; like put 1 MiB, which is 2–3% faster one at a time,
  that is how closed-loop clients fall into step with group commit when each request's own work
  takes a little less time, not a cost of the change.

## Caveats

- One machine: the harness, versitygw, the delay relay and voidfs-server share 15 cores. Large
  puts eight at once are bound by that CPU, which the real run's separate hosts would not be.
- The emulated distance adds latency, not a bandwidth limit, so large transfers are far faster
  than S3's; only the real run can judge the large rows (as before).
- The checkpoint probe's drive reaches 20,000 objects; a checkpoint of a drive of 600,000 rows
  takes hundreds of milliseconds of CPU, which the probe does not reach. That work now runs on a
  blocking thread and delays no commit, but it still uses a core while it runs.
