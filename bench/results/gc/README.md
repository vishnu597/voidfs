# Garbage collection: before and after

*27–28 September 2026, one Mac (Apple M5 Pro, 15 CPUs, macOS 27). All 49 scenarios through
`BENCH_ONE_WAY_MS=4 bench/scripts/local.sh`, with the bucket (versitygw 1.8.0) 12 ms away. voidfs
target only is compared here; each file also has the bare bucket's rows.*

| Files | voidfs-server |
|---|---|
| `main-1`, `main-2`, `main-3` | `main` at `a9ea745`: before garbage collection |
| `gc-1`, `gc-2` | This branch: garbage collection, and the writers' checks of format §12.4 on every upload |

The `commit` label in the files says `a9ea745 (with uncommitted changes)` for all five, because the
runs passed the server binary with `BENCH_SERVER_BIN`. This table says which is which.

## Result

Across all 49 rows the branch is at parity: the geometric mean of the p50 ratios is 1.007 and
the median 1.002. By family: edits 1.002, metadata 1.004, reads 1.001, writes 1.024.

Rows that moved by 3% or more (medians of the runs):

| Scenario | main (ms) | branch (ms) | Change |
|---|--:|--:|--:|
| range 64 KiB of 64 MiB | 0.3 | 0.3 | +9.6% |
| get 4 KiB | 0.4 | 0.4 | −3.2% |
| move dir 200 × 64 KiB | 104.6 | 99.9 | −4.6% |
| append 4 KiB to 64 MiB | 98.1 | 94.8 | −3.4% |
| head | 0.2 | 0.3 | +16.6% |
| get 32 MiB | 76.2 | 73.5 | −3.6% |
| list 200 keys | 1.1 | 1.0 | −4.1% |
| append 4 KiB to 32 MiB | 98.5 | 95.4 | −3.1% |
| rename 64 MiB | 106.2 | 101.1 | −4.8% |
| delete 4 KiB, middle of 32 MiB | 96.2 | 99.3 | +3.3% |
| get 1 MiB | 1.4 | 1.3 | −3.4% |
| insert 4 KiB, middle of 32 MiB | 99.0 | 95.0 | −4.0% |
| insert 4 KiB, start of 32 MiB | 93.4 | 97.1 | +4.0% |
| append 4 KiB to 1 MiB | 95.6 | 98.5 | +3.0% |
| put 4 KiB | 103.7 | 107.3 | +3.5% |
| truncate 4 KiB, end of 1 MiB | 93.1 | 96.4 | +3.6% |
| fanout put 1000 × 4 KiB, 32 at once | 411.4 | 426.4 | +3.6% |
| fanout put 1000 × 4 KiB, 64 at once | 829.6 | 885.7 | +6.8% |
| fanout put 200 × 256 KiB, 32 at once | 368.4 | 393.3 | +6.8% |
| overwrite 4 KiB | 104.6 | 111.3 | +6.5% |

The sub-millisecond rows (`head`, the range read) moved by tens of microseconds, within their
usual spread between runs (10–20%).

## The slower small writes

Four write rows are 3.5–7% slower (4 KiB overwrite, the three fan-out puts), and in every run.
What was established:

- **It comes from the new write path.** A diagnostic build of this branch with the old one
  (skip an upload when the bytes are cached, no reuse set) matched `main` on these rows: 103.1,
  104.7, 415 and 838 ms, against 103.7, 104.6, 411 and 830.
- **Part of it was moka.** The reuse set was first a moka cache. Its housekeeping runs inside
  inserts, on the async worker, and a plain set instead brought 4 KiB puts from +5.5% to +3.5%
  and fan-out puts at 32 from +7.4% to +3.6%. The shard cache is also moka; see
  [step-3-performance.md](../../../docs/step-3-performance.md), item 2.
- **It is not extra requests to the bucket.** Over the whole 49-scenario sequence, the two builds
  sent the same requests to versitygw (24,841 and 24,702 shard PUTs, 12,258 and 12,134 log
  PUTs).
- **It is not CPU.** On loopback the same rows are within run-to-run noise. A cost of the size
  measured here, about 1 ms per queued commit, would have tripled the loopback fan-out rows.
- **It does not appear in a short run.** Running only these rows, alternating builds three times
  each, gave parity (−0.9% to +0.1%).

The cause of what remains is not identified. These rows are set by one log write per commit,
which group commit (step 3, item 1) removes, so they should be measured again after that.
