# Group commit: before and after

*28 September 2026, one Mac (Apple M5 Pro, 15 CPUs, macOS 27). The bucket (versitygw 1.8.0) 12 ms
away through `BENCH_ONE_WAY_MS=4 bench/scripts/local.sh`, and on loopback. voidfs target only is
compared between builds; each full run also has the bare bucket's rows, which the score against
SpaceFS uses.*

| Files | voidfs-server |
|---|---|
| `main-1`, `main-2`, `probe-main-c*`, `loopback-main-*` | `main` at `ae3a466`: one log entry per mutation, written while the drive's commit lock is held |
| `group-1`, `group-2`, `probe-group-c*`, `loopback-group-*` | This branch: group commit. Mutations that queue while a log entry is being written go into the next entry together |

The `commit` label in the files says `ae3a466 (with uncommitted changes)` for all of them,
because the runs passed the server binary with `BENCH_SERVER_BIN`. This table says which is
which.

Run order:
1. The probes, alternating builds: group and main at 1, main and group at 2, group and main at
   4, main and group at 8.
2. The full runs at 12 ms: `main-1`, `group-1`, `group-2`, `main-2` (A B B A, because
   consecutive full runs alternate whichever build runs; see [checkpoints](../checkpoints/README.md)).
3. The same on loopback.
4. Focused runs of the rows that came out slower, alternating builds: the multipart rows at 12 ms
   (8 at once and one at a time) and on loopback, and the 1 MiB puts on loopback. Their files
   are not kept; the tables below have their figures.

## What to expect

A write row's p50 was its concurrency times one conditional PUT: every mutation on a drive
waited for the log in turn. With group commit, the mutations waiting while an entry is written
all go into the next one. A write should take about two or three round trips at any
concurrency: its shards, perhaps the entry already in flight, then its own.

## The concurrency probe

Rename 64 MiB and put 4 KiB, at 1, 2, 4 and 8 at once (`--targets voidfs --scenario rename-64m
--scenario put-4k --ops-scale 0.5`). p50 in ms, p90 in brackets:

| | 1 | 2 | 4 | 8 |
|---|--:|--:|--:|--:|
| rename 64 MiB, main | 13.8 (15.0) | 27.4 (29.2) | 54.4 (58.0) | 107.3 (111.5) |
| rename 64 MiB, group commit | 13.9 (15.0) | 20.7 (27.5) | 26.1 (27.1) | 26.3 (27.2) |
| put 4 KiB, main | 28.9 (30.4) | 27.8 (30.1) | 56.0 (58.8) | 110.3 (115.2) |
| put 4 KiB, group commit | 28.7 (30.2) | 28.1 (30.4) | 28.1 (41.8) | 29.7 (44.4) |

The p50 stays flat from 1 to 8 at once: a rename at about two round trips, a put at about two
and a half. Alone, both builds are the same. At 4 and 8, a put's p90 rises to about three and a
half round trips: a put whose shards land just after an entry leaves waits for that entry, then
for its own.

## Result at 12 ms

Comparing runs in the same position (`main-1` with `group-2`, and `group-1` with `main-2`), the
geometric mean of the p50 ratios over the 49 rows is **0.504** and the median 0.479. By family:
writes 0.325, edits 0.464, metadata 0.482, reads 1.017. No run had errors.

Against SpaceFS's ratios to the bare bucket:

| Run | Rows at or ahead of SpaceFS | Edits (24) | Writes (11) | Reads (10) | Metadata (4) | Rows faster than the bare bucket |
|---|--:|--:|--:|--:|--:|--:|
| `main-1` | 6 | 0 | 1 | 3 | 2 | 26 |
| `group-1` | 17 | 10 | 2 | 3 | 2 | 24 |
| `group-2` | 17 | 9 | 2 | 3 | 3 | 26 |
| `main-2` | 7 | 0 | 2 | 3 | 2 | 29 |

The rows faster than the bare bucket did not change with the build. The six that differ between
runs sit near parity in all four runs and cross it either way: get 32 MiB, two fan-out gets, the
two patches in 32 and 64 MiB, and multipart put 256 MiB (whose bare time in `main-2` was 2.3 s).

Rows that moved by 3% or more:

