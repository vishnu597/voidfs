# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T142910Z` |
| When | 2026-09-29T14:29:10Z |
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
| move dir 200 × 64 KiB | 395 | 0.92 | 428× faster | 18× faster |
| rename 64 MiB | 103 | 0.79 | 130× faster | 7.9× faster |
| list 200 keys | 35.5 | 1.1 | 32× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 177 | 5.7 | 31× faster | 13× faster |
| append 4 KiB to 64 MiB | 186 | 7.0 | 27× faster | 15× faster |
| write at 4 KiB in 64 MiB | 220 | 9.8 | 23× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 185 | 8.8 | 21× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 189 | 9.3 | 20× faster | 11× faster |
| append 4 KiB to 32 MiB | 106 | 5.4 | 20× faster | 8.0× faster |
| delete 4 KiB, start of 64 MiB | 196 | 10.2 | 19× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 203 | 12.7 | 16× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 103 | 7.2 | 14× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 97.1 | 7.4 | 13× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 103 | 8.2 | 13× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 104 | 9.2 | 11× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 112 | 12.8 | 8.8× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 118 | 13.9 | 8.5× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.45 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.7 | 0.88 | 5.3× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.7× faster | 34× faster |
| head | 0.75 | 0.19 | 3.9× faster | 13× faster |
| get 4 KiB | 0.82 | 0.21 | 3.9× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.77 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 179 | 106 | 1.7× faster | 2.1× faster |
| get 1 MiB | 1.00 | 0.71 | 1.4× faster | 4.5× faster |
| multipart put 256 MiB × 16 MiB | 1,173 | 974 | 1.2× faster | 1.8× slower |
| patch 16 × 4 KiB in 32 MiB | 90.2 | 78.2 | 1.2× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 263 | 260 | 1.0× faster | 2.4× slower |
| delete 4 KiB, start of 1 MiB | 4.5 | 4.9 | 1.1× slower | 1.5× slower |
| get 64 MiB | 48.9 | 54.4 | 1.1× slower | 17× faster |
| insert 4 KiB, start of 1 MiB | 4.4 | 4.9 | 1.1× slower | 1.7× slower |
| stream get 256 MiB | 179 | 205 | 1.1× slower | 16× faster |
| delete 4 KiB, middle of 1 MiB | 4.2 | 5.0 | 1.2× slower | 1.4× slower |
| get 32 MiB | 22.8 | 27.1 | 1.2× slower | 12× faster |
| stream get 64 MiB | 43.3 | 51.4 | 1.2× slower | 15× faster |
| insert 4 KiB, middle of 1 MiB | 4.2 | 5.0 | 1.2× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 4.0 | 4.8 | 1.2× slower | 2.1× slower |
| append 4 KiB to 1 MiB | 4.1 | 5.0 | 1.2× slower | 1.0× faster |
| write at 4 KiB in 1 MiB | 4.0 | 5.0 | 1.3× slower | 1.6× slower |
| overwrite 4 KiB | 1.00 | 1.5 | 1.5× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.8 | 9.0 | 1.5× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 2.8 | 5.0 | 1.8× slower | 2.2× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.7 | 6.7 | 1.8× slower | 2.8× slower |
| put 4 KiB | 0.81 | 1.6 | 1.9× slower | 2.0× slower |
| put 64 MiB | 151 | 297 | 2.0× slower | 1.1× slower |
| put 32 MiB | 75.6 | 154 | 2.0× slower | 1.2× slower |
| overwrite 1 MiB | 3.2 | 7.6 | 2.3× slower | 1.9× slower |
| put 1 MiB | 3.0 | 7.7 | 2.6× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.3 | 21.3 | 4.9× slower | 1.5× slower |
