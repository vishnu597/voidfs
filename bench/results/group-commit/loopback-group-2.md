# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T193359Z` |
| When | 2026-09-28T19:33:59Z |
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

voidfs is faster in **28 of 49** scenarios and slower in the other **21**. Geometric mean speed-up over the bare bucket: **2.5×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 396 | 1.2 | 324× faster | 18× faster |
| rename 64 MiB | 110 | 1.1 | 103× faster | 7.9× faster |
| list 200 keys | 43.8 | 1.3 | 33× faster | 9.1× faster |
| append 4 KiB to 64 MiB | 252 | 7.9 | 32× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 212 | 7.9 | 27× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 254 | 9.8 | 26× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 218 | 11.7 | 19× faster | 6.2× faster |
| append 4 KiB to 32 MiB | 103 | 7.1 | 15× faster | 8.0× faster |
| delete 4 KiB, start of 64 MiB | 224 | 16.0 | 14× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 225 | 16.3 | 14× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 222 | 21.6 | 10× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 105 | 10.6 | 9.9× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 109 | 11.1 | 9.8× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 112 | 14.0 | 8.0× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 98.9 | 12.8 | 7.7× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 116 | 16.3 | 7.1× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 107 | 18.8 | 5.7× faster | 4.8× faster |
| range 64 KiB of 64 MiB | 1.0 | 0.22 | 4.5× faster | 34× faster |
| get 4 KiB | 0.81 | 0.21 | 3.9× faster | 23× faster |
| head | 0.93 | 0.24 | 3.9× faster | 13× faster |
| patch 16 × 4 KiB in 64 MiB | 276 | 113 | 2.4× faster | 2.1× faster |
| multipart put 64 MiB × 8 MiB | 376 | 328 | 1.1× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 101 | 88.7 | 1.1× faster | 1.4× faster |
| get 1 MiB | 1.1 | 0.98 | 1.1× faster | 4.5× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.8 | 2.6 | 1.1× faster | 5.3× faster |
| append 4 KiB to 1 MiB | 4.7 | 4.6 | 1.0× faster | 1.0× faster |
| multipart put 256 MiB × 16 MiB | 1,360 | 1,344 | 1.0× faster | 1.8× slower |
| fanout get 1000 × 4 KiB, 64 at once | 5.0 | 4.9 | 1.0× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.9 | 3.1 | 1.1× slower | 6.2× faster |
| truncate 4 KiB, end of 1 MiB | 6.0 | 6.6 | 1.1× slower | 2.1× slower |
| stream get 64 MiB | 56.4 | 64.1 | 1.1× slower | 15× faster |
| stream get 256 MiB | 217 | 252 | 1.2× slower | 16× faster |
| delete 4 KiB, middle of 1 MiB | 5.3 | 6.4 | 1.2× slower | 1.4× slower |
| get 64 MiB | 49.1 | 59.1 | 1.2× slower | 17× faster |
| insert 4 KiB, start of 1 MiB | 4.9 | 5.9 | 1.2× slower | 1.7× slower |
| delete 4 KiB, start of 1 MiB | 5.1 | 6.6 | 1.3× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 4.6 | 6.0 | 1.3× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 5.6 | 7.5 | 1.3× slower | 1.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 4.1 | 5.7 | 1.4× slower | 2.2× slower |
| overwrite 4 KiB | 1.7 | 2.6 | 1.5× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.7 | 10.3 | 1.5× slower | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 5.9 | 9.7 | 1.6× slower | 2.8× slower |
| get 32 MiB | 25.7 | 45.6 | 1.8× slower | 12× faster |
| put 32 MiB | 91.5 | 172 | 1.9× slower | 1.2× slower |
| put 4 KiB | 1.1 | 2.3 | 2.1× slower | 2.0× slower |
| put 64 MiB | 145 | 302 | 2.1× slower | 1.1× slower |
| put 1 MiB | 3.5 | 8.5 | 2.4× slower | 1.7× slower |
| overwrite 1 MiB | 4.4 | 10.8 | 2.5× slower | 1.9× slower |
| patch 16 × 4 KiB in 1 MiB | 5.1 | 23.7 | 4.6× slower | 1.5× slower |
