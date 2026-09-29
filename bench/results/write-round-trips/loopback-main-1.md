# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T213320Z` |
| When | 2026-09-29T21:33:20Z |
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
| move dir 200 × 64 KiB | 382 | 0.92 | 414× faster | 18× faster |
| rename 64 MiB | 111 | 0.75 | 149× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 254 | 5.0 | 51× faster | 13× faster |
| append 4 KiB to 64 MiB | 175 | 5.1 | 34× faster | 15× faster |
| list 200 keys | 35.2 | 1.0 | 34× faster | 9.1× faster |
| insert 4 KiB, middle of 64 MiB | 244 | 8.6 | 28× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 225 | 10.3 | 22× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 178 | 10.1 | 18× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 187 | 11.3 | 17× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 180 | 11.2 | 16× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 91.2 | 6.0 | 15× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 108 | 7.9 | 14× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 105 | 8.5 | 12× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 103 | 9.0 | 11× faster | 3.5× faster |
| delete 4 KiB, start of 32 MiB | 91.5 | 8.7 | 10× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 102 | 10.4 | 9.9× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 105 | 11.3 | 9.3× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.45 | 5.5× faster | 5.3× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.22 | 4.9× faster | 34× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.2 | 0.93 | 4.5× faster | 2.6× faster |
| head | 0.77 | 0.21 | 3.7× faster | 13× faster |
| get 4 KiB | 0.81 | 0.22 | 3.6× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.79 | 3.1× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 171 | 91.6 | 1.9× faster | 2.1× faster |
| get 1 MiB | 1.0 | 0.75 | 1.4× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 98.9 | 73.7 | 1.3× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,109 | 904 | 1.2× faster | 1.8× slower |
| truncate 4 KiB, end of 1 MiB | 4.3 | 3.8 | 1.1× faster | 2.1× slower |
| multipart put 64 MiB × 8 MiB | 253 | 257 | 1.0× slower | 2.4× slower |
| get 32 MiB | 23.1 | 25.1 | 1.1× slower | 12× faster |
| stream get 64 MiB | 45.1 | 49.6 | 1.1× slower | 15× faster |
| stream get 256 MiB | 179 | 200 | 1.1× slower | 16× faster |
| append 4 KiB to 1 MiB | 4.0 | 4.4 | 1.1× slower | 1.0× faster |
| delete 4 KiB, start of 1 MiB | 4.3 | 4.8 | 1.1× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 4.3 | 4.9 | 1.1× slower | 1.7× slower |
| get 64 MiB | 45.1 | 53.4 | 1.2× slower | 17× faster |
| insert 4 KiB, middle of 1 MiB | 4.2 | 5.0 | 1.2× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.0 | 4.9 | 1.2× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 4.2 | 5.1 | 1.2× slower | 1.4× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.4 | 8.6 | 1.3× slower | 2.4× slower |
| overwrite 4 KiB | 0.97 | 1.5 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 2.8 | 5.1 | 1.8× slower | 2.2× slower |
| put 4 KiB | 0.90 | 1.7 | 1.9× slower | 2.0× slower |
| put 64 MiB | 149 | 296 | 2.0× slower | 1.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.4 | 6.8 | 2.0× slower | 2.8× slower |
| put 32 MiB | 72.7 | 151 | 2.1× slower | 1.2× slower |
| overwrite 1 MiB | 3.1 | 7.6 | 2.4× slower | 1.9× slower |
| put 1 MiB | 2.9 | 7.5 | 2.6× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.1 | 17.3 | 4.3× slower | 1.5× slower |
