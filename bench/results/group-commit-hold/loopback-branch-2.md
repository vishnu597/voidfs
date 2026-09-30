# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T151931Z` |
| When | 2026-09-30T15:19:31Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | bc851c2 |
| distance to the bucket | none (loopback) |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.4×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 469 | 1.3 | 367× faster | 18× faster |
| rename 64 MiB | 115 | 0.95 | 120× faster | 7.9× faster |
| delete 4 KiB, start of 64 MiB | 213 | 5.0 | 42× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 202 | 5.3 | 38× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 241 | 7.5 | 32× faster | 6.8× faster |
| list 200 keys | 40.2 | 1.3 | 32× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 225 | 7.5 | 30× faster | 13× faster |
| write at 4 KiB in 64 MiB | 211 | 8.8 | 24× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 228 | 10.4 | 22× faster | 11× faster |
| append 4 KiB to 32 MiB | 105 | 4.9 | 22× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 107 | 6.9 | 15× faster | 3.5× faster |
| insert 4 KiB, middle of 64 MiB | 217 | 14.9 | 15× faster | 11× faster |
| write at 4 KiB in 32 MiB | 109 | 8.2 | 13× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 104 | 8.5 | 12× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 96.7 | 11.1 | 8.7× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 97.0 | 11.4 | 8.5× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 101 | 12.2 | 8.3× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.8 | 0.46 | 6.1× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.9 | 0.93 | 5.3× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.2 | 0.26 | 4.5× faster | 34× faster |
| head | 0.89 | 0.21 | 4.3× faster | 13× faster |
| get 4 KiB | 0.89 | 0.24 | 3.7× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 3.0 | 0.86 | 3.4× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 220 | 114 | 1.9× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.6 | 3.7 | 1.8× faster | 2.4× slower |
| get 1 MiB | 1.2 | 0.78 | 1.5× faster | 4.5× faster |
| multipart put 256 MiB × 16 MiB | 1,424 | 986 | 1.4× faster | 1.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.6 | 2.6 | 1.4× faster | 2.2× slower |
| multipart put 64 MiB × 8 MiB | 331 | 243 | 1.4× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 108 | 84.0 | 1.3× faster | 1.4× faster |
| overwrite 4 KiB | 1.2 | 1.2 | 1.0× faster | 3.1× slower |
| stream get 256 MiB | 264 | 275 | 1.0× slower | 16× faster |
| get 32 MiB | 27.6 | 30.6 | 1.1× slower | 12× faster |
| put 64 MiB | 195 | 217 | 1.1× slower | 1.1× slower |
| delete 4 KiB, start of 1 MiB | 4.5 | 5.1 | 1.1× slower | 1.5× slower |
| put 32 MiB | 95.4 | 110 | 1.1× slower | 1.2× slower |
| delete 4 KiB, middle of 1 MiB | 5.8 | 6.7 | 1.2× slower | 1.4× slower |
| stream get 64 MiB | 61.4 | 74.1 | 1.2× slower | 15× faster |
| get 64 MiB | 59.2 | 71.5 | 1.2× slower | 17× faster |
| insert 4 KiB, start of 1 MiB | 4.7 | 5.7 | 1.2× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 4.8 | 6.0 | 1.2× slower | 1.0× faster |
| write at 4 KiB in 1 MiB | 4.3 | 5.6 | 1.3× slower | 1.6× slower |
| put 4 KiB | 1.1 | 1.4 | 1.3× slower | 2.0× slower |
| truncate 4 KiB, end of 1 MiB | 4.9 | 6.6 | 1.3× slower | 2.1× slower |
| insert 4 KiB, middle of 1 MiB | 5.1 | 7.0 | 1.4× slower | 1.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.3 | 7.7 | 1.8× slower | 2.8× slower |
| overwrite 1 MiB | 4.2 | 7.8 | 1.8× slower | 1.9× slower |
| put 1 MiB | 3.2 | 6.0 | 1.9× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.8 | 20.8 | 4.4× slower | 1.5× slower |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | delete_prefix | get | list | put | put_new |
|---|--:|--:|--:|--:|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.00 |  |  |  |  |  |
| get 4 KiB | 400 | 0.00 |  |  |  |  |  |
| move dir 200 × 64 KiB | 32 | 0.25 |  |  |  |  | 0.25 |
| get 64 MiB | 64 | 0.00 |  |  |  |  |  |
| stream get 256 MiB | 32 | 0.00 |  |  |  |  |  |
| stream get 64 MiB | 64 | 0.00 |  |  |  |  |  |
| append 4 KiB to 64 MiB | 64 | 1.61 |  |  |  | 1.00 | 0.61 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.75 |  | 0.02 |  | 1.00 | 0.73 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.84 |  |  |  | 1.00 | 0.84 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| insert 4 KiB, start of 64 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| write at 4 KiB in 64 MiB | 64 | 1.69 |  | 0.05 |  | 1.00 | 0.64 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| delete 4 KiB, start of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| insert 4 KiB, start of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.17 |  | 5.05 |  | 15.27 | 0.86 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.95 |  | 0.05 |  | 12.00 | 0.91 |
| append 4 KiB to 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| put 64 MiB | 64 | 29.09 |  | 0.02 |  | 28.11 | 0.97 |
| put 32 MiB | 64 | 15.69 |  |  |  | 14.73 | 0.95 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.64 |  |  |  | 1.00 | 0.64 |
| patch 16 × 4 KiB in 1 MiB | 128 | 2.07 |  |  |  | 1.34 | 0.73 |
| delete 4 KiB, start of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| write at 4 KiB in 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| insert 4 KiB, start of 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| put 1 MiB | 400 | 1.84 |  |  |  | 1.17 | 0.67 |
| multipart put 256 MiB × 16 MiB | 32 | 174.22 | 1.00 | 33.00 | 1.00 | 138.41 | 0.81 |
| overwrite 1 MiB | 400 | 1.95 |  |  |  | 1.18 | 0.77 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.22 | 1.00 | 17.00 | 1.00 | 41.62 | 0.59 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.14 |  |  |  | 1.00 | 0.14 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
