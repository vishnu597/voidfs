# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T142038Z` |
| When | 2026-09-29T14:20:38Z |
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
| move dir 200 × 64 KiB | 415 | 1.1 | 380× faster | 18× faster |
| rename 64 MiB | 110 | 0.92 | 120× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 229 | 6.6 | 35× faster | 13× faster |
| append 4 KiB to 64 MiB | 246 | 7.1 | 35× faster | 15× faster |
| list 200 keys | 38.4 | 1.3 | 30× faster | 9.1× faster |
| delete 4 KiB, start of 64 MiB | 218 | 8.0 | 27× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 239 | 11.9 | 20× faster | 11× faster |
| write at 4 KiB in 64 MiB | 213 | 11.1 | 19× faster | 6.2× faster |
| truncate 4 KiB, end of 32 MiB | 109 | 5.7 | 19× faster | 5.9× faster |
| insert 4 KiB, start of 64 MiB | 249 | 13.6 | 18× faster | 6.8× faster |
| write at 4 KiB in 32 MiB | 113 | 7.7 | 15× faster | 5.1× faster |
| insert 4 KiB, middle of 64 MiB | 241 | 17.6 | 14× faster | 11× faster |
| delete 4 KiB, start of 32 MiB | 107 | 8.0 | 13× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 104 | 8.0 | 13× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 110 | 9.4 | 12× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 112 | 11.8 | 9.5× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 109 | 14.6 | 7.4× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.7 | 0.45 | 5.9× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 5.2 | 0.95 | 5.4× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.26 | 4.4× faster | 34× faster |
| head | 0.94 | 0.24 | 3.9× faster | 13× faster |
| get 4 KiB | 0.85 | 0.23 | 3.6× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.77 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 243 | 109 | 2.2× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 110 | 73.0 | 1.5× faster | 1.4× faster |
| get 1 MiB | 1.1 | 0.85 | 1.3× faster | 4.5× faster |
| multipart put 256 MiB × 16 MiB | 1,327 | 1,182 | 1.1× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 317 | 293 | 1.1× faster | 2.4× slower |
| stream get 256 MiB | 231 | 248 | 1.1× slower | 16× faster |
| stream get 64 MiB | 53.4 | 59.3 | 1.1× slower | 15× faster |
| get 64 MiB | 53.6 | 60.3 | 1.1× slower | 17× faster |
| delete 4 KiB, start of 1 MiB | 4.7 | 5.4 | 1.2× slower | 1.5× slower |
| append 4 KiB to 1 MiB | 4.3 | 5.1 | 1.2× slower | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 4.9 | 6.0 | 1.2× slower | 1.4× slower |
| get 32 MiB | 26.7 | 33.0 | 1.2× slower | 12× faster |
| truncate 4 KiB, end of 1 MiB | 4.3 | 5.3 | 1.2× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 4.2 | 5.3 | 1.3× slower | 1.7× slower |
| write at 4 KiB in 1 MiB | 4.2 | 5.6 | 1.3× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 5.0 | 6.7 | 1.4× slower | 1.4× slower |
| overwrite 4 KiB | 1.3 | 1.9 | 1.5× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.1 | 5.0 | 1.6× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.3 | 9.2 | 1.7× slower | 2.4× slower |
| put 64 MiB | 189 | 343 | 1.8× slower | 1.1× slower |
| put 4 KiB | 1.0 | 2.0 | 1.9× slower | 2.0× slower |
| put 32 MiB | 92.9 | 181 | 1.9× slower | 1.2× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.4 | 8.5 | 2.0× slower | 2.8× slower |
| overwrite 1 MiB | 4.2 | 10.1 | 2.4× slower | 1.9× slower |
| put 1 MiB | 3.3 | 8.0 | 2.4× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.5 | 24.1 | 5.4× slower | 1.5× slower |
