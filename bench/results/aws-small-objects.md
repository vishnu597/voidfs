# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T023035Z` |
| When | 2026-09-29T02:30:35Z |
| Bare bucket | the bucket reached directly (endpoint and name not recorded) |
| voidfs | a voidfs server (endpoint not recorded) |
| bucket | AWS S3, us-east-1, reached from this Mac over a home internet connection |
| client | Darwin 27.0.0, 15 vCPUs |
| commit | 0ff3f2d (main) |
| voidfs server | voidfs-server release build on the client host, over that same bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 23 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Operations | scaled by 0.25 from the harness defaults |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **16 of 23** scenarios and slower in the other **7**. Geometric mean speed-up over the bare bucket: **4.3×** (SpaceFS's, same rows: 1.4×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| get 1 MiB | 341 | 1.7 | 199× faster | 4.5× faster |
| get 4 KiB | 50.8 | 0.31 | 162× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 394 | 3.5 | 114× faster | 6.2× faster |
| head | 35.8 | 0.36 | 99× faster | 13× faster |
| fanout get 1000 × 4 KiB, 32 at once | 52.8 | 1.0 | 51× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 52.5 | 1.7 | 31× faster | 2.6× faster |
| list 200 keys | 61.3 | 2.3 | 26× faster | 9.1× faster |
| move dir 200 × 64 KiB | 3,216 | 125 | 26× faster | 18× faster |
| insert 4 KiB, middle of 1 MiB | 870 | 365 | 2.4× faster | 1.4× slower |
| write at 4 KiB in 1 MiB | 847 | 364 | 2.3× faster | 1.6× slower |
| delete 4 KiB, start of 1 MiB | 807 | 411 | 2.0× faster | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 877 | 466 | 1.9× faster | 1.7× slower |
| append 4 KiB to 1 MiB | 569 | 313 | 1.8× faster | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 793 | 453 | 1.8× faster | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | 690 | 438 | 1.6× faster | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 552 | 436 | 1.3× faster | 2.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 281 | 349 | 1.2× slower | 2.8× slower |
| overwrite 1 MiB | 320 | 401 | 1.3× slower | 1.9× slower |
| put 1 MiB | 292 | 391 | 1.3× slower | 1.7× slower |
| put 4 KiB | 55.8 | 153 | 2.8× slower | 2.0× slower |
| overwrite 4 KiB | 54.8 | 162 | 3.0× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 55.8 | 169 | 3.0× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 59.6 | 208 | 3.5× slower | 2.4× slower |
