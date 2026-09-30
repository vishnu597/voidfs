# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T151808Z` |
| When | 2026-09-30T15:18:08Z |
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

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **3.4×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 396 | 1.2 | 335× faster | 18× faster |
| rename 64 MiB | 108 | 0.93 | 116× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 225 | 4.7 | 48× faster | 13× faster |
| append 4 KiB to 64 MiB | 187 | 5.1 | 37× faster | 15× faster |
| list 200 keys | 35.8 | 1.2 | 29× faster | 9.1× faster |
| write at 4 KiB in 64 MiB | 177 | 6.0 | 29× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 200 | 8.6 | 23× faster | 6.8× faster |
| insert 4 KiB, middle of 64 MiB | 181 | 10.1 | 18× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 210 | 12.2 | 17× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 227 | 14.1 | 16× faster | 11× faster |
| write at 4 KiB in 32 MiB | 100 | 6.3 | 16× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 109 | 7.2 | 15× faster | 8.0× faster |
| truncate 4 KiB, end of 32 MiB | 94.9 | 6.5 | 15× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 114 | 8.4 | 14× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 106 | 9.3 | 11× faster | 4.8× faster |
| insert 4 KiB, start of 32 MiB | 112 | 11.5 | 9.8× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 93.8 | 10.1 | 9.3× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.7 | 0.44 | 6.1× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.7 | 0.91 | 5.1× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.24 | 4.7× faster | 34× faster |
| head | 0.81 | 0.20 | 4.1× faster | 13× faster |
| get 4 KiB | 0.88 | 0.24 | 3.7× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.81 | 3.4× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 210 | 117 | 1.8× faster | 2.1× faster |
| multipart put 64 MiB × 8 MiB | 443 | 259 | 1.7× faster | 2.4× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.4 | 3.8 | 1.7× faster | 2.4× slower |
| get 1 MiB | 1.1 | 0.71 | 1.6× faster | 4.5× faster |
| multipart put 256 MiB × 16 MiB | 1,417 | 911 | 1.6× faster | 1.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.5 | 2.8 | 1.2× faster | 2.2× slower |
| patch 16 × 4 KiB in 32 MiB | 103 | 86.0 | 1.2× faster | 1.4× faster |
| overwrite 4 KiB | 1.3 | 1.4 | 1.0× slower | 3.1× slower |
| stream get 64 MiB | 49.6 | 54.3 | 1.1× slower | 15× faster |
| stream get 256 MiB | 193 | 219 | 1.1× slower | 16× faster |
| put 32 MiB | 98.1 | 114 | 1.2× slower | 1.2× slower |
| put 64 MiB | 191 | 224 | 1.2× slower | 1.1× slower |
| get 32 MiB | 25.6 | 30.2 | 1.2× slower | 12× faster |
| insert 4 KiB, start of 1 MiB | 4.6 | 5.6 | 1.2× slower | 1.7× slower |
| put 4 KiB | 1.1 | 1.4 | 1.2× slower | 2.0× slower |
| delete 4 KiB, start of 1 MiB | 4.7 | 5.8 | 1.2× slower | 1.5× slower |
| get 64 MiB | 47.5 | 59.0 | 1.2× slower | 17× faster |
| truncate 4 KiB, end of 1 MiB | 5.1 | 6.6 | 1.3× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 4.4 | 5.7 | 1.3× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 5.3 | 7.1 | 1.3× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 5.0 | 6.8 | 1.4× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 4.7 | 6.7 | 1.4× slower | 1.0× faster |
| overwrite 1 MiB | 4.2 | 7.7 | 1.8× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.4 | 8.1 | 1.8× slower | 2.8× slower |
| put 1 MiB | 3.2 | 6.3 | 2.0× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.7 | 21.0 | 4.5× slower | 1.5× slower |

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
| truncate 4 KiB, end of 64 MiB | 64 | 1.70 |  | 0.02 |  | 1.00 | 0.69 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| insert 4 KiB, start of 64 MiB | 64 | 1.81 |  | 0.03 |  | 1.00 | 0.78 |
| write at 4 KiB in 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| delete 4 KiB, start of 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| insert 4 KiB, start of 32 MiB | 64 | 1.73 |  |  |  | 1.02 | 0.72 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.75 |  | 4.28 |  | 15.48 | 0.98 |
| patch 16 × 4 KiB in 32 MiB | 64 | 13.20 |  | 0.05 |  | 12.36 | 0.80 |
| append 4 KiB to 1 MiB | 128 | 1.53 |  |  |  | 1.00 | 0.53 |
| put 64 MiB | 64 | 28.81 |  |  |  | 27.91 | 0.91 |
| put 32 MiB | 64 | 15.14 |  |  |  | 14.19 | 0.95 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.96 |  | 0.01 |  | 1.20 | 0.76 |
| delete 4 KiB, start of 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| write at 4 KiB in 1 MiB | 128 | 1.54 |  |  |  | 1.00 | 0.54 |
| insert 4 KiB, start of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| put 1 MiB | 400 | 1.89 |  |  |  | 1.19 | 0.70 |
| multipart put 256 MiB × 16 MiB | 32 | 175.62 | 1.00 | 33.38 | 1.00 | 139.34 | 0.91 |
| overwrite 1 MiB | 400 | 1.89 |  |  |  | 1.19 | 0.70 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.97 | 1.00 | 17.00 | 1.00 | 42.25 | 0.72 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
