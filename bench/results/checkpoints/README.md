# Content-defined checkpoint segments: before and after

*28 September 2026, one Mac (Apple M5 Pro, macOS 27). All 49 scenarios through
`BENCH_ONE_WAY_MS=4 bench/scripts/local.sh`, with the bucket (versitygw 1.8.0) 12 ms away. voidfs
target only is compared here; each file also has the bare bucket's rows.*

| Files | voidfs-server |
|---|---|
| `main-1`, `main-2`, `main-3` | `main` at `f6154da`: segments of 4,096 rows, a checkpoint at every seq that is a multiple of 1,000 |
| `e11-1`, `e11-2`, `e11-3` | This branch: content-defined segments, and a checkpoint 1,000 commits or 16 MiB of log after the last |

The `commit` label in the files says `f6154da (with uncommitted changes)` for all six, because the
runs passed the server binary with `BENCH_SERVER_BIN`. This table says which is which.

Run order: `main-1`, `e11-1`, `main-2`, `e11-2`, then `e11-3`, `main-3`.

## What to expect

Little. The change is to the checkpoint writer, and the benchmark barely checkpoints: each
scenario creates its own drive and deletes it afterwards, and only the fan-out scenarios (1,000
files) commit more than 1,000 times. On every other commit the new code adds a length and two
additions. The server never restarts, so the reuse of the previous checkpoint's pages after a
restart does not come into it.

## Result

The branch is at parity. Comparing runs in the same position (below), the geometric mean of the
p50 ratios over the 49 rows is 1.006 and the median 1.009. By family: edits 0.997, metadata
1.001, reads 1.025, writes 1.009.

## Runs alternate, whichever build is running

The first four runs alternated `main` and the branch, and several rows seemed to move 7–10% with
the build: move dir, rename and appends slower on the branch; fan-out puts and small overwrites
faster. The reversed pair showed that these rows follow the run's position, not the build:

| p50 (ms) | main-1 | e11-1 | main-2 | e11-2 | e11-3 | main-3 |
|---|--:|--:|--:|--:|--:|--:|
| move dir 200 × 64 KiB | 95.7 | 106.3 | 94.7 | 103.6 | 95.6 | 104.3 |
| rename 64 MiB | 96.1 | 105.4 | 97.1 | 106.9 | 96.5 | 109.7 |
| append 4 KiB to 64 MiB | 88.5 | 97.9 | 89.9 | 96.5 | 90.8 | 100.2 |
| fanout put 1000 × 4 KiB, 32 at once | 446.1 | 410.7 | 441.4 | 411.3 | 450.5 | 412.6 |
| fanout put 1000 × 4 KiB, 64 at once | 900.6 | 830.9 | 883.8 | 824.8 | 902.9 | 830.5 |
| overwrite 4 KiB | 111.5 | 104.0 | 109.8 | 103.7 | 112.8 | 104.5 |

The odd runs (1st, 3rd, 5th) agree with each other, and so do the even ones, whichever build ran.
The cause is outside voidfs and was not found. Each run deletes its temporary bucket, about 33 GB,
before the next starts, which is one candidate. **Compare runs by position when alternating
builds**, or alternate in pairs (A B B A).

So the comparison above takes, for each row, the branch's median over the main median in the
same parity (odd: `main-1`, `main-2` against `e11-3`; even: `main-3` against `e11-1`, `e11-2`), and
the geometric mean of the two ratios.

Rows that moved by 3% or more on that basis:

| Scenario | main (ms) | branch (ms) | Change | odd / even |
|---|--:|--:|--:|--:|
| get 4 KiB | 0.3 | 0.4 | +15.8% | +8.9% / +23.1% |
| get 32 MiB | 70.3 | 76.3 | +10.4% | −13.9% / +41.4% |
| fanout get 1000 × 4 KiB, 32 at once | 12.4 | 13.1 | +3.3% | +0.0% / +6.6% |
| write at 4 KiB in 32 MiB | 92.2 | 95.8 | +5.7% | +2.2% / +9.3% |
| delete 4 KiB, middle of 32 MiB | 96.3 | 96.6 | +3.6% | −1.8% / +9.4% |
| delete 4 KiB, start of 32 MiB | 92.5 | 95.2 | +3.4% | +1.8% / +5.1% |
| get 1 MiB | 1.4 | 1.3 | −6.3% | −4.6% / −7.9% |
| insert 4 KiB, middle of 32 MiB | 92.3 | 99.7 | +6.4% | +8.7% / +4.1% |
| insert 4 KiB, start of 32 MiB | 89.6 | 96.6 | +4.8% | +3.1% / +6.6% |
| patch 16 × 4 KiB in 64 MiB | 317.7 | 208.0 | −30.6% | −18.9% / −40.6% |
| patch 16 × 4 KiB in 32 MiB | 144.6 | 140.0 | +4.2% | −3.3% / +12.2% |
| insert 4 KiB, middle of 1 MiB | 93.9 | 95.2 | +3.2% | +5.0% / +1.4% |
| multipart put 256 MiB × 16 MiB | 1216.9 | 1232.0 | +3.5% | +1.0% / +6.0% |
| put 4 KiB | 103.0 | 103.1 | +3.1% | +5.5% / +0.6% |

- The reads are the sub-millisecond and cache-bound rows that vary most between identical runs,
  and patch in 64 MiB is bimodal (about 208 or 318 ms in either build).
- Four 32 MiB edit rows came out 3–9% slower in both parities. Their drives never checkpoint,
  and their commits run the same code in both builds. A focused run of just those four rows,
  alternating the builds twice, showed no difference (p50 in ms):

  | | main-1 | e11-1 | main-2 | e11-2 |
  |---|--:|--:|--:|--:|
  | write at 4 KiB in 32 MiB | 90.1 | 91.1 | 89.1 | 92.0 |
  | delete 4 KiB, start of 32 MiB | 91.4 | 90.2 | 92.6 | 89.0 |
  | insert 4 KiB, middle of 32 MiB | 98.8 | 90.9 | 89.2 | 88.6 |
  | insert 4 KiB, start of 32 MiB | 90.7 | 89.7 | 88.1 | 95.2 |

  A focused run of move dir, rename and fan-out put 32 at once agreed as well: 94–97, 96–99 and
  416–417 ms in both builds.

## The checkpoint writer on its own

Not measured by the benchmark. In a release build on the same Mac, in memory, a checkpoint of
600,000 rows (200,000 files with one version each) takes about 600 ms of CPU. Cutting by key (a
SHA-256 of each row's key) is about 50 ms of that. Copying the state out (`DriveState::rows`) is
280–420 ms, and is unchanged. What the change saves is uploads: a checkpoint stores about one new
page per table a commit touched, where it used to store every page after the first changed row.
