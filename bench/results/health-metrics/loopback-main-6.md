# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T144508Z` |
| When | 2026-09-29T14:45:08Z |
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

voidfs is faster in **28 of 49** scenarios and slower in the other **21**. Geometric mean speed-up over the bare bucket: **3.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 362 | 0.98 | 372× faster | 18× faster |
| rename 64 MiB | 112 | 0.84 | 133× faster | 7.9× faster |
| write at 4 KiB in 64 MiB | 242 | 7.1 | 34× faster | 6.2× faster |
| list 200 keys | 39.5 | 1.2 | 32× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 203 | 7.2 | 28× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 265 | 10.6 | 25× faster | 6.8× faster |
| truncate 4 KiB, end of 32 MiB | 98.1 | 4.1 | 24× faster | 5.9× faster |
| delete 4 KiB, start of 64 MiB | 218 | 9.6 | 23× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 178 | 7.9 | 22× faster | 15× faster |
| delete 4 KiB, middle of 64 MiB | 231 | 10.7 | 22× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 241 | 11.5 | 21× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 101 | 6.7 | 15× faster | 3.5× faster |
| delete 4 KiB, start of 32 MiB | 121 | 9.1 | 13× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 113 | 9.9 | 11× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 110 | 10.8 | 10× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 119 | 14.1 | 8.4× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 102 | 12.9 | 7.8× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.8 | 0.49 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 5.0 | 0.95 | 5.3× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.8× faster | 34× faster |
| head | 0.76 | 0.19 | 3.9× faster | 13× faster |
| get 4 KiB | 0.81 | 0.22 | 3.8× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.82 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 244 | 117 | 2.1× faster | 2.1× faster |
| get 1 MiB | 1.1 | 0.85 | 1.3× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 108 | 93.7 | 1.1× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 329 | 294 | 1.1× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,343 | 1,326 | 1.0× faster | 1.8× slower |
| get 64 MiB | 46.1 | 50.3 | 1.1× slower | 17× faster |
| stream get 256 MiB | 179 | 199 | 1.1× slower | 16× faster |
| stream get 64 MiB | 44.7 | 50.5 | 1.1× slower | 15× faster |
| delete 4 KiB, start of 1 MiB | 4.7 | 5.6 | 1.2× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 4.6 | 5.4 | 1.2× slower | 1.6× slower |
| append 4 KiB to 1 MiB | 4.7 | 5.8 | 1.2× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 5.3 | 6.7 | 1.3× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 5.0 | 6.4 | 1.3× slower | 2.1× slower |
| delete 4 KiB, middle of 1 MiB | 5.3 | 6.9 | 1.3× slower | 1.4× slower |
| get 32 MiB | 24.1 | 31.6 | 1.3× slower | 12× faster |
| insert 4 KiB, start of 1 MiB | 4.7 | 6.2 | 1.3× slower | 1.7× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.8 | 6.0 | 1.6× slower | 2.2× slower |
| overwrite 4 KiB | 1.5 | 2.4 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.3 | 9.2 | 1.7× slower | 2.4× slower |
| put 64 MiB | 198 | 372 | 1.9× slower | 1.1× slower |
| put 4 KiB | 1.1 | 2.1 | 1.9× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 5.2 | 10.1 | 1.9× slower | 2.8× slower |
| put 32 MiB | 95.2 | 190 | 2.0× slower | 1.2× slower |
| overwrite 1 MiB | 4.4 | 10.9 | 2.5× slower | 1.9× slower |
| put 1 MiB | 3.3 | 8.8 | 2.7× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.4 | 25.8 | 5.8× slower | 1.5× slower |