| Scenario | main (ms) | group commit (ms) | Change | odd / even |
|---|--:|--:|--:|--:|
| fanout put 1000 × 4 KiB, 64 at once | 850.8 | 38.0 | −95.5% | −95.8% / −95.2% |
| fanout put 1000 × 4 KiB, 32 at once | 424.9 | 37.4 | −91.2% | −91.8% / −90.6% |
| fanout put 200 × 256 KiB, 32 at once | 386.6 | 37.7 | −90.3% | −90.7% / −89.8% |
| rename 64 MiB | 101.9 | 25.1 | −75.3% | −77.4% / −73.0% |
| move dir 200 × 64 KiB | 101.1 | 26.2 | −74.1% | −75.5% / −72.6% |
| put 4 KiB | 106.2 | 28.3 | −73.4% | −75.2% / −71.4% |
| overwrite 4 KiB | 106.2 | 30.0 | −71.8% | −71.4% / −72.1% |
| append 4 KiB to 32 MiB | 95.8 | 34.4 | −64.0% | −66.0% / −62.0% |
| truncate 4 KiB, end of 1 MiB | 97.1 | 35.5 | −63.4% | −65.6% / −61.2% |
| append 4 KiB to 64 MiB | 94.5 | 34.8 | −63.2% | −62.8% / −63.6% |
| insert 4 KiB, middle of 1 MiB | 96.5 | 35.7 | −63.0% | −65.5% / −60.3% |
| overwrite 1 MiB | 86.9 | 32.2 | −63.0% | −62.8% / −63.2% |
| put 1 MiB | 86.7 | 32.2 | −62.9% | −62.5% / −63.2% |
| delete 4 KiB, start of 1 MiB | 96.3 | 36.3 | −62.3% | −64.2% / −60.3% |
| append 4 KiB to 1 MiB | 95.8 | 36.1 | −62.3% | −63.9% / −60.6% |
| insert 4 KiB, start of 1 MiB | 97.0 | 36.9 | −62.0% | −64.3% / −59.5% |
| delete 4 KiB, middle of 1 MiB | 94.6 | 36.1 | −61.8% | −63.5% / −60.0% |
| write at 4 KiB in 1 MiB | 95.4 | 37.1 | −61.2% | −65.6% / −56.3% |
| insert 4 KiB, start of 64 MiB | 97.8 | 38.7 | −60.4% | −64.6% / −55.8% |
| truncate 4 KiB, end of 64 MiB | 95.9 | 38.8 | −59.6% | −56.4% / −62.6% |
| insert 4 KiB, start of 32 MiB | 92.9 | 37.7 | −59.4% | −61.1% / −57.6% |
| delete 4 KiB, start of 64 MiB | 95.5 | 43.0 | −55.1% | −52.7% / −57.4% |
| write at 4 KiB in 64 MiB | 96.5 | 45.5 | −52.9% | −54.3% / −51.4% |
| write at 4 KiB in 32 MiB | 92.1 | 43.9 | −52.4% | −55.4% / −49.1% |
| delete 4 KiB, start of 32 MiB | 92.3 | 44.2 | −52.1% | −52.8% / −51.4% |
| truncate 4 KiB, end of 32 MiB | 93.1 | 44.7 | −52.0% | −52.6% / −51.4% |
| insert 4 KiB, middle of 32 MiB | 97.3 | 47.5 | −51.2% | −46.2% / −55.7% |
| insert 4 KiB, middle of 64 MiB | 97.1 | 47.5 | −51.1% | −51.6% / −50.6% |
| patch 16 × 4 KiB in 1 MiB | 90.5 | 45.5 | −49.8% | −50.1% / −49.5% |
| delete 4 KiB, middle of 32 MiB | 95.1 | 51.4 | −46.0% | −46.4% / −45.5% |
| delete 4 KiB, middle of 64 MiB | 94.0 | 51.2 | −45.6% | −47.3% / −43.9% |
| get 4 KiB | 0.4 | 0.3 | −13.2% | −25.9% / +1.7% |
| list 200 keys | 1.2 | 1.0 | −11.1% | −6.6% / −15.3% |
| stream get 256 MiB | 196.6 | 180.3 | −7.9% | +0.4% / −15.6% |
| head | 0.3 | 0.2 | −5.0% | −17.5% / +9.4% |
| fanout get 200 × 256 KiB, 32 at once | 14.1 | 15.2 | +7.7% | −6.6% / +24.2% |
| multipart put 64 MiB × 8 MiB | 398.7 | 430.7 | +8.0% | +5.5% / +10.5% |
| get 32 MiB | 63.3 | 72.5 | +15.8% | −11.7% / +51.7% |
| get 1 MiB | 1.2 | 1.4 | +18.5% | +13.3% / +24.0% |
| patch 16 × 4 KiB in 64 MiB | 221.1 | 297.1 | +28.6% | −3.8% / +72.1% |

