# Health checks and metrics: what recording costs

*29 September 2026, one Mac (Apple M5 Pro, 15 CPUs, macOS 27). versitygw 1.8.0 as the bucket, 12 ms
away through `BENCH_ONE_WAY_MS=4 bench/scripts/local.sh` and on loopback, with voidfs-server's
512 MiB cache. Step 2, last item, of the [parity plan](../../../docs/PARITY.md#7-step-by-step-plan).*

| Files | voidfs-server |
|---|---|
| `rtt12-main-*`, `loopback-main-*` | `main` at `acfb616` |
| `rtt12-branch-*`, `loopback-branch-*` | This branch: every request to the S3 port and to the bucket, and every cache read and log write, recorded for `/metrics` |
| `requests-rtt12` | This branch with `BENCH_BUCKET_REQUESTS=1`: the admin listener on, and the harness reading voidfs's requests to the bucket before and after each scenario's rounds ([below](#what-the-counters-show)) |

Both binaries were built with `cargo build --release -p voidfs-server`, each in a target directory
of its own (`main` in a separate worktree), and passed with `BENCH_SERVER_BIN`. Only the branch's
has the metric names and `--admin-listen` in it (`strings`), and their hashes differ. The files'
`commit` label names the checkout the harness ran from, the same for both; the file names say
which server ran. The A/B runs don't turn the admin listener on: what is measured is the
recording, which happens whether or not anything scrapes it.

Run order:
1. The full 49 rows at 12 ms: `rtt12-main-1`, `rtt12-branch-1`, `rtt12-branch-2`, `rtt12-main-2`.
2. The full 49 rows on loopback, three sets: A B B A (`main-1`, `branch-1`, `branch-2`,
   `main-2`), then B A A B (`branch-3`, `main-3`, `main-4`, `branch-4`), then A B B A (`main-5`,
   `branch-5`, `branch-6`, `main-6`). The second and third sets were added because of the first
   ([below](#on-loopback)).
3. Focused runs alternating builds (A B B A B A A B): small reads and writes on loopback, at the
   harness's operations and at 4 and 100 times as many; the rows that moved at 12 ms; the small
   writes one at a time; and the edits that moved on loopback. Their files are not kept; the
   tables below have their figures.
4. `requests-rtt12`.

No run had errors.

## What to expect

Recording is atomic adds. Per request to the S3 port: two clock reads, a latency histogram and a
counter, resolved to their labels when the server starts. Per request to the bucket the same,
plus the time of the last success or failure for `/readyz`. Per shard or page read, a hit or miss
counter; per log entry written, two histograms and a counter. Nothing is looked up by label, and
nothing is recorded per drive.

Two micro-benchmarks bound it:

| What | Cost |
|---|---|
| What a cache-hit GET records (two clock reads, a histogram, two counters), `prometheus` 0.14 | 45 ns on one thread; 910 ns a request with 8 threads doing nothing else on the same series, the worst case for contention |
| moka's per-key lock on each cache insert, which an eviction listener (to count evictions) turns on | 0.2–0.4 µs an insert and read: 680 → 900 ns on one thread, 650 → 1,030 ns on 8, inserting 4 KiB values past a full cache |

A cache-hit GET of 4 KiB takes 220 µs on loopback and a put of 4 KiB 1.7 ms (one or two cache
inserts), so neither should show. The runs below are there to check that nothing else does.

## At 12 ms

Comparing runs in the same position (`main-1` with `branch-2`, `branch-1` with `main-2`), the
geometric mean of the p50 ratios over the 49 rows is **1.001** (median 0.999); relative to the
bare bucket in the same run, 1.019 (median 1.010). The difference is the two multipart rows,
whose bare side swung from 591 to 2,185 ms for 256 MiB while voidfs took 1,213–1,298 ms in all
four runs. Without them, **1.002** raw and **0.996** relative to bare.

| Run | Rows at or ahead of SpaceFS's ratio | Rows faster than the bare bucket |
|---|--:|--:|
| `rtt12-main-1` | 22 | 31 |
| `rtt12-branch-1` | 22 | 30 |
| `rtt12-branch-2` | 25 | 31 |
| `rtt12-main-2` | 20 | 31 |

Rows that moved 5% or more relative to the bare bucket, 9 faster and 12 slower (two of them the
multipart rows):

| Scenario | main p50 (ms) | branch p50 (ms) | Raw change | Change relative to bare (odd / even) |
|---|--:|--:|--:|--:|
| insert 4 KiB, start of 32 MiB | 38.9 | 36.0 | −7.5% | −15.7% (−15.2% / −16.2%) |
| patch 16 × 4 KiB in 64 MiB | 140.8 | 132.5 | −5.7% | −11.3% (−16.6% / −5.7%) |
| write at 4 KiB in 64 MiB | 37.2 | 36.0 | −3.3% | −11.2% (−2.7% / −18.9%) |
| fanout get 1000 × 4 KiB, 64 at once | 1.12 | 1.00 | −10.7% | −10.7% (−6.6% / −14.6%) |
| put 4 KiB | 35.0 | 32.9 | −5.8% | −10.0% (−14.2% / −5.6%) |
| insert 4 KiB, middle of 32 MiB | 40.9 | 38.7 | −5.4% | −9.5% (−3.4% / −15.2%) |
| append 4 KiB to 64 MiB | 35.5 | 34.5 | −2.8% | −8.5% (−4.9% / −12.1%) |
| delete 4 KiB, start of 32 MiB | 38.4 | 36.9 | −3.9% | −7.0% (−10.1% / −3.9%) |
| fanout put 200 × 256 KiB, 32 at once | 37.2 | 36.2 | −2.7% | −6.4% (−5.7% / −7.1%) |
| list 200 keys | 1.13 | 1.12 | −0.4% | +5.2% (+15.5% / −4.2%) |
| fanout get 200 × 256 KiB, 32 at once | 0.99 | 1.03 | +4.7% | +5.5% (+9.2% / +1.9%) |
| delete 4 KiB, start of 1 MiB | 35.7 | 36.5 | +2.2% | +5.7% (+10.6% / +1.1%) |
| delete 4 KiB, start of 64 MiB | 35.7 | 37.0 | +3.7% | +6.0% (+7.0% / +5.1%) |
| truncate 4 KiB, end of 32 MiB | 35.1 | 35.8 | +2.0% | +6.6% (+9.0% / +4.3%) |
| patch 16 × 4 KiB in 32 MiB | 100.1 | 101.8 | +1.7% | +7.0% (+6.2% / +7.7%) |
| overwrite 4 KiB | 30.1 | 33.0 | +9.6% | +7.3% (+6.6% / +8.0%) |
| delete 4 KiB, middle of 32 MiB | 39.3 | 42.8 | +8.9% | +9.0% (+25.3% / −5.2%) |
| delete 4 KiB, middle of 64 MiB | 38.9 | 41.6 | +7.0% | +9.3% (+9.6% / +8.9%) |
| append 4 KiB to 32 MiB | 35.4 | 36.1 | +2.1% | +9.5% (+1.5% / +18.2%) |
| multipart put 64 MiB × 8 MiB | 450.0 | 448.2 | −0.4% | +31.4% (+53.6% / +12.5%) |
| multipart put 256 MiB × 16 MiB | 1,272 | 1,236 | −2.8% | +137.0% (+50.3% / +273.8%) |

Stream get 64 MiB and 256 MiB were 8.3% and 5.5% slower raw, within 5% relative to bare. Those,
the rows slower in both positions, and the small writes were run again, eight times alternating
builds (12 ms, 8 at once):

| Scenario | main p50 (ms) | branch p50 (ms) | Change | Relative to bare |
|---|--:|--:|--:|--:|
| stream get 256 MiB | 226.4 | 223.2 | −1.4% | −2.6% |
| stream get 64 MiB | 55.8 | 55.5 | −0.5% | −2.2% |
| delete 4 KiB, middle of 64 MiB | 39.1 | 38.7 | −1.1% | +2.0% |
| delete 4 KiB, middle of 32 MiB | 38.5 | 41.0 | +6.4% | +0.0% |
| insert 4 KiB, start of 32 MiB | 38.8 | 38.4 | −1.1% | +1.6% |
| delete 4 KiB, middle of 1 MiB | 35.2 | 34.7 | −1.4% | −0.9% |
| put 4 KiB | 33.6 | 35.8 | +6.8% | +9.4% |
| overwrite 4 KiB | 35.8 | 34.9 | −2.4% | +3.8% |

Put 4 KiB went from 5.8% faster in the full runs to 6.8% slower here, in the way closed-loop
clients fall into step with group commit: its rounds' p50s are two-valued in both builds (main's
alone range 29.4–38.7 ms), while every round's p90 (40–43 ms) and wall time (mean 865 ms for
`main`, 862 for the branch) stay put. One at a time, four runs alternating:

| Scenario | main p50 (ms) | branch p50 (ms) | Change |
|---|--:|--:|--:|
| put 4 KiB | 26.0, 27.4 | 27.3, 27.1 | +1.9% of medians, from `main`'s first run |
| overwrite 4 KiB | 26.8, 27.6 | 27.2, 27.2 | 0.0% |

## On loopback

Over all twelve runs, each build three times in each position, the geometric mean of the p50
ratios is **1.006** (median 0.999) and, relative to the bare bucket, **1.000** (median 0.995):
edits 1.005, metadata 1.001, reads 0.982, writes 1.002.

The first set alone said 4.2% slower relative to bare. The machine was slower in its two middle
runs, the bare bucket too (its geometric mean went 22.9, 24.5, 25.0, 24.0 ms). Relative to bare,
the middle runs of the first two sets were 3–4% behind whichever build ran there; in the third,
the last run was the slow one:

| Set | Order | voidfs over bare, by run (geometric mean of the 49 rows) | Branch against `main` |
|---|---|---|--:|
| 1 | A B B A | 0.316, **0.331**, **0.329**, 0.317 | +4.2% |
| 2 | B A A B | 0.322, **0.330**, **0.330**, 0.319 | −2.7% |
| 3 | A B B A | 0.321, 0.325, 0.324, **0.330** | −0.2% |

Rows that moved 5% or more relative to bare over the twelve runs, 12 faster and 9 slower:

| Scenario | main p50 (ms) | branch p50 (ms) | Raw change | Change relative to bare (odd / even) |
|---|--:|--:|--:|--:|
| delete 4 KiB, start of 64 MiB | 10.6 | 9.27 | −11.4% | −19.2% (−14.4% / −23.7%) |
| patch 16 × 4 KiB in 32 MiB | 77.4 | 80.1 | +4.3% | −12.3% (−16.3% / −8.1%) |
| get 1 MiB | 0.87 | 0.72 | −13.9% | −12.3% (−10.4% / −14.2%) |
| insert 4 KiB, middle of 32 MiB | 12.1 | 11.3 | −9.3% | −11.1% (+3.6% / −23.7%) |
| patch 16 × 4 KiB in 64 MiB | 104.0 | 101.1 | −2.8% | −8.1% (−20.4% / +6.0%) |
| append 4 KiB to 64 MiB | 6.38 | 5.95 | −0.1% | −7.9% (+0.5% / −15.7%) |
| insert 4 KiB, start of 64 MiB | 9.25 | 9.39 | −4.5% | −7.5% (−11.9% / −2.9%) |
| delete 4 KiB, middle of 32 MiB | 12.8 | 11.8 | −6.7% | −6.9% (−2.6% / −11.0%) |
| patch 16 × 4 KiB in 1 MiB | 20.6 | 20.5 | −1.0% | −6.7% (−6.9% / −6.4%) |
| write at 4 KiB in 1 MiB | 5.33 | 4.72 | −11.8% | −6.6% (−3.4% / −9.7%) |
| insert 4 KiB, start of 1 MiB | 5.40 | 5.25 | −2.8% | −6.3% (−9.6% / −2.8%) |
| get 32 MiB | 28.1 | 26.5 | −4.4% | −5.3% (+1.4% / −11.5%) |
| write at 4 KiB in 32 MiB | 8.55 | 8.41 | −1.7% | +5.5% (+20.0% / −7.2%) |
| insert 4 KiB, start of 32 MiB | 7.57 | 8.88 | +14.7% | +5.6% (−10.3% / +24.2%) |
| insert 4 KiB, middle of 1 MiB | 5.56 | 6.07 | +9.2% | +8.8% (+12.1% / +5.5%) |
| fanout put 1000 × 4 KiB, 64 at once | 9.10 | 9.08 | −0.1% | +10.6% (+10.1% / +11.0%) |
| delete 4 KiB, middle of 64 MiB | 12.3 | 12.5 | +6.9% | +12.3% (+2.1% / +23.6%) |
| truncate 4 KiB, end of 64 MiB | 6.44 | 6.79 | +11.0% | +14.7% (+6.9% / +23.1%) |
| insert 4 KiB, middle of 64 MiB | 11.3 | 12.7 | +13.9% | +19.3% (+48.1% / −3.9%) |
| delete 4 KiB, start of 32 MiB | 7.29 | 9.17 | +14.7% | +21.7% (+29.1% / +14.7%) |
| append 4 KiB to 32 MiB | 6.11 | 6.73 | +17.5% | +26.3% (+34.6% / +18.5%) |

Edits on loopback wait on versitygw's disk, and with 32 operations a round their p50 moves a lot
between identical runs: append 4 KiB to 32 MiB took 4.10–9.92 ms in `main`'s six runs and
4.40–9.42 in the branch's. The slower ones were run again, eight times alternating:

| Scenario | main p50 (ms) | branch p50 (ms) | Change | Relative to bare |
|---|--:|--:|--:|--:|
| truncate 4 KiB, end of 64 MiB | 5.72 | 5.50 | −3.8% | −15.7% |
| insert 4 KiB, middle of 64 MiB | 12.5 | 11.5 | −8.0% | −3.2% |
| append 4 KiB to 32 MiB | 4.44 | 5.30 | +19.5% | +19.9% |
| delete 4 KiB, start of 64 MiB | 7.67 | 9.28 | +21.0% | +10.2% |
| delete 4 KiB, start of 32 MiB | 9.05 | 9.26 | +2.3% | +9.1% |
| insert 4 KiB, middle of 1 MiB | 5.48 | 5.29 | −3.4% | −0.2% |
| fanout put 1000 × 4 KiB, 64 at once | 9.97 | 9.89 | −0.8% | +10.6% |

Delete 4 KiB at the start of 64 MiB is now 21% slower where the full runs had it 19% faster. Append
4 KiB to 32 MiB came out slower both times, so it ran eight more times with 256 operations a round:
6.02 ms for `main` and 6.20 for the branch (+2.9%), with ranges of 5.50–6.67 and 4.17–7.74, and
the branch's per-round means lower (about 6.4 against 7.0 ms).

## Small reads and writes, focused

Where a recording's cost would be the largest share: cache hits of under a millisecond, and small
writes. Eight runs each, alternating builds, on loopback.

| Scenario | Harness's operations | 4× the operations | 100× (20,000 a round) |
|---|--:|--:|--:|
| get 4 KiB | −0.2% | +5.9% | −2.8% (0.24 → 0.23 ms) |
| head | −0.9% | +1.9% | +0.3% (0.21 → 0.21 ms) |
| range 64 KiB of 64 MiB | +0.4% | −0.1% | |
| get 1 MiB | −4.3% | +1.4% | |
| fanout get 1000 × 4 KiB, 32 at once | −3.0% | +1.8% | |
| fanout get 1000 × 4 KiB, 64 at once | +2.4% | +1.8% | |
| fanout get 200 × 256 KiB, 32 at once | +1.4% | +2.3% | |
| put 4 KiB | +5.0% | −1.3% | |
| overwrite 4 KiB | +2.1% | +0.1% | |
| append 4 KiB to 1 MiB | −1.8% | +6.2% | |
| write at 4 KiB in 1 MiB | −4.9% | −1.9% | |
| fanout put 1000 × 4 KiB, 32 at once | −0.6% | +1.2% | |
| fanout put 1000 × 4 KiB, 64 at once | −4.0% | +0.3% | |
| fanout put 200 × 256 KiB, 32 at once | +3.5% | −2.7% | |

Changes are of the median p50 over the bare bucket's in the same run. The longest runs, where each
round of get 4 KiB takes 0.55–0.68 s of cache hits, differ by less than the runs of either build
differ among themselves, and so do their rounds' wall times.

## Verdict

No row costs anything that the runs can tell apart from the spread between identical runs, at
12 ms or on loopback. Every row that moved in the full runs either moved the other way when run
again, or stayed within its own spread once it had enough operations. That fits the
micro-benchmarks: well under a microsecond per request against 220 µs for the fastest one.

## What the counters show

`requests-rtt12` is the first run with voidfs's own requests to the bucket counted per scenario
(from `voidfs_bucket_requests_total`, over the measured rounds). Some rows, per operation:

| Scenario | Bucket requests | Of which |
|---|--:|---|
| every read (get 4 KiB to stream get 256 MiB, fan-out gets, range, head, list) | 0.00 | all served from memory |
| put 4 KiB, 8 at once | 1.31 | 1.00 shard PUT, 0.31 log entry (group commit: about 3 puts an entry) |
| put 4 KiB, 1,000 at 64 at once | 1.04 | 1.00 shard PUT, 0.04 log entry |
| insert 4 KiB in the middle of 64 MiB | 1.44 | 1.00 PUT, 0.44 log entry |
| rename 64 MiB, move dir | 0.25 | log entries only |
| put 64 MiB | 29.2 | 28.45 shard PUTs, 0.77 log entry |
| patch 16 × 4 KiB in 64 MiB | 21.7 | 15.8 PUTs, **5.4 GETs**, 0.55 log entry |
| multipart put 256 MiB × 16 MiB | 174.8 | 138.9 PUTs, **33 GETs**, 1 listing, 1 deletion by prefix, 0.91 log entry |

Three things only this shows:
- A 4 KiB edit is one shard upload and then a share of a log entry, so its time at 12 ms is an
  upload, then a log write, one after the other: the cost step 3, item 3 is about.
- Patch in 64 MiB reads shards back from the bucket: eight workers editing 64 MiB files of their
  own fill the 512 MiB cache, so some of what a patch rewrites has been evicted.
- A multipart upload of 16 parts reads `upload.json` again for each part and at completion (17),
  and then its 16 part records one after another (`parts_of` in `s3/object.rs`): 16 round trips
  in a row when it completes. The 8-part upload shows the same, 17 GETs.

## Caveats

- One machine, and the bucket is versitygw on its disk: loopback rows that write are as much a
  measure of the disk as of voidfs, as the position effect above shows.
- The micro-benchmarks isolate recording and moka's key lock. They leave out effects on caches and
  the scheduler, which the end-to-end runs cover as far as their spread allows: a cost below
  about 2% on a single row would not have shown.
- Scrapes cost more: gathering runs moka's pending housekeeping for both caches and encodes about
  490 lines. That happens once per scrape, not per request, and the A/B runs didn't scrape.
