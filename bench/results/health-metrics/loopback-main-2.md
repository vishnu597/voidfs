# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T142214Z` |
| When | 2026-09-29T14:22:14Z |
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

voidfs is faster in **28 of 49** scenarios and slower in the other **21**. Geometric mean speed-up over the bare bucket: **3.2×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 382 | 0.96 | 399× faster | 18× faster |
| rename 64 MiB | 105 | 0.75 | 141× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 178 | 2.3 | 78× faster | 15× faster |
| list 200 keys | 35.1 | 1.0 | 35× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 214 | 10.3 | 21× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 190 | 9.7 | 20× faster | 6.8× faster |
| insert 4 KiB, start of 32 MiB | 119 | 6.1 | 19× faster | 3.5× faster |
| delete 4 KiB, start of 64 MiB | 201 | 10.5 | 19× faster | 6.9× faster |
| append 4 KiB to 32 MiB | 116 | 6.4 | 18× faster | 8.0× faster |
| write at 4 KiB in 64 MiB | 189 | 10.9 | 17× faster | 6.2× faster |
| delete 4 KiB, start of 32 MiB | 99.5 | 5.8 | 17× faster | 4.6× faster |
| truncate 4 KiB, end of 32 MiB | 101 | 6.0 | 17× faster | 5.9× faster |
| insert 4 KiB, middle of 64 MiB | 206 | 12.5 | 16× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 189 | 12.6 | 15× faster | 11× faster |
| write at 4 KiB in 32 MiB | 102 | 7.7 | 13× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 108 | 9.0 | 12× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 122 | 11.4 | 11× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.46 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.7 | 0.89 | 5.3× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.22 | 4.7× faster | 34× faster |
| head | 0.73 | 0.18 | 4.0× faster | 13× faster |
| get 4 KiB | 0.81 | 0.21 | 4.0× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.80 | 3.2× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 218 | 102 | 2.1× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 109 | 77.3 | 1.4× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 339 | 300 | 1.1× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,416 | 1,311 | 1.1× faster | 1.8× slower |
| get 1 MiB | 1.0 | 0.99 | 1.0× faster | 4.5× faster |
| get 64 MiB | 47.3 | 50.6 | 1.1× slower | 17× faster |
| stream get 256 MiB | 183 | 197 | 1.1× slower | 16× faster |
| stream get 64 MiB | 44.1 | 47.9 | 1.1× slower | 15× faster |
| get 32 MiB | 23.1 | 26.8 | 1.2× slower | 12× faster |
| insert 4 KiB, start of 1 MiB | 4.5 | 5.2 | 1.2× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 4.3 | 5.1 | 1.2× slower | 1.0× faster |
| truncate 4 KiB, end of 1 MiB | 4.9 | 5.8 | 1.2× slower | 2.1× slower |
| delete 4 KiB, middle of 1 MiB | 4.9 | 6.0 | 1.2× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 4.4 | 5.5 | 1.2× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 4.7 | 5.9 | 1.3× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.7 | 6.0 | 1.3× slower | 1.6× slower |
| fanout put 1000 × 4 KiB, 64 at once | 7.5 | 9.8 | 1.3× slower | 2.4× slower |
| overwrite 4 KiB | 1.3 | 2.2 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.4 | 5.6 | 1.7× slower | 2.2× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.4 | 8.1 | 1.8× slower | 2.8× slower |
| put 64 MiB | 177 | 333 | 1.9× slower | 1.1× slower |
| put 4 KiB | 1.1 | 2.0 | 1.9× slower | 2.0× slower |
| put 32 MiB | 82.4 | 162 | 2.0× slower | 1.2× slower |
| overwrite 1 MiB | 4.3 | 10.6 | 2.5× slower | 1.9× slower |
| put 1 MiB | 3.5 | 9.2 | 2.7× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.4 | 23.3 | 5.3× slower | 1.5× slower |
