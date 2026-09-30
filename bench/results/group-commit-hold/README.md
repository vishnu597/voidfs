# A hold before each log entry

*30 September 2026, one Mac (Apple M5 Pro, 15 CPUs, macOS 27). versitygw 1.8.0 as the bucket, 12 ms
away through `BENCH_ONE_WAY_MS=4 bench/scripts/local.sh` and on loopback, with voidfs-server's
512 MiB cache. A follow-up to group commit, step 3, item 1 of the
[parity plan](../../../docs/PARITY.md#7-step-by-step-plan)
([step-3-performance.md](../../../docs/step-3-performance.md#item-1-group-commit-one-log-write-per-batch-not-per-mutation)):
after a log entry lands, the next one waits, briefly, for the requests it answered, so that
concurrent small writes and renames share one round trip instead of taking two
([small-content](../small-content/README.md#why-eight-at-once-still-takes-two-round-trips) found
why they took two).*

| Files | voidfs-server |
|---|---|
| `rtt12-main-*`, `loopback-main-*` | `main` at `bc851c2` |
| `rtt12-branch-*`, `loopback-branch-*` | This branch |

Every run recorded voidfs's own requests to the bucket (`BENCH_BUCKET_REQUESTS=1`). Each binary was
built with `cargo build --release -p voidfs-server`, in a target directory of its own (`main` in a
separate worktree), and passed with `BENCH_SERVER_BIN`; their hashes differ, and only the branch's
has `voidfs_commit_hold_seconds` in it (`strings`). The harness ran from the `main` worktree with
`BENCH_POOL_FEATURES=inline-data`, so every pool listed `inline-data`, as `main`'s do since
[RFC 0003](../../../rfcs/0003-small-content-in-descriptors.md). Every file's `commit` label says
`bc851c2`; the file names say which server ran.

The focused runs' files are not kept; the tables below have their figures. Besides `main` and the
branch, they used two diagnostic builds, neither of which is in the branch:

| Name | voidfs-server |
|---|---|
| `diag` | An earlier version of the branch, with a switch: no hold, a hold after every entry, or a hold decided from the entry before alone; and a spin below a timer tick |
| `diag2` | The branch before two last changes that do not change what these runs did (entries written in under 4 ms left out of the tally, where a zero-length hold had stood; a count corrected for entries that overflow a batch), with switches to turn holding off and to spin below a tick |

Run order:
1. Exploring, at twice the operations: `main` against `diag` in each mode at 12 ms (mirrored,
   two runs each); `main` against `diag2` at 12 ms (A B B A B A A B), and on loopback with and
   without the spin (mirrored, four runs each).
2. The branch against `main`, focused, four times the operations, A B B A B A A B: the small
   writes, rename, the folder move and the edits in 1 MiB files, at 12 ms and on loopback.
3. Put 4 KiB at 1, 2, 4 and 8 at once, every latency kept (`--samples`), A B B A each.
4. The full 49 rows at 12 ms (A B B A), and on loopback (A B B A, then B A A B).
5. Focused again, on the rows the full runs showed slower, at 12 ms and on loopback, with the
   server's metrics scraped every 2 seconds; then `diag2` with holding off and on, the same
   binary, on the rows still in question.

No run had errors.

## What the hold does

`Pool::drain` ([pool.rs](../../../crates/voidfs-server/src/pool.rs)) writes a drive's log entries
one at a time, and took each batch as soon as the entry before it landed. The requests that entry
had just answered then came back while the next was in flight, waited for it, and then for their
own: two groups of clients taking turns, each write two round trips.

- **After an entry lands, the next waits until as many mutations have queued since as the entry
  answered,** or until a whole batch (256) is waiting. A client alone never waits: the entry after
  its last one waits for its next write, which is what comes. `commit_all` tells a waiting task
  through a `Notify`, whose `notified()` is made before the queue is looked at, so none is missed.
- **For at most a quarter of the entry's write time, and 2 ms.** A client near the server comes
  back well within that; one further away would not be caught by a hold worth making. tokio's
  timers fire on a 1 ms tick, up to a tick late, so the timer is set a tick early: a hold never
  passes its bound.
- **Not after an entry written in less than 4 ms** (loopback, or a bucket on the same machine):
  the bound would be under a tick. Measured alternatives:
  - a spin of `yield_now`, bounded by a quarter of the write (about 75 µs on loopback): log
    entries per put 4 KiB stayed at 0.25, so no client came back within it, and it kept a worker
    busy;
  - waiting only for the queue to fill, with no timer, would wait for ever when clients do not
    come back.
- **Only while it pays.** Clients that do not come back straight away (edits, which upload a shard
  before they commit; requests arriving at their own pace) would make whatever is waiting pay the
  bound each time. The drive keeps a tally of how many mutations queued within each entry's bound
  against how many the entry answered, held or not (each mutation carries the instant it was
  queued), with the sums decayed by an eighth per entry. It holds while at least half came back.
  This was the third try:

  | Edits in 1 MiB files at 12 ms, relative to bare, against `main` | 8 rows |
  |---|---|
  | Hold after every entry (`diag`) | seven of the eight 8–19% slower |
  | Decide from the entry before alone (`diag`) | −3% to +9%, seven of them slower |
  | The tally (`diag2`, A B B A B A A B) | −2.4% to +3.3% |

  Deciding from one entry failed because edits' entries answer one to three: one request that
  happens to arrive within the bound turned holding on. In all three, the small writes, rename and
  the folder move gained the same.
- **The commit lock is not held while holding,** so forks, hard deletes and a checkpoint's swap
  take it between entries as before.
- **Where the state lives:** the last entry's landing, bound and count are local to the draining
  task; the tally is on the drive, since that task ends whenever the queue empties. Not in
  `Cadence`, which each checkpoint resets.
- `voidfs_commit_hold_seconds` reports each hold. At 12 ms, over the second focused set's
  scenarios, 72–104 holds a run, 1.6–1.8 ms on average; on loopback, 40–57 a run out of about
  2,000 entries, 0.7–1.0 ms on average, after writes that took over 4 ms under load.

Tests in `pool.rs`, each seen to fail with the code it guards broken: clients writing together
share entries, and start holding again after they had stopped (eight clients, ten rounds: at most
15 entries, where taking turns makes 20); a client alone never waits; a hold whose clients do not
come back ends by its bound, and the next entries are not held; an entry written in under 4 ms is
not held. The bound is checked to within 10 ms, so a hold of 3–4 ms instead of 2 would pass.

## Eight to 64 at once, a small write takes one round trip

At 12 ms, `main` against the branch, A B B A B A A B, four times the operations. p50 relative to
the bare bucket in the same run, the median of four runs each:

| Scenario | `main` | Branch | Each pair | SpaceFS | p50 (ms) | p90 (ms) | Log entries per op |
|---|--:|--:|---|--:|---|---|---|
| put 4 KiB | 2.18× | 1.20× | −45% −46% −44% −47% | 2.01× | 29.3 → 16.0 | 32.4 → 24.5 | 0.250 → 0.138 |
| overwrite 4 KiB | 2.19× | 1.15× | −44% −52% −46% −49% | 3.05× | 29.2 → 15.5 | 31.8 → 18.4 | 0.250 → 0.135 |
| fan-out put 1,000 × 4 KiB, 32 at once | 2.46× | 1.39× | −43% −42% −46% −41% | 2.23× | 30.6 → 17.4 | 33.2 → 33.3 | 0.064 → 0.041 |
| fan-out put 1,000 × 4 KiB, 64 at once | 2.48× | 1.47× | −40% −40% −46% −40% | 2.41× | 30.5 → 17.9 | 32.8 → 33.3 | 0.032 → 0.022 |
| rename 64 MiB | 0.227× | 0.116× | −47% −50% −50% −52% | 0.126× | 27.3 → 13.9 | 29.6 → 14.8 | 0.250 → 0.133 |
| move dir 200 × 64 KiB | 0.062× | 0.032× | −49% −48% −50% −49% | 0.056× | 27.0 → 13.8 | 28.0 → 20.8 | 0.250 → 0.141 |

- All six are at or ahead of SpaceFS's ratio in every branch run; with `main`, only overwrite 4 KiB
  was. Rename 64 MiB, at 0.109–0.118× in the four runs, crossed SpaceFS's 0.126×.
- Each entry now holds about seven of the eight puts, not four. The fan-out puts' p90 is still two
  round trips: some of 32 or 64 clients come back after the hold's 1–2 ms.
- The edits in 1 MiB files moved −2.9% to +5.5% (median over the eight +0.8%), and not the same
  way in each pair; their log entries per operation went from 0.371 to 0.34–0.37.

## One to eight at once

Put 4 KiB at 12 ms, A B B A at each concurrency, twice the operations, every latency kept:

| At once | `main` p50 (ms) | Branch p50 | `main` / bare | Branch / bare | `main` p90 / p99 | Branch p90 / p99 | Log entries per op |
|--:|--:|--:|--:|--:|---|---|---|
| 1 | 13.5 | 13.5 | 1.04× | 1.03× | 15.0 / 17.5 | 15.0 / 16.7 | 1.00 → 1.00 |
| 2 | 26.5 | 14.0 | 2.00× | 1.05× | 28.1 / 30.2 | 15.5 / 28.3 | 0.99 → 0.51 |
| 4 | 26.7 | 14.1 | 2.03× | 1.05× | 28.5 / 30.5 | 15.9 / 30.1 | 0.50 → 0.26 |
| 8 | 27.1 | 14.3 | 2.22× | 1.13× | 29.4 / 31.5 | 15.6 / 30.5 | 0.25 → 0.13 |

Two at once took two round trips, each client in an entry of its own; now they share one. One at
a time nothing changes. p99 is still about two round trips: a few requests miss the hold.

## On loopback, nothing is held

A log entry there takes about 0.3 ms, so no entry is held but the few written in over 4 ms under
load. Focused, A B B A B A A B, four times the operations, relative to the bare bucket:

| Scenario | `main` | Branch | Change | Each pair | Log entries per op |
|---|--:|--:|--:|---|---|
| put 4 KiB | 1.08× | 1.12× | +3.7% | +1% +1% +7% +5% | 0.251 → 0.251 |
| overwrite 4 KiB | 0.93× | 0.95× | +1.7% | +4% −3% −4% +9% | 0.251 → 0.251 |
| fan-out put 1,000 × 4 KiB, 32 at once | 0.67× | 0.67× | +0.7% | −3% +10% +2% −6% | 0.064 → 0.064 |
| fan-out put 1,000 × 4 KiB, 64 at once | 0.55× | 0.53× | −3.7% | −2% −6% −1% −12% | 0.033 → 0.033 |
| rename 64 MiB | 0.0078× | 0.0076× | −3.2% | −6% −3% +2% −4% | 0.250 → 0.250 |
| move dir 200 × 64 KiB | 0.0021× | 0.0020× | −4.7% | −9% −6% −1% −0% | 0.250 → 0.250 |
| edits in 1 MiB files (8) | | | −3.9% to +9.9% | | 0.59–0.75, unchanged within 0.04 |

The rows that looked slower were run again, with the same method:

| Scenario | First set | Second set | `diag2`, holding off against on |
|---|--:|--:|--:|
| put 4 KiB | +3.7% | +2.3% (+1% +2% +2% +3%) | −0.5% |
| overwrite 4 KiB | +1.7% | +0.7% | |
| patch 16 × 4 KiB in 1 MiB | +9.9% | −4.1% | |
| insert 4 KiB, start of 1 MiB | +7.4% (+27% +3% −4% −1%) | | |
| truncate 4 KiB, end of 1 MiB | +5.9% | −6.1% | |
| insert 4 KiB, middle of 1 MiB | +2.2% | −0.4% | |

Put 4 KiB is 2–4% slower than `main`'s, and slower in every pair, but within 0.5% with holding off
and on in one binary. What else differs between the builds costs a few nanoseconds a write (a timestamp and
a `notify_waiters` per request), so this reads as the two binaries, not the hold; not measured
further. The edits in 1 MiB files moved both ways between the two sets.

## The 49 rows at 12 ms

`rtt12-main-1`, `rtt12-branch-1`, `rtt12-branch-2`, `rtt12-main-2`, compared in the same position
(`main-1` with `branch-2`, `branch-1` with `main-2`): relative to the bare bucket in the same run,
the geometric mean of the p50 ratios over the 49 rows is **0.932** (median 0.986). By family: writes
0.830, metadata 0.747, edits 0.990, reads 1.002.

| Run | Rows at or ahead of SpaceFS's ratio (writes, metadata) | Rows faster than the bare bucket | Geometric mean speed-up over bare |
|---|--:|--:|--:|
| `rtt12-main-1` | 22 (3, 3) | 30 | 2.7× |
| `rtt12-branch-1` | 26 (6, 4) | 30 | 2.9× |
| `rtt12-branch-2` | 25 (6, 4) | 30 | 2.9× |
| `rtt12-main-2` | 22 (5, 2) | 30 | 2.7× |

SpaceFS's geometric mean is 2.8×. Rename 64 MiB, put 4 KiB and the two fan-out puts of 4 KiB are
ahead of SpaceFS's ratio in both branch runs; `main` had put 4 KiB and the fan-out put at 64 ahead
in one run of two.

Rows that moved 5% or more relative to the bare bucket:

| Scenario | Change relative to bare (odd / even) |
|---|--:|
| rename 64 MiB | −54.4% (−60.7% / −47.2%) |
| put 4 KiB | −44.9% (−47.3% / −42.3%) |
| fanout put 1000 × 4 KiB, 32 at once | −44.3% (−43.6% / −45.0%) |
| overwrite 4 KiB | −42.3% (−40.0% / −44.5%) |
| fanout put 1000 × 4 KiB, 64 at once | −41.5% (−41.4% / −41.6%) |
| move dir 200 × 64 KiB | −40.7% (−39.8% / −41.5%) |
| write at 4 KiB in 32 MiB | −12.2% (−5.3% / −18.7%) |
| append 4 KiB to 64 MiB | −11.1% (−19.7% / −1.5%) |
| delete 4 KiB, middle of 64 MiB | −9.1% (−26.6% / +12.6%) |
| fanout get 1000 × 4 KiB, 32 at once | −8.9% (−5.9% / −11.8%) |
| truncate 4 KiB, end of 64 MiB | −8.5% (−19.4% / +4.0%) |
| patch 16 × 4 KiB in 1 MiB | −6.5% (−10.6% / −2.2%) |
| insert 4 KiB, start of 32 MiB | −6.4% (−2.7% / −10.0%) |
| fanout get 200 × 256 KiB, 32 at once | −6.2% (−8.1% / −4.3%) |
| append 4 KiB to 32 MiB | −5.7% (−2.5% / −8.7%) |
| put 64 MiB | −5.3% (−13.3% / +3.4%) |
| truncate 4 KiB, end of 32 MiB | −5.2% (+0.5% / −10.5%) |
| get 64 MiB | −5.1% (−8.8% / −1.2%) |
| range 64 KiB of 64 MiB | +5.1% (+11.8% / −1.2%) |
| multipart put 64 MiB × 8 MiB | +5.1% (−6.2% / +17.9%) |
| insert 4 KiB, middle of 64 MiB | +6.4% (+12.1% / +1.0%) |
| patch 16 × 4 KiB in 64 MiB | +6.8% (+0.4% / +13.7%) |
| get 4 KiB | +7.0% (+8.6% / +5.4%) |
| patch 16 × 4 KiB in 32 MiB | +7.6% (+11.4% / +4.0%) |
| delete 4 KiB, start of 64 MiB | +9.1% (+1.8% / +16.9%) |
| stream get 64 MiB | +11.3% (+21.8% / +1.7%) |
| write at 4 KiB in 64 MiB | +11.6% (+23.1% / +1.2%) |
| list 200 keys | +17.0% (+9.7% / +24.7%) |
| multipart put 256 MiB × 16 MiB | +26.0% (+52.4% / +4.1%) |

- `rtt12-branch-2` ran slow throughout, the bare bucket too (stream get 64 MiB: voidfs 117 ms,
  bare 226, against 50–54 and 121–141 in the other runs), which is most of the odd column's large
  figures. Multipart put 256 MiB took 1,047–1,161 ms through voidfs in all four runs; the bare
  bucket's 621–1,003 ms is what moved.
- The slower-looking rows had focused runs, A B B A B A A B at four times the operations: range
  −2.2%, get 4 KiB +0.2%, stream get 64 MiB −1.4%, insert in the middle of 64 MiB −10.4%, delete at
  the start of 64 MiB −2.2%, write at 4 KiB in 64 MiB +1.3%, delete at the start of 32 MiB +0.3%,
  patch in 64 and 32 MiB −4.0% and −3.1%.
- **List 200 keys stays slower:** +18.1% in the focused runs, slower in all four pairs (p50 1.0 to
  1.1 ms, where the bare bucket takes about 40), and +11.2% (+6% +9% +21% +9%) with `diag2`'s
  holding off and on in one binary, so the hold causes it. It is still 31–38× faster than the bare
  bucket in the full runs (`main` 39–42×), where SpaceFS's ratio is 9.1×. Not explained: the list's drive was filled 8 puts at
  a time, which now land in entries of about 8, not 4, but listing a state built either way takes
  the same 29–32 µs in process; the harness's list requests do not commit, and the server does not
  reach the bucket to answer them. On loopback, where the setup's entries are not held, it was
  +4.8% against `main` (every pair) but +4.1% with holding off and on in one binary, the direction
  differing between pairs.

## On loopback

Two sets, A B B A then B A A B, compared by position over all eight runs: the geometric mean of the
p50 ratios is **0.975** relative to the bare bucket (median 0.997), 0.978 raw. By family, relative to
bare: writes 1.036, edits 0.949, metadata 0.879, reads 1.015.

| Runs | Rows at or ahead of SpaceFS's ratio | Rows faster than the bare bucket | Geometric mean speed-up over bare |
|---|--:|--:|--:|
| `loopback-main-1` to `-4` | 31–35 (writes 7–10) | 31–33 | 3.3–3.7× |
| `loopback-branch-1` to `-4` | 33–35 (writes 8–10) | 30–31 | 3.4–3.9× |

The writes' 1.036 is mostly the bare bucket's: put 32 MiB (+27%) had one `main` run whose bare
bucket took 403 ms against 83–106 in the others, and the multipart uploads' bare times ranged
240–528 ms and 1.2–3.4 s. Truncate at the end of 32 MiB (+41%) had a `main` run whose bare bucket
took 294 ms against 95–114, and a branch run at 11 ms against 4.9–7.0. The edits inside 32 and 64 MiB files run 32 operations a round on
loopback and vary by 2× between identical runs, as before.

## What the counters show

Per operation, the focused runs at 12 ms: log entries fell from 0.250 to 0.13–0.14 for put and
overwrite 4 KiB, rename and the folder move, from 0.064 to 0.041 and from 0.032 to 0.022 for the
fan-out puts. The edits' log entries moved by up to 0.03 per operation either way. In the full
runs, other requests moved only where they vary between runs anyway: patch 16 × 4 KiB in 64 MiB reads 5.1–5.6 shards
back per operation (eight 64 MiB working sets overflow the cache, as before), and the shard PUTs of
patches and multipart parts moved by up to 12%.

## Caveats

- One machine: the harness, versitygw, the relay and the server share 15 CPUs. The relay adds its
  delay each way with a timer, about 14–15 ms a round trip in all.
- The hold catches clients that come back within 1–2 ms of an answer, which clients on the same
  machine or network do. Clients further from the server than that (SpaceFS's `client-host`
  topology puts voidfs beside them; the `server-us-east-1` one does not) would not be caught, and
  the tally would then stop holding.
- SpaceFS's figures are from their cloud run, not this setup; the comparison is of each row's ratio
  to the bare bucket.
- The small-write rows at 8 at once and more fall into step with group commit, so their p50 moves a
  few percent between identical runs, and by position; each focused figure above is a median of
  four.
