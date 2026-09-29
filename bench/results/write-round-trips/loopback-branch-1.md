# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T213444Z` |
| When | 2026-09-29T21:34:44Z |
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

voidfs is faster in **28 of 49** scenarios and slower in the other **21**. Geometric mean speed-up over the bare bucket: **3.2×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 386 | 0.93 | 415× faster | 18× faster |
| rename 64 MiB | 115 | 0.71 | 161× faster | 7.9× faster |
| list 200 keys | 35.4 | 1.0 | 35× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 194 | 6.0 | 33× faster | 13× faster |
| append 4 KiB to 64 MiB | 213 | 7.5 | 28× faster | 15× faster |
| append 4 KiB to 32 MiB | 100.0 | 4.1 | 24× faster | 8.0× faster |
| write at 4 KiB in 64 MiB | 176 | 7.3 | 24× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 210 | 9.1 | 23× faster | 6.9× faster |
| truncate 4 KiB, end of 32 MiB | 114 | 5.5 | 21× faster | 5.9× faster |
| insert 4 KiB, middle of 64 MiB | 230 | 14.0 | 16× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 177 | 10.9 | 16× faster | 6.8× faster |
| write at 4 KiB in 32 MiB | 104 | 6.6 | 16× faster | 5.1× faster |
| delete 4 KiB, middle of 64 MiB | 177 | 12.6 | 14× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 100.0 | 8.8 | 11× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 110 | 10.2 | 11× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 105 | 12.5 | 8.4× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 107 | 14.3 | 7.5× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.43 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.4 | 0.87 | 5.1× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.22 | 4.9× faster | 34× faster |
| get 4 KiB | 0.82 | 0.21 | 3.9× faster | 23× faster |
| head | 0.73 | 0.19 | 3.9× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.74 | 3.4× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 178 | 100 | 1.8× faster | 2.1× faster |
| get 1 MiB | 1.1 | 0.71 | 1.6× faster | 4.5× faster |
| multipart put 256 MiB × 16 MiB | 1,045 | 735 | 1.4× faster | 1.8× slower |
| patch 16 × 4 KiB in 32 MiB | 94.4 | 72.8 | 1.3× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 256 | 218 | 1.2× faster | 2.4× slower |
| stream get 256 MiB | 184 | 196 | 1.1× slower | 16× faster |
| get 32 MiB | 21.9 | 23.6 | 1.1× slower | 12× faster |
| put 64 MiB | 154 | 170 | 1.1× slower | 1.1× slower |
| delete 4 KiB, middle of 1 MiB | 4.2 | 4.7 | 1.1× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 4.2 | 4.7 | 1.1× slower | 1.5× slower |
| get 64 MiB | 43.1 | 50.3 | 1.2× slower | 17× faster |
| insert 4 KiB, start of 1 MiB | 4.2 | 4.9 | 1.2× slower | 1.7× slower |
| put 32 MiB | 73.2 | 86.7 | 1.2× slower | 1.2× slower |
| stream get 64 MiB | 42.4 | 50.9 | 1.2× slower | 15× faster |
| truncate 4 KiB, end of 1 MiB | 4.1 | 5.0 | 1.2× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 3.8 | 4.6 | 1.2× slower | 1.6× slower |
| append 4 KiB to 1 MiB | 4.1 | 5.3 | 1.3× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 4.2 | 5.5 | 1.3× slower | 1.4× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.8 | 8.6 | 1.5× slower | 2.4× slower |
| overwrite 4 KiB | 1.0 | 1.5 | 1.5× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.1 | 4.9 | 1.6× slower | 2.2× slower |
| put 4 KiB | 0.85 | 1.5 | 1.8× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.5 | 6.3 | 1.8× slower | 2.8× slower |
| overwrite 1 MiB | 3.1 | 5.7 | 1.9× slower | 1.9× slower |
| put 1 MiB | 2.9 | 5.6 | 1.9× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.0 | 16.2 | 4.1× slower | 1.5× slower |
