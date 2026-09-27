# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260927T203839Z` |
| When | 2026-09-27T20:38:39Z |
| Bare bucket | the bucket reached directly (endpoint and name not recorded) |
| voidfs | a voidfs server (endpoint not recorded) |
| bucket | Cloudflare R2 (location not recorded), reached from this Mac over a home internet connection |
| client | Darwin 27.0.0, 15 vCPUs |
| commit | c434fcb (with uncommitted changes) |
| voidfs server | voidfs-server on the client host, over that same bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 23 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Operations | scaled by 0.25 from the harness defaults |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **8 of 23** scenarios and slower in the other **15**. Geometric mean speed-up over the bare bucket: **1.4×** (SpaceFS's, same rows: 1.4×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| get 4 KiB | 128 | 0.39 | 324× faster | 23× faster |
| head | 76.8 | 0.52 | 147× faster | 13× faster |
| fanout get 1000 × 4 KiB, 32 at once | 114 | 1.2 | 94× faster | 5.3× faster |
| get 1 MiB | 166 | 1.9 | 89× faster | 4.5× faster |
| list 200 keys | 176 | 2.4 | 73× faster | 9.1× faster |
| fanout get 200 × 256 KiB, 32 at once | 204 | 3.1 | 67× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 121 | 2.4 | 50× faster | 2.6× faster |
| move dir 200 × 64 KiB | 8,616 | 571 | 15× faster | 18× faster |
| append 4 KiB to 1 MiB | 674 | 1,650 | 2.4× slower | 1.0× faster |
| delete 4 KiB, start of 1 MiB | 579 | 1,612 | 2.8× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 552 | 1,537 | 2.8× slower | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 593 | 1,664 | 2.8× slower | 2.1× slower |
| insert 4 KiB, middle of 1 MiB | 550 | 1,603 | 2.9× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 541 | 1,653 | 3.1× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 540 | 1,696 | 3.1× slower | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | 546 | 1,720 | 3.2× slower | 1.5× slower |
| put 1 MiB | 292 | 1,588 | 5.4× slower | 1.7× slower |
| overwrite 1 MiB | 315 | 1,791 | 5.7× slower | 1.9× slower |
| put 4 KiB | 218 | 1,626 | 7.5× slower | 2.0× slower |
| overwrite 4 KiB | 213 | 1,742 | 8.2× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 319 | 6,534 | 20× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 207 | 6,698 | 32× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 211 | 13,035 | 62× slower | 2.4× slower |
