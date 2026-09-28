# Bucket capability probe: before and after

*28 September 2026, one Mac (Apple M5 Pro, macOS 27). All 49 scenarios through
`BENCH_ONE_WAY_MS=4 bench/scripts/local.sh`, with the bucket (versitygw 1.8.0) 12 ms away. voidfs
target only is compared here; each file also has the bare bucket's rows.*

| Files | voidfs-server |
|---|---|
| `main-1`, `main-2` | `main` at `a068b2c` |
| `probe-1`, `probe-2` | This branch: the capability probe at start, and every create-if-absent write through the pool's commit guard |

The `commit` label in the files says `a068b2c (with uncommitted changes)` for all four, because
the runs passed the server binary with `BENCH_SERVER_BIN`. This table says which is which.

Run order: `main-1`, `probe-1`, `probe-2`, `main-2` (A B B A, because consecutive full runs
alternate whichever build runs; see [checkpoints](../checkpoints/README.md)).

## What to expect

Nothing. The probe runs once, before the server listens. With the default guard, a commit sends
the same requests as before; it only goes through `Pool::create`, which chooses between the two
guards.

## Result

The branch is at parity. Comparing runs in the same position (`main-1` with `probe-2`, and
`probe-1` with `main-2`), the geometric mean of the p50 ratios over the 49 rows is 0.993 and the
median 0.993. By family: edits 0.995, metadata 0.972, reads 0.984, writes 1.003. No run had
errors.

Rows that moved by 3% or more on that basis:

| Scenario | main (ms) | branch (ms) | Change | odd / even |
|---|--:|--:|--:|--:|
| get 4 KiB | 0.4 | 0.3 | −13.8% | −37.3% / +18.5% |
| head | 0.3 | 0.2 | −9.8% | −10.6% / −8.9% |
| patch 16 × 4 KiB in 64 MiB | 227.7 | 242.6 | +6.6% | +15.5% / −1.6% |
| insert 4 KiB, middle of 32 MiB | 99.4 | 93.7 | −5.7% | −6.5% / −4.8% |
| multipart put 256 MiB × 16 MiB | 1123.2 | 1179.8 | +5.0% | +6.1% / +4.0% |
| get 32 MiB | 64.2 | 60.8 | −4.7% | −17.6% / +10.2% |
| insert 4 KiB, start of 64 MiB | 94.3 | 90.0 | −4.6% | −4.3% / −4.8% |
| stream get 64 MiB | 48.8 | 46.6 | −4.6% | −1.4% / −7.6% |
| fanout get 200 × 256 KiB, 32 at once | 14.1 | 14.7 | +4.2% | +3.9% / +4.6% |
| get 1 MiB | 1.3 | 1.4 | +4.2% | +1.0% / +7.4% |
| delete 4 KiB, start of 64 MiB | 90.6 | 94.3 | +4.1% | +1.3% / +7.0% |
| write at 4 KiB in 32 MiB | 90.7 | 94.2 | +3.9% | +6.1% / +1.8% |

- The reads are the sub-millisecond and cache-bound rows that vary most between identical runs,
  and patch in 64 MiB is bimodal, as before.
- The edits inside 32 and 64 MiB files moved both ways, by 4–6%.
- Multipart put 256 MiB was 5% slower in both positions. Six focused runs of the two multipart
  rows, alternating the builds, showed no difference (p50 in ms):

  | | main | branch | branch | main | main | branch |
  |---|--:|--:|--:|--:|--:|--:|
  | multipart put 256 MiB × 16 MiB | 1210.0 | 1218.6 | 1190.6 | 1235.7 | 1219.8 | 1211.1 |
  | multipart put 64 MiB × 8 MiB | 416.0 | 412.3 | 383.3 | 409.7 | 408.9 | 404.1 |

The last 40 seconds of `main-1` (15:44:38 to 15:45:17 UTC) overlapped a probe of Cloudflare R2
from the same Mac: 26 requests and one short start of a debug server. Judged by their round
times, the rows then running were the last four: fan-out put 1000 × 4 KiB at 64, multipart put
64 MiB, fan-out put 200 × 256 KiB and overwrite 4 KiB. None of them moved by 3% or more. The
256 MiB multipart put ran about 90 seconds before the end, outside that window.
