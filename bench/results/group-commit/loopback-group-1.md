# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T193214Z` |
| When | 2026-09-28T19:32:14Z |
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

voidfs is faster in **26 of 49** scenarios and slower in the other **23**. Geometric mean speed-up over the bare bucket: **2.5×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 377 | 0.97 | 391× faster | 18× faster |
| rename 64 MiB | 123 | 0.84 | 146× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 222 | 4.0 | 55× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 209 | 5.7 | 37× faster | 13× faster |
| list 200 keys | 34.5 | 1.2 | 28× faster | 9.1× faster |
| insert 4 KiB, start of 64 MiB | 249 | 13.2 | 19× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 223 | 13.3 | 17× faster | 6.9× faster |
| write at 4 KiB in 64 MiB | 226 | 13.7 | 16× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 216 | 15.1 | 14× faster | 11× faster |
| append 4 KiB to 32 MiB | 98.4 | 7.3 | 13× faster | 8.0× faster |
| insert 4 KiB, middle of 64 MiB | 182 | 15.2 | 12× faster | 11× faster |
| write at 4 KiB in 32 MiB | 112 | 12.1 | 9.3× faster | 5.1× faster |
| truncate 4 KiB, end of 32 MiB | 97.3 | 10.8 | 9.0× faster | 5.9× faster |
| insert 4 KiB, middle of 32 MiB | 114 | 13.4 | 8.5× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 101 | 14.2 | 7.1× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 108 | 15.8 | 6.9× faster | 3.5× faster |
| delete 4 KiB, middle of 32 MiB | 105 | 21.1 | 5.0× faster | 4.8× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.24 | 4.6× faster | 34× faster |
| head | 0.75 | 0.18 | 4.1× faster | 13× faster |
| get 4 KiB | 0.81 | 0.23 | 3.6× faster | 23× faster |
| get 1 MiB | 1.4 | 0.97 | 1.5× faster | 4.5× faster |
| patch 16 × 4 KiB in 64 MiB | 274 | 198 | 1.4× faster | 2.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.7 | 2.4 | 1.1× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 5.7 | 5.2 | 1.1× faster | 2.6× faster |
| multipart put 64 MiB × 8 MiB | 392 | 370 | 1.1× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,593 | 1,543 | 1.0× faster | 1.8× slower |
| delete 4 KiB, start of 1 MiB | 5.1 | 5.3 | 1.0× slower | 1.5× slower |
| patch 16 × 4 KiB in 32 MiB | 109 | 115 | 1.1× slower | 1.4× faster |
| stream get 256 MiB | 201 | 218 | 1.1× slower | 16× faster |
| get 64 MiB | 51.3 | 56.7 | 1.1× slower | 17× faster |
| stream get 64 MiB | 50.0 | 55.7 | 1.1× slower | 15× faster |
| truncate 4 KiB, end of 1 MiB | 5.3 | 6.0 | 1.1× slower | 2.1× slower |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 3.0 | 1.2× slower | 6.2× faster |
| insert 4 KiB, start of 1 MiB | 5.3 | 6.3 | 1.2× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 5.6 | 6.8 | 1.2× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 5.5 | 6.9 | 1.2× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 5.7 | 7.1 | 1.3× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.8 | 7.0 | 1.5× slower | 1.6× slower |
| overwrite 4 KiB | 1.6 | 2.5 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.8 | 10.8 | 1.6× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.8 | 6.5 | 1.7× slower | 2.2× slower |
| get 32 MiB | 24.4 | 42.0 | 1.7× slower | 12× faster |
| put 4 KiB | 1.3 | 2.3 | 1.8× slower | 2.0× slower |
| put 64 MiB | 238 | 454 | 1.9× slower | 1.1× slower |
| put 32 MiB | 103 | 197 | 1.9× slower | 1.2× slower |
| fanout put 200 × 256 KiB, 32 at once | 5.0 | 10.6 | 2.1× slower | 2.8× slower |
| overwrite 1 MiB | 4.6 | 11.4 | 2.5× slower | 1.9× slower |
| put 1 MiB | 3.6 | 9.4 | 2.6× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 5.1 | 31.3 | 6.1× slower | 1.5× slower |
