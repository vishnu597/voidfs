# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T213724Z` |
| When | 2026-09-29T21:37:24Z |
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
| move dir 200 × 64 KiB | 387 | 0.88 | 439× faster | 18× faster |
| rename 64 MiB | 106 | 0.82 | 130× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 173 | 4.1 | 42× faster | 15× faster |
| list 200 keys | 38.1 | 1.1 | 35× faster | 9.1× faster |
| write at 4 KiB in 64 MiB | 227 | 8.4 | 27× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 206 | 7.7 | 27× faster | 13× faster |
| delete 4 KiB, middle of 64 MiB | 192 | 9.2 | 21× faster | 11× faster |
| append 4 KiB to 32 MiB | 107 | 5.5 | 20× faster | 8.0× faster |
| truncate 4 KiB, end of 32 MiB | 113 | 6.0 | 19× faster | 5.9× faster |
| insert 4 KiB, start of 64 MiB | 202 | 11.7 | 17× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 195 | 12.5 | 16× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 198 | 14.3 | 14× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 114 | 8.3 | 14× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 114 | 8.2 | 14× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 111 | 8.7 | 13× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 114 | 9.4 | 12× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 104 | 10.8 | 9.6× faster | 4.6× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.45 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.5 | 0.91 | 5.0× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.7× faster | 34× faster |
| head | 0.74 | 0.20 | 3.8× faster | 13× faster |
| get 4 KiB | 0.81 | 0.22 | 3.7× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.77 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 212 | 103 | 2.1× faster | 2.1× faster |
| get 1 MiB | 1.0 | 0.72 | 1.5× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 100 | 70.8 | 1.4× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 332 | 260 | 1.3× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,351 | 1,187 | 1.1× faster | 1.8× slower |
| stream get 256 MiB | 193 | 196 | 1.0× slower | 16× faster |
| write at 4 KiB in 1 MiB | 4.2 | 4.7 | 1.1× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 4.6 | 5.2 | 1.1× slower | 1.4× slower |
| get 64 MiB | 44.2 | 49.9 | 1.1× slower | 17× faster |
| stream get 64 MiB | 42.9 | 50.4 | 1.2× slower | 15× faster |
| delete 4 KiB, start of 1 MiB | 4.6 | 5.5 | 1.2× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 4.4 | 5.3 | 1.2× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 4.9 | 6.0 | 1.2× slower | 1.4× slower |
| get 32 MiB | 21.7 | 27.0 | 1.2× slower | 12× faster |
| append 4 KiB to 1 MiB | 4.3 | 5.4 | 1.3× slower | 1.0× faster |
| truncate 4 KiB, end of 1 MiB | 4.7 | 5.9 | 1.3× slower | 2.1× slower |
| overwrite 4 KiB | 1.2 | 1.9 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.1 | 5.0 | 1.6× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.2 | 9.2 | 1.8× slower | 2.4× slower |
| put 32 MiB | 91.7 | 174 | 1.9× slower | 1.2× slower |
| put 64 MiB | 180 | 348 | 1.9× slower | 1.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.3 | 8.2 | 1.9× slower | 2.8× slower |
| put 4 KiB | 0.97 | 1.9 | 2.0× slower | 2.0× slower |
| overwrite 1 MiB | 4.2 | 10.0 | 2.4× slower | 1.9× slower |
| put 1 MiB | 3.1 | 8.2 | 2.6× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.4 | 20.2 | 4.6× slower | 1.5× slower |
