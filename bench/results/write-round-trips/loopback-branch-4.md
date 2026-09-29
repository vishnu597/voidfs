# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T214321Z` |
| When | 2026-09-29T21:43:21Z |
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

voidfs is faster in **28 of 49** scenarios and slower in the other **21**. Geometric mean speed-up over the bare bucket: **3.1×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 384 | 0.90 | 425× faster | 18× faster |
| rename 64 MiB | 114 | 0.89 | 128× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 258 | 4.1 | 63× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 212 | 5.4 | 40× faster | 6.8× faster |
| list 200 keys | 40.3 | 1.1 | 36× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 221 | 9.3 | 24× faster | 13× faster |
| delete 4 KiB, start of 64 MiB | 230 | 10.2 | 22× faster | 6.9× faster |
| write at 4 KiB in 64 MiB | 247 | 11.3 | 22× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 252 | 14.6 | 17× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 116 | 6.9 | 17× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 108 | 7.5 | 15× faster | 4.6× faster |
| delete 4 KiB, middle of 64 MiB | 218 | 17.8 | 12× faster | 11× faster |
| write at 4 KiB in 32 MiB | 113 | 12.5 | 9.1× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 114 | 12.8 | 8.9× faster | 3.5× faster |
| append 4 KiB to 32 MiB | 113 | 13.4 | 8.4× faster | 8.0× faster |
| insert 4 KiB, middle of 32 MiB | 111 | 14.0 | 7.9× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 116 | 15.5 | 7.5× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.8 | 0.46 | 6.0× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.9 | 0.96 | 5.1× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.24 | 4.4× faster | 34× faster |
| head | 0.79 | 0.20 | 4.0× faster | 13× faster |
| get 4 KiB | 0.83 | 0.22 | 3.8× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.85 | 3.2× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 240 | 107 | 2.2× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 118 | 88.2 | 1.3× faster | 1.4× faster |
| get 1 MiB | 1.1 | 0.86 | 1.3× faster | 4.5× faster |
| multipart put 256 MiB × 16 MiB | 1,488 | 1,184 | 1.3× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 309 | 265 | 1.2× faster | 2.4× slower |
| stream get 256 MiB | 188 | 206 | 1.1× slower | 16× faster |
| stream get 64 MiB | 47.3 | 52.2 | 1.1× slower | 15× faster |
| put 32 MiB | 109 | 122 | 1.1× slower | 1.2× slower |
| get 32 MiB | 27.2 | 31.0 | 1.1× slower | 12× faster |
| put 64 MiB | 212 | 246 | 1.2× slower | 1.1× slower |
| get 64 MiB | 47.6 | 55.6 | 1.2× slower | 17× faster |
| delete 4 KiB, middle of 1 MiB | 5.8 | 7.0 | 1.2× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 5.2 | 6.6 | 1.3× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 4.6 | 5.9 | 1.3× slower | 1.6× slower |
| insert 4 KiB, start of 1 MiB | 4.8 | 6.1 | 1.3× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 5.2 | 6.8 | 1.3× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 5.6 | 7.4 | 1.3× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 5.8 | 7.6 | 1.3× slower | 2.1× slower |
| overwrite 4 KiB | 1.4 | 2.4 | 1.6× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 5.0 | 8.5 | 1.7× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.7 | 6.4 | 1.7× slower | 2.2× slower |
| overwrite 1 MiB | 5.4 | 9.3 | 1.7× slower | 1.9× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.7 | 10.4 | 1.8× slower | 2.4× slower |
| put 1 MiB | 3.5 | 6.6 | 1.9× slower | 1.7× slower |
| put 4 KiB | 1.3 | 2.6 | 2.1× slower | 2.0× slower |
| patch 16 × 4 KiB in 1 MiB | 5.0 | 25.6 | 5.1× slower | 1.5× slower |
