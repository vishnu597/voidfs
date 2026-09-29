# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T214147Z` |
| When | 2026-09-29T21:41:47Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 41d489e |
| distance to the bucket | none (loopback) |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **27 of 49** scenarios and slower in the other **22**. Geometric mean speed-up over the bare bucket: **3.1×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 387 | 1.0 | 382× faster | 18× faster |
| rename 64 MiB | 105 | 0.81 | 130× faster | 7.9× faster |
| insert 4 KiB, start of 64 MiB | 227 | 5.4 | 42× faster | 6.8× faster |
| append 4 KiB to 64 MiB | 214 | 6.2 | 35× faster | 15× faster |
| list 200 keys | 37.6 | 1.1 | 34× faster | 9.1× faster |
| delete 4 KiB, start of 64 MiB | 259 | 7.7 | 33× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 203 | 6.3 | 32× faster | 13× faster |
| write at 4 KiB in 64 MiB | 205 | 9.7 | 21× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 226 | 12.2 | 18× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 113 | 7.1 | 16× faster | 3.5× faster |
| append 4 KiB to 32 MiB | 112 | 7.3 | 15× faster | 8.0× faster |
| delete 4 KiB, middle of 64 MiB | 186 | 13.0 | 14× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 100 | 7.1 | 14× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 116 | 9.3 | 12× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 118 | 11.3 | 10× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 103 | 11.8 | 8.7× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 109 | 15.3 | 7.1× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.46 | 5.8× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.6 | 0.94 | 4.9× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.0 | 0.22 | 4.7× faster | 34× faster |
| head | 0.74 | 0.19 | 4.0× faster | 13× faster |
| get 4 KiB | 0.84 | 0.22 | 3.7× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.82 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 225 | 116 | 1.9× faster | 2.1× faster |
| get 1 MiB | 1.1 | 0.72 | 1.5× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 105 | 75.2 | 1.4× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 342 | 297 | 1.1× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,352 | 1,364 | 1.0× slower | 1.8× slower |
| write at 4 KiB in 1 MiB | 4.7 | 4.9 | 1.0× slower | 1.6× slower |
| stream get 64 MiB | 46.4 | 50.0 | 1.1× slower | 15× faster |
| stream get 256 MiB | 189 | 207 | 1.1× slower | 16× faster |
| get 32 MiB | 24.1 | 26.8 | 1.1× slower | 12× faster |
| delete 4 KiB, start of 1 MiB | 4.8 | 5.4 | 1.1× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 5.4 | 6.1 | 1.1× slower | 2.1× slower |
| get 64 MiB | 46.2 | 52.6 | 1.1× slower | 17× faster |
| insert 4 KiB, start of 1 MiB | 4.6 | 5.9 | 1.3× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 5.1 | 6.7 | 1.3× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 5.2 | 7.2 | 1.4× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 4.9 | 7.0 | 1.4× slower | 1.0× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.9 | 5.7 | 1.5× slower | 2.2× slower |
| overwrite 4 KiB | 1.4 | 2.3 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.6 | 9.3 | 1.7× slower | 2.4× slower |
| put 64 MiB | 191 | 366 | 1.9× slower | 1.1× slower |
| put 4 KiB | 1.0 | 2.0 | 2.0× slower | 2.0× slower |
| put 32 MiB | 94.7 | 189 | 2.0× slower | 1.2× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.4 | 9.3 | 2.1× slower | 2.8× slower |
| overwrite 1 MiB | 4.4 | 10.7 | 2.4× slower | 1.9× slower |
| put 1 MiB | 3.4 | 8.7 | 2.5× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 5.2 | 27.2 | 5.3× slower | 1.5× slower |