- Every edit, every put and overwrite of 4 KiB or 1 MiB, the fan-out puts, the rename and the
  folder move are 46–96% faster. Puts of 32 and 64 MiB, which spend their time taking in the
  body, did not change.
- The reads that moved are the sub-millisecond and cache-bound rows that vary most between
  identical runs, and they moved both ways. Patch in 64 MiB is bimodal, about 210 or 380 ms in
  either build; `group-1` drew the slow mode.
- Multipart put 64 MiB was slower in both positions, but not per upload, and not in throughput;
  see [Multipart uploads](#multipart-uploads).

## The four small-write rows garbage collection made slower

[Garbage collection](../gc/README.md) left overwrite 4 KiB and the three fan-out puts 3.5–7%
slower, for a reason not found, and those rows were to be measured again after group commit.
Medians of the runs, in ms:

| Scenario | Before garbage collection | With it (`main` here) | Group commit | Bare bucket | SpaceFS's ratio would need |
|---|--:|--:|--:|--:|--:|
| overwrite 4 KiB | 104.6 | 106.2 | 30.0 | 13.6 | 42 |
| fanout put 1000 × 4 KiB, 32 at once | 411.4 | 424.9 | 37.4 | 12.1 | 27 |
| fanout put 1000 × 4 KiB, 64 at once | 829.6 | 850.8 | 38.0 | 12.5 | 30 |
| fanout put 200 × 256 KiB, 32 at once | 368.4 | 386.6 | 37.7 | 13.6 | 38 |

The difference garbage collection made was a few percent of a time set by one log write per
commit; that time is gone. Measured without the queue, at one at a time in the probe, a 4 KiB
put took the same in both builds (28.7 and 28.9 ms). What these rows now cost is round trips:
the shards, the log entry in flight, then the rows' own. Whether garbage collection's write path
still adds a fixed cost per commit would take a diagnostic build (group commit with the old
write path), which was not made.

## What still holds rows back at 12 ms

32 rows are behind SpaceFS's ratio in both group-commit runs (two more, delete 4 KiB at the
start of 64 MiB and move dir, cross it between runs).

- **Writes (9).** Fan-out put 200 × 256 KiB and put 4 KiB are within 2–4% of it, fan-out puts of
  4 KiB and the 1 MiB puts within 16–28%. A put still takes its shards, then the log: two round
  trips one after the other ([step 3](../../../docs/step-3-performance.md), item 3). Puts of 32
  and 64 MiB are about half as far ahead as SpaceFS (ingest, item 3), and multipart put 256 MiB
  crosses SpaceFS's ratio from run to run as before.
- **Edits (14).** The edits inside 32 and 64 MiB files take 34–51 ms where SpaceFS's ratio needs
  17–40. The three patches need patch to rewrite each shard once (item 4).
- **Reads (8).** The shard cache's admission (item 2) for the three fan-out gets and get 32 MiB;
  large gets and streams can't be judged without a bandwidth limit.
