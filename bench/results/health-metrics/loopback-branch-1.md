# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T141903Z` |
| When | 2026-09-29T14:19:03Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | acfb616 (with uncommitted changes) |
| distance to the bucket | none (loopback) |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **27 of 49** scenarios and slower in the other **22**. Geometric mean speed-up over the bare bucket: **3.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 396 | 0.99 | 398× faster | 18× faster |
| rename 64 MiB | 120 | 0.90 | 134× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 224 | 3.1 | 72× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 231 | 7.0 | 33× faster | 13× faster |
| list 200 keys | 35.8 | 1.1 | 33× faster | 9.1× faster |
| delete 4 KiB, start of 64 MiB | 233 | 9.3 | 25× faster | 6.9× faster |
| insert 4 KiB, start of 64 MiB | 212 | 9.1 | 23× faster | 6.8× faster |
| append 4 KiB to 32 MiB | 99.6 | 4.4 | 23× faster | 8.0× faster |
| write at 4 KiB in 64 MiB | 184 | 10.7 | 17× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 221 | 12.9 | 17× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 178 | 11.7 | 15× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 107 | 7.1 | 15× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 106 | 9.1 | 12× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 110 | 11.6 | 9.5× faster | 4.8× faster |
| insert 4 KiB, start of 32 MiB | 117 | 12.3 | 9.5× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 99.8 | 10.9 | 9.1× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 107 | 14.7 | 7.3× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.45 | 5.6× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 5.0 | 0.93 | 5.4× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.2 | 0.26 | 4.5× faster | 34× faster |
| head | 0.74 | 0.19 | 3.9× faster | 13× faster |
| get 4 KiB | 0.90 | 0.26 | 3.5× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.80 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 202 | 114 | 1.8× faster | 2.1× faster |
| get 1 MiB | 1.2 | 0.66 | 1.8× faster | 4.5× faster |
| multipart put 64 MiB × 8 MiB | 416 | 318 | 1.3× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 103 | 86.0 | 1.2× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,467 | 1,545 | 1.1× slower | 1.8× slower |
| get 64 MiB | 45.6 | 49.3 | 1.1× slower | 17× faster |
| truncate 4 KiB, end of 1 MiB | 4.3 | 4.7 | 1.1× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 5.4 | 6.1 | 1.1× slower | 1.7× slower |
| stream get 256 MiB | 175 | 199 | 1.1× slower | 16× faster |
| stream get 64 MiB | 45.5 | 52.9 | 1.2× slower | 15× faster |
| get 32 MiB | 22.8 | 26.6 | 1.2× slower | 12× faster |
| append 4 KiB to 1 MiB | 4.7 | 5.6 | 1.2× slower | 1.0× faster |
| delete 4 KiB, start of 1 MiB | 5.1 | 6.3 | 1.2× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 4.8 | 6.2 | 1.3× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 5.3 | 6.8 | 1.3× slower | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 5.4 | 7.2 | 1.3× slower | 1.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.2 | 5.1 | 1.6× slower | 2.2× slower |
| overwrite 4 KiB | 1.3 | 2.2 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.5 | 9.7 | 1.8× slower | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.4 | 8.7 | 2.0× slower | 2.8× slower |
| put 32 MiB | 92.7 | 186 | 2.0× slower | 1.2× slower |
| put 64 MiB | 166 | 338 | 2.0× slower | 1.1× slower |
| put 4 KiB | 0.91 | 2.0 | 2.2× slower | 2.0× slower |
| overwrite 1 MiB | 4.1 | 10.1 | 2.5× slower | 1.9× slower |
| put 1 MiB | 3.9 | 10.2 | 2.6× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.5 | 26.7 | 5.9× slower | 1.5× slower |
