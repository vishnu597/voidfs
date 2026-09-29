# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T143034Z` |
| When | 2026-09-29T14:30:34Z |
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
| move dir 200 × 64 KiB | 405 | 1.0 | 391× faster | 18× faster |
| rename 64 MiB | 111 | 0.73 | 151× faster | 7.9× faster |
| list 200 keys | 35.6 | 1.0 | 34× faster | 9.1× faster |
| append 4 KiB to 64 MiB | 189 | 6.7 | 28× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 210 | 8.4 | 25× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 268 | 10.9 | 24× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 219 | 10.6 | 21× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 212 | 12.0 | 18× faster | 11× faster |
| append 4 KiB to 32 MiB | 102 | 5.8 | 17× faster | 8.0× faster |
| insert 4 KiB, middle of 64 MiB | 184 | 11.2 | 16× faster | 11× faster |
| write at 4 KiB in 64 MiB | 182 | 11.7 | 16× faster | 6.2× faster |
| delete 4 KiB, start of 32 MiB | 106 | 7.0 | 15× faster | 4.6× faster |
| truncate 4 KiB, end of 32 MiB | 96.9 | 6.5 | 15× faster | 5.9× faster |
| insert 4 KiB, start of 32 MiB | 111 | 7.9 | 14× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 111 | 8.9 | 12× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 103 | 10.5 | 9.8× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 107 | 11.5 | 9.3× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.45 | 5.6× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.7 | 0.87 | 5.4× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.7× faster | 34× faster |
| get 4 KiB | 0.83 | 0.21 | 3.9× faster | 23× faster |
| head | 0.73 | 0.20 | 3.7× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.80 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 169 | 94.9 | 1.8× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 96.8 | 69.5 | 1.4× faster | 1.4× faster |
| get 1 MiB | 1.0 | 0.81 | 1.3× faster | 4.5× faster |
| multipart put 64 MiB × 8 MiB | 305 | 283 | 1.1× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,198 | 1,162 | 1.0× faster | 1.8× slower |
| get 64 MiB | 50.1 | 53.3 | 1.1× slower | 17× faster |
| stream get 64 MiB | 46.5 | 50.9 | 1.1× slower | 15× faster |
| get 32 MiB | 25.6 | 28.3 | 1.1× slower | 12× faster |
| stream get 256 MiB | 186 | 206 | 1.1× slower | 16× faster |
| truncate 4 KiB, end of 1 MiB | 4.1 | 4.6 | 1.1× slower | 2.1× slower |
| append 4 KiB to 1 MiB | 4.2 | 5.0 | 1.2× slower | 1.0× faster |
| write at 4 KiB in 1 MiB | 4.3 | 5.1 | 1.2× slower | 1.6× slower |
| insert 4 KiB, start of 1 MiB | 4.3 | 5.3 | 1.2× slower | 1.7× slower |
| delete 4 KiB, start of 1 MiB | 4.2 | 5.1 | 1.2× slower | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 4.7 | 5.9 | 1.3× slower | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 4.6 | 5.9 | 1.3× slower | 1.4× slower |
| overwrite 4 KiB | 1.1 | 1.8 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.1 | 5.1 | 1.7× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.2 | 9.0 | 1.7× slower | 2.4× slower |
| put 64 MiB | 164 | 317 | 1.9× slower | 1.1× slower |
| put 4 KiB | 0.81 | 1.6 | 1.9× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.2 | 8.6 | 2.0× slower | 2.8× slower |
| put 32 MiB | 81.0 | 166 | 2.1× slower | 1.2× slower |
| overwrite 1 MiB | 3.4 | 8.0 | 2.4× slower | 1.9× slower |
| put 1 MiB | 3.0 | 7.8 | 2.6× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.2 | 19.9 | 4.7× slower | 1.5× slower |
