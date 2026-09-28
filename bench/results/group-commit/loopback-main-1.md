# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T193042Z` |
| When | 2026-09-28T19:30:42Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | ae3a466 (with uncommitted changes) |
| distance to the bucket | none (loopback) |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **27 of 49** scenarios and slower in the other **22**. Geometric mean speed-up over the bare bucket: **2.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 379 | 3.3 | 115× faster | 18× faster |
| rename 64 MiB | 114 | 2.7 | 42× faster | 7.9× faster |
| list 200 keys | 34.5 | 1.1 | 30× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 207 | 9.1 | 23× faster | 13× faster |
| write at 4 KiB in 64 MiB | 200 | 10.9 | 18× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 265 | 15.6 | 17× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 223 | 14.4 | 15× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 185 | 13.4 | 14× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 186 | 15.7 | 12× faster | 15× faster |
| delete 4 KiB, middle of 64 MiB | 206 | 20.4 | 10× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 107 | 10.8 | 10.0× faster | 5.9× faster |
| insert 4 KiB, start of 32 MiB | 111 | 12.5 | 8.8× faster | 3.5× faster |
| append 4 KiB to 32 MiB | 98.7 | 11.5 | 8.6× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 99.3 | 12.2 | 8.2× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 104 | 14.0 | 7.4× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 103 | 15.3 | 6.7× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 92.9 | 17.5 | 5.3× faster | 4.8× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.24 | 4.6× faster | 34× faster |
| head | 0.74 | 0.19 | 4.0× faster | 13× faster |
| get 4 KiB | 0.84 | 0.22 | 3.9× faster | 23× faster |
| get 1 MiB | 1.4 | 0.97 | 1.4× faster | 4.5× faster |
| patch 16 × 4 KiB in 64 MiB | 185 | 133 | 1.4× faster | 2.1× faster |
| multipart put 64 MiB × 8 MiB | 293 | 263 | 1.1× faster | 2.4× slower |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 2.3 | 1.1× faster | 5.3× faster |
| multipart put 256 MiB × 16 MiB | 1,078 | 974 | 1.1× faster | 1.8× slower |
| patch 16 × 4 KiB in 32 MiB | 94.2 | 89.7 | 1.1× faster | 1.4× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.3 | 4.3 | 1.0× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.4 | 2.7 | 1.1× slower | 6.2× faster |
| stream get 256 MiB | 201 | 220 | 1.1× slower | 16× faster |
| stream get 64 MiB | 47.7 | 52.9 | 1.1× slower | 15× faster |
| get 64 MiB | 46.0 | 59.3 | 1.3× slower | 17× faster |
| append 4 KiB to 1 MiB | 4.7 | 6.3 | 1.4× slower | 1.0× faster |
| write at 4 KiB in 1 MiB | 4.1 | 6.2 | 1.5× slower | 1.6× slower |
| get 32 MiB | 28.1 | 45.4 | 1.6× slower | 12× faster |
| insert 4 KiB, middle of 1 MiB | 4.9 | 8.0 | 1.6× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 4.5 | 7.6 | 1.7× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 4.5 | 7.7 | 1.7× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 4.3 | 7.7 | 1.8× slower | 1.5× slower |
| put 32 MiB | 76.3 | 154 | 2.0× slower | 1.2× slower |
| put 64 MiB | 148 | 299 | 2.0× slower | 1.1× slower |
| truncate 4 KiB, end of 1 MiB | 4.8 | 10.2 | 2.1× slower | 2.1× slower |
| put 1 MiB | 3.1 | 7.2 | 2.3× slower | 1.7× slower |
| overwrite 1 MiB | 3.2 | 7.6 | 2.4× slower | 1.9× slower |
| overwrite 4 KiB | 1.1 | 3.0 | 2.9× slower | 3.1× slower |
| put 4 KiB | 0.89 | 3.1 | 3.5× slower | 2.0× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.5 | 23.7 | 3.6× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.3 | 12.2 | 3.7× slower | 2.2× slower |
| patch 16 × 4 KiB in 1 MiB | 4.3 | 20.1 | 4.7× slower | 1.5× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.8 | 19.9 | 5.3× slower | 2.8× slower |
