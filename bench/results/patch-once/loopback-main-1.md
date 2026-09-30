# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T191425Z` |
| When | 2026-09-30T19:14:25Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 10feda4 |
| distance to the bucket | none (loopback) |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.3×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 369 | 0.94 | 393× faster | 18× faster |
| rename 64 MiB | 106 | 1.1 | 99× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 187 | 5.0 | 38× faster | 13× faster |
| list 200 keys | 35.1 | 1.1 | 32× faster | 9.1× faster |
| append 4 KiB to 64 MiB | 180 | 7.5 | 24× faster | 15× faster |
| write at 4 KiB in 64 MiB | 211 | 8.9 | 24× faster | 6.2× faster |
| truncate 4 KiB, end of 32 MiB | 106 | 4.6 | 23× faster | 5.9× faster |
| insert 4 KiB, start of 64 MiB | 184 | 9.7 | 19× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 193 | 10.9 | 18× faster | 6.9× faster |
| append 4 KiB to 32 MiB | 94.6 | 5.5 | 17× faster | 8.0× faster |
| delete 4 KiB, middle of 64 MiB | 205 | 12.2 | 17× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 224 | 14.5 | 15× faster | 11× faster |
| write at 4 KiB in 32 MiB | 112 | 8.2 | 14× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 113 | 8.5 | 13× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 102 | 9.6 | 11× faster | 3.5× faster |
| delete 4 KiB, middle of 32 MiB | 93.9 | 9.8 | 9.6× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 107 | 12.1 | 8.8× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.45 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.5 | 0.89 | 5.0× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.8× faster | 34× faster |
| get 4 KiB | 0.84 | 0.22 | 3.9× faster | 23× faster |
| head | 0.74 | 0.20 | 3.7× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.81 | 3.1× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 199 | 96.8 | 2.1× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 4.8 | 3.3 | 1.5× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,108 | 789 | 1.4× faster | 1.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 2.8 | 2.0 | 1.4× faster | 2.2× slower |
| patch 16 × 4 KiB in 32 MiB | 95.4 | 69.8 | 1.4× faster | 1.4× faster |
| get 1 MiB | 0.99 | 0.89 | 1.1× faster | 4.5× faster |
| overwrite 4 KiB | 0.98 | 0.89 | 1.1× faster | 3.1× slower |
| multipart put 64 MiB × 8 MiB | 233 | 220 | 1.1× faster | 2.4× slower |
| get 32 MiB | 25.3 | 26.3 | 1.0× slower | 12× faster |
| append 4 KiB to 1 MiB | 4.4 | 4.6 | 1.0× slower | 1.0× faster |
| put 4 KiB | 0.85 | 0.94 | 1.1× slower | 2.0× slower |
| put 32 MiB | 80.1 | 90.1 | 1.1× slower | 1.2× slower |
| delete 4 KiB, middle of 1 MiB | 4.1 | 4.8 | 1.2× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.9 | 5.7 | 1.2× slower | 1.6× slower |
| stream get 256 MiB | 166 | 198 | 1.2× slower | 16× faster |
| stream get 64 MiB | 41.4 | 49.7 | 1.2× slower | 15× faster |
| get 64 MiB | 42.9 | 52.9 | 1.2× slower | 17× faster |
| put 64 MiB | 149 | 184 | 1.2× slower | 1.1× slower |
| insert 4 KiB, start of 1 MiB | 4.3 | 5.4 | 1.3× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 4.0 | 5.2 | 1.3× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 4.0 | 5.3 | 1.3× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 4.2 | 5.6 | 1.3× slower | 2.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.6 | 6.4 | 1.8× slower | 2.8× slower |
| put 1 MiB | 3.1 | 5.6 | 1.8× slower | 1.7× slower |
| overwrite 1 MiB | 3.0 | 5.6 | 1.9× slower | 1.9× slower |
| patch 16 × 4 KiB in 1 MiB | 4.2 | 18.6 | 4.5× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.64 |  | 0.02 |  | 1.00 | 0.62 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.59 |  |  |  | 1.00 | 0.59 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| insert 4 KiB, start of 64 MiB | 64 | 1.75 |  | 0.03 |  | 1.00 | 0.72 |
| write at 4 KiB in 64 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| delete 4 KiB, start of 32 MiB | 64 | 1.81 |  |  |  | 1.00 | 0.81 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| insert 4 KiB, start of 32 MiB | 64 | 1.62 |  |  |  | 1.00 | 0.62 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.25 |  | 4.75 |  | 15.66 | 0.84 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.19 |  | 0.09 |  | 10.16 | 0.94 |
| append 4 KiB to 1 MiB | 128 | 1.53 |  |  |  | 1.01 | 0.52 |
| put 64 MiB | 64 | 29.52 |  |  |  | 28.58 | 0.94 |
| put 32 MiB | 64 | 15.30 |  |  |  | 14.30 | 1.00 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.64 |  |  |  | 1.00 | 0.64 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.80 |  |  |  | 1.17 | 0.62 |
| delete 4 KiB, start of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| write at 4 KiB in 1 MiB | 128 | 1.52 |  |  |  | 1.00 | 0.52 |
| insert 4 KiB, start of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| put 1 MiB | 400 | 1.89 |  |  |  | 1.18 | 0.71 |
| multipart put 256 MiB × 16 MiB | 32 | 173.50 | 1.00 | 33.00 | 1.00 | 137.62 | 0.88 |
| overwrite 1 MiB | 400 | 1.87 |  |  |  | 1.17 | 0.70 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.41 | 1.00 | 17.00 | 1.00 | 41.69 | 0.72 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
