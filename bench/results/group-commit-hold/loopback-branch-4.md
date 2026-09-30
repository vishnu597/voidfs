# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T152817Z` |
| When | 2026-09-30T15:28:17Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.6×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 409 | 1.1 | 359× faster | 18× faster |
| rename 64 MiB | 161 | 1.2 | 135× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 266 | 5.3 | 50× faster | 15× faster |
| list 200 keys | 43.2 | 1.2 | 37× faster | 9.1× faster |
| write at 4 KiB in 64 MiB | 234 | 6.5 | 36× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 208 | 6.6 | 32× faster | 13× faster |
| insert 4 KiB, middle of 64 MiB | 319 | 11.3 | 28× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 210 | 7.7 | 27× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 294 | 12.3 | 24× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 206 | 8.8 | 23× faster | 6.8× faster |
| insert 4 KiB, start of 32 MiB | 113 | 5.0 | 22× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 113 | 5.3 | 21× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 116 | 7.3 | 16× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 97.1 | 6.7 | 14× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 97.8 | 7.3 | 13× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 105 | 10.3 | 10× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 112 | 12.4 | 9.0× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.43 | 5.9× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.7 | 0.85 | 5.5× faster | 2.6× faster |
| head | 1.00 | 0.19 | 5.2× faster | 13× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.24 | 4.6× faster | 34× faster |
| get 4 KiB | 0.86 | 0.24 | 3.6× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.8 | 0.92 | 3.0× faster | 6.2× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.4 | 3.3 | 1.9× faster | 2.4× slower |
| patch 16 × 4 KiB in 64 MiB | 188 | 108 | 1.7× faster | 2.1× faster |
| multipart put 256 MiB × 16 MiB | 1,250 | 726 | 1.7× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 240 | 152 | 1.6× faster | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.3 | 2.1 | 1.6× faster | 2.2× slower |
| patch 16 × 4 KiB in 32 MiB | 106 | 77.8 | 1.4× faster | 1.4× faster |
| get 1 MiB | 1.2 | 1.0 | 1.2× faster | 4.5× faster |
| overwrite 4 KiB | 1.00 | 0.90 | 1.1× faster | 3.1× slower |
| stream get 256 MiB | 216 | 228 | 1.1× slower | 16× faster |
| get 32 MiB | 27.1 | 28.7 | 1.1× slower | 12× faster |
| stream get 64 MiB | 48.1 | 54.6 | 1.1× slower | 15× faster |
| put 4 KiB | 0.81 | 0.93 | 1.1× slower | 2.0× slower |
| delete 4 KiB, start of 1 MiB | 4.1 | 4.7 | 1.2× slower | 1.5× slower |
| put 32 MiB | 90.4 | 110 | 1.2× slower | 1.2× slower |
| put 64 MiB | 160 | 196 | 1.2× slower | 1.1× slower |
| get 64 MiB | 44.0 | 54.6 | 1.2× slower | 17× faster |
| truncate 4 KiB, end of 1 MiB | 4.1 | 5.1 | 1.2× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 4.0 | 5.0 | 1.2× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 4.9 | 6.1 | 1.3× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 3.9 | 5.0 | 1.3× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 4.7 | 6.2 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 4.0 | 5.3 | 1.3× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.7 | 6.3 | 1.7× slower | 2.8× slower |
| overwrite 1 MiB | 3.1 | 5.7 | 1.8× slower | 1.9× slower |
| put 1 MiB | 2.9 | 5.8 | 2.0× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.3 | 21.2 | 5.0× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.69 |  |  |  | 1.02 | 0.67 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.67 |  | 0.02 |  | 1.00 | 0.66 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.84 |  | 0.05 |  | 1.00 | 0.80 |
| insert 4 KiB, start of 64 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| write at 4 KiB in 64 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.72 |  |  |  | 1.02 | 0.70 |
| delete 4 KiB, start of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| insert 4 KiB, start of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 22.33 |  | 5.77 |  | 15.78 | 0.78 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.61 |  | 0.03 |  | 11.75 | 0.83 |
| append 4 KiB to 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| put 64 MiB | 64 | 29.33 |  |  |  | 28.33 | 1.00 |
| put 32 MiB | 64 | 14.88 |  |  |  | 13.92 | 0.95 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.73 |  |  |  | 1.04 | 0.69 |
| delete 4 KiB, start of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| write at 4 KiB in 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| insert 4 KiB, start of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| put 1 MiB | 400 | 1.90 |  |  |  | 1.21 | 0.69 |
| multipart put 256 MiB × 16 MiB | 32 | 174.81 | 1.00 | 33.28 | 1.00 | 138.59 | 0.94 |
| overwrite 1 MiB | 400 | 1.86 |  |  |  | 1.18 | 0.69 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.47 | 1.00 | 17.00 | 1.00 | 41.50 | 0.97 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