- **Rename 64 MiB** takes two round trips at 8 at once (25 ms, where SpaceFS's ratio needs 18.5).
  A rename is one log write, so this is waiting for the entry in flight. When an entry lands and
  its eight requests are answered, the first of the next eight to arrive starts an entry alone,
  and the others wait for it. Holding the log for a fraction of a millisecond before an entry,
  while the previous one had company, would let them go together; not tried.

## Multipart uploads

Multipart put 64 MiB came out 8% slower at 12 ms, in both positions. Focused runs of the two
multipart rows, with put 64 MiB as a control, alternating builds (at 12 ms, 8 at once; p50 in ms,
and for each of the two rounds, the p90 and the time the round took):

| | main | group | group | main | main | group |
|---|--:|--:|--:|--:|--:|--:|
| multipart put 64 MiB × 8 MiB, p50 | 407.8 | 445.2 | 443.5 | 412.3 | 414.6 | 430.4 |
| its p90 | 512, 529 | 473, 464 | 457, 466 | 505, 507 | 482, 522 | 454, 445 |
| its rounds (s) | 0.92, 0.91 | 0.93, 0.90 | 0.91, 0.94 | 0.91, 0.92 | 0.90, 0.93 | 0.91, 0.90 |
| multipart put 256 MiB × 16 MiB, p50 | 1193.9 | 1243.1 | 1258.3 | 1185.0 | 1181.3 | 1322.3 |
| put 64 MiB, p50 | 369.9 | 374.6 | 371.1 | 371.5 | 368.6 | 370.6 |

And one at a time (at 12 ms, p50 in ms):

| | main | group | group | main |
|---|--:|--:|--:|--:|
| multipart put 64 MiB × 8 MiB | 268.5 | 269.4 | 268.6 | 270.7 |
| multipart put 256 MiB × 16 MiB | 484.1 | 485.8 | 482.8 | 485.2 |

- One at a time, an upload takes the same in both builds.
- Eight at once, the rounds take the same time, so throughput is unchanged, but the p50 is
  4–9% higher and the p90 about 10% lower. The harness uploads all of an operation's parts at
  once, from 64 connections shared by the whole run: eight uploads of eight parts fill them. With
  one commit at a time, the eight completions landed one after another and the next uploads
  started spread out. Now they land in one log entry, the next eight start together, and their
  parts compete for the connections and the bucket at the same moment.
- Multipart put 256 MiB (16 parts, twice as many as there are connections) moved the same way
  here, and its bare bucket varied as much as it did (546–1,079 ms in these runs).

## On loopback

The same A B B A on loopback, where round trips cost about 0.3 ms: this is where the committer's
own costs would show (a task per mutation, planning in order, encoding each transaction to
measure it). Files `loopback-main-*` and `loopback-group-*`; no run had errors.

- Across the 49 rows, compared by position, the geometric mean of the p50 ratios is 0.897 and the
  median 0.983. Relative to the bare bucket in the same run, 0.803: writes 0.712, edits 0.807,
  metadata 0.599, reads 1.019.
- The commit-bound rows are faster here too: rename 2.6 → 1.0 ms, move dir 2.9 → 1.1, put 4 KiB
  3.4 → 2.3, fan-out puts 11.7 / 23.1 / 21.8 → 6.1 / 10.5 / 10.2 ms (1,000 × 4 KiB at 32 and 64,
  200 × 256 KiB).
- Rows at or ahead of SpaceFS's ratio: 29–30, against 20–21 for `main` (loopback is even further
  from SpaceFS's setup than the 12 ms runs).
- Some disk-bound rows came out slower in both positions: multipart puts by 38%, put 32 MiB by
  12%. The bare bucket was slower in the same runs by about as much (multipart put 64 MiB: 293
  and 281 ms in `main`'s runs, 392 and 376 in the branch's), so the local disk had slowed. In
  focused runs alternating the builds, the disk slowed steadily through the sequence for both,
  and relative to the bare bucket the builds overlap:

  | Loopback, p50 over the bare bucket's | main | group | group | main | main | group |
  |---|--:|--:|--:|--:|--:|--:|
  | multipart put 64 MiB × 8 MiB | 0.87 | 1.05 | 0.85 | 0.84 | 0.85 | 0.91 |
  | multipart put 256 MiB × 16 MiB | 0.86 | 0.79 | 0.86 | 0.93 | 0.92 | 0.89 |
  | put 64 MiB | 1.99 | 2.01 | 2.08 | 2.10 | 2.15 | 2.10 |

  Put and overwrite 1 MiB, 17% and 5% slower in the full runs relative to the bare bucket, were
  within 3% in focused runs (`main` first, then the branch: 7.2–7.6 and 7.5–7.7 ms for put 1 MiB,
  7.1–7.5 and 7.6–8.2 ms for overwrite 1 MiB), and put 4 KiB was faster (2.8–3.0 against
  1.6–2.0 ms).
