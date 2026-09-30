# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T152428Z` |
| When | 2026-09-30T15:24:28Z |
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

voidfs is faster in **33 of 49** scenarios and slower in the other **16**. Geometric mean speed-up over the bare bucket: **3.7×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 367 | 0.91 | 402× faster | 18× faster |
| rename 64 MiB | 369 | 1.6 | 224× faster | 7.9× faster |
| write at 4 KiB in 64 MiB | 797 | 12.4 | 64× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 281 | 5.9 | 47× faster | 15× faster |
| list 200 keys | 34.8 | 1.1 | 33× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 257 | 9.0 | 29× faster | 13× faster |
| delete 4 KiB, start of 64 MiB | 256 | 9.1 | 28× faster | 6.9× faster |
| insert 4 KiB, start of 64 MiB | 286 | 11.5 | 25× faster | 6.8× faster |
| insert 4 KiB, middle of 64 MiB | 243 | 11.1 | 22× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 223 | 10.2 | 22× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 102 | 6.7 | 15× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 111 | 9.1 | 12× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 105 | 8.8 | 12× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 113 | 9.7 | 12× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 107 | 10.9 | 9.8× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 105 | 11.7 | 8.9× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 104 | 12.0 | 8.7× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.49 | 5.2× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.7 | 0.90 | 5.2× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.22 | 4.8× faster | 34× faster |
| head | 0.74 | 0.17 | 4.4× faster | 13× faster |
| get 4 KiB | 0.84 | 0.21 | 4.0× faster | 23× faster |
| put 32 MiB | 403 | 109 | 3.7× faster | 1.2× slower |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.82 | 3.2× faster | 6.2× faster |
| multipart put 64 MiB × 8 MiB | 515 | 242 | 2.1× faster | 2.4× slower |
| patch 16 × 4 KiB in 64 MiB | 253 | 130 | 1.9× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.0 | 3.5 | 1.7× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 3,426 | 2,109 | 1.6× faster | 1.8× slower |
| patch 16 × 4 KiB in 32 MiB | 122 | 75.3 | 1.6× faster | 1.4× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.3 | 2.2 | 1.5× faster | 2.2× slower |
| get 1 MiB | 1.0 | 0.86 | 1.2× faster | 4.5× faster |
| put 64 MiB | 277 | 237 | 1.2× faster | 1.1× slower |
| overwrite 4 KiB | 1.0 | 0.94 | 1.1× faster | 3.1× slower |
| stream get 256 MiB | 193 | 202 | 1.0× slower | 16× faster |
| insert 4 KiB, middle of 1 MiB | 4.2 | 4.5 | 1.1× slower | 1.4× slower |
| stream get 64 MiB | 47.6 | 51.8 | 1.1× slower | 15× faster |
| get 32 MiB | 25.8 | 28.3 | 1.1× slower | 12× faster |
| get 64 MiB | 51.3 | 57.3 | 1.1× slower | 17× faster |
| put 4 KiB | 0.83 | 0.96 | 1.2× slower | 2.0× slower |
| write at 4 KiB in 1 MiB | 4.4 | 5.2 | 1.2× slower | 1.6× slower |
| delete 4 KiB, start of 1 MiB | 4.1 | 4.9 | 1.2× slower | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 4.2 | 5.1 | 1.2× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 4.1 | 5.2 | 1.2× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 4.0 | 5.4 | 1.3× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 4.0 | 5.7 | 1.4× slower | 1.0× faster |
| overwrite 1 MiB | 3.5 | 5.8 | 1.7× slower | 1.9× slower |
| put 1 MiB | 3.0 | 5.5 | 1.9× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.4 | 6.4 | 1.9× slower | 2.8× slower |
| patch 16 × 4 KiB in 1 MiB | 4.3 | 21.5 | 5.0× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.64 |  |  |  | 1.03 | 0.61 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.80 |  |  |  | 1.00 | 0.80 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| insert 4 KiB, start of 64 MiB | 64 | 1.73 |  | 0.03 |  | 1.00 | 0.70 |
| write at 4 KiB in 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| delete 4 KiB, start of 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| insert 4 KiB, start of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.30 |  | 5.17 |  | 15.31 | 0.81 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.70 |  |  |  | 11.84 | 0.86 |
| append 4 KiB to 1 MiB | 128 | 1.53 |  |  |  | 1.00 | 0.53 |
| put 64 MiB | 64 | 29.12 |  |  |  | 28.33 | 0.80 |
| put 32 MiB | 64 | 15.14 |  |  |  | 14.22 | 0.92 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.79 |  |  |  | 1.12 | 0.66 |
| delete 4 KiB, start of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| write at 4 KiB in 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| insert 4 KiB, start of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| put 1 MiB | 400 | 1.93 |  |  |  | 1.22 | 0.70 |
| multipart put 256 MiB × 16 MiB | 32 | 174.84 | 1.00 | 33.00 | 1.00 | 139.00 | 0.84 |
| overwrite 1 MiB | 400 | 1.84 |  |  |  | 1.15 | 0.69 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 62.00 | 1.00 | 17.03 | 1.00 | 42.09 | 0.88 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
