# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260927T202141Z` |
| When | 2026-09-27T20:21:41Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | c434fcb (with uncommitted changes) |
| distance to the bucket | none (loopback) |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **26 of 49** scenarios and slower in the other **23**. Geometric mean speed-up over the bare bucket: **2.1×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 396 | 2.9 | 138× faster | 18× faster |
| rename 64 MiB | 113 | 2.6 | 44× faster | 7.9× faster |
| list 200 keys | 34.5 | 1.1 | 31× faster | 9.1× faster |
| insert 4 KiB, start of 64 MiB | 219 | 9.2 | 24× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 183 | 8.3 | 22× faster | 13× faster |
| append 4 KiB to 64 MiB | 188 | 8.7 | 22× faster | 15× faster |
| write at 4 KiB in 64 MiB | 203 | 11.8 | 17× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 194 | 14.6 | 13× faster | 6.9× faster |
| append 4 KiB to 32 MiB | 108 | 9.3 | 12× faster | 8.0× faster |
| delete 4 KiB, middle of 64 MiB | 182 | 17.2 | 11× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 109 | 10.4 | 10× faster | 3.5× faster |
| insert 4 KiB, middle of 64 MiB | 195 | 20.8 | 9.4× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 111 | 13.5 | 8.2× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 94.7 | 13.2 | 7.2× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 110 | 17.7 | 6.2× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 101 | 16.7 | 6.1× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 88.0 | 14.7 | 6.0× faster | 4.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.25 | 4.6× faster | 34× faster |
| head | 0.77 | 0.19 | 4.0× faster | 13× faster |
| get 4 KiB | 0.86 | 0.24 | 3.7× faster | 23× faster |
| patch 16 × 4 KiB in 64 MiB | 203 | 131 | 1.5× faster | 2.1× faster |
| multipart put 256 MiB × 16 MiB | 1,124 | 975 | 1.2× faster | 1.8× slower |
| fanout get 1000 × 4 KiB, 32 at once | 2.4 | 2.2 | 1.1× faster | 5.3× faster |
| get 1 MiB | 1.1 | 1.0 | 1.0× faster | 4.5× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.3 | 4.2 | 1.0× faster | 2.6× faster |
| multipart put 64 MiB × 8 MiB | 254 | 252 | 1.0× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 92.7 | 93.9 | 1.0× slower | 1.4× faster |
| stream get 256 MiB | 185 | 201 | 1.1× slower | 16× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.4 | 2.8 | 1.1× slower | 6.2× faster |
| stream get 64 MiB | 45.9 | 52.3 | 1.1× slower | 15× faster |
| get 64 MiB | 44.6 | 51.2 | 1.1× slower | 17× faster |
| write at 4 KiB in 1 MiB | 4.0 | 6.4 | 1.6× slower | 1.6× slower |
| delete 4 KiB, start of 1 MiB | 4.2 | 6.7 | 1.6× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 4.2 | 6.9 | 1.6× slower | 2.1× slower |
| append 4 KiB to 1 MiB | 4.4 | 7.2 | 1.7× slower | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 4.2 | 7.0 | 1.7× slower | 1.4× slower |
| get 32 MiB | 23.3 | 39.5 | 1.7× slower | 12× faster |
| insert 4 KiB, start of 1 MiB | 4.4 | 7.4 | 1.7× slower | 1.7× slower |
| overwrite 1 MiB | 4.0 | 6.9 | 1.7× slower | 1.9× slower |
| put 32 MiB | 78.6 | 150 | 1.9× slower | 1.2× slower |
| put 64 MiB | 157 | 312 | 2.0× slower | 1.1× slower |
| insert 4 KiB, middle of 1 MiB | 3.8 | 8.3 | 2.2× slower | 1.4× slower |
| overwrite 4 KiB | 1.0 | 2.6 | 2.5× slower | 3.1× slower |
| put 1 MiB | 2.8 | 7.3 | 2.6× slower | 1.7× slower |
| put 4 KiB | 0.88 | 2.7 | 3.0× slower | 2.0× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.8 | 23.3 | 4.0× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 2.9 | 11.9 | 4.2× slower | 2.2× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.6 | 16.2 | 4.5× slower | 2.8× slower |
| patch 16 × 4 KiB in 1 MiB | 4.3 | 20.1 | 4.7× slower | 1.5× slower |
