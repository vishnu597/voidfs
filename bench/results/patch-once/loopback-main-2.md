# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T191821Z` |
| When | 2026-09-30T19:18:21Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.6×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 388 | 1.3 | 308× faster | 18× faster |
| rename 64 MiB | 116 | 0.76 | 153× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 225 | 4.7 | 48× faster | 15× faster |
| write at 4 KiB in 64 MiB | 257 | 6.3 | 41× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 190 | 4.9 | 39× faster | 13× faster |
| list 200 keys | 36.7 | 1.1 | 33× faster | 9.1× faster |
| delete 4 KiB, start of 64 MiB | 215 | 9.2 | 23× faster | 6.9× faster |
| append 4 KiB to 32 MiB | 103 | 4.5 | 23× faster | 8.0× faster |
| delete 4 KiB, middle of 64 MiB | 236 | 11.0 | 22× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 215 | 10.5 | 21× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 230 | 12.0 | 19× faster | 6.8× faster |
| insert 4 KiB, start of 32 MiB | 99.9 | 5.2 | 19× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 98.9 | 5.2 | 19× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 105 | 6.6 | 16× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 101 | 9.5 | 11× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 112 | 10.6 | 11× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 105 | 12.4 | 8.4× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.7 | 0.47 | 5.6× faster | 5.3× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.22 | 5.0× faster | 34× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.3 | 0.94 | 4.5× faster | 2.6× faster |
| head | 0.74 | 0.19 | 3.9× faster | 13× faster |
| get 4 KiB | 0.83 | 0.21 | 3.9× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.72 | 3.7× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 218 | 115 | 1.9× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.1 | 3.3 | 1.8× faster | 2.4× slower |
| get 1 MiB | 1.1 | 0.71 | 1.5× faster | 4.5× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.2 | 2.3 | 1.4× faster | 2.2× slower |
| patch 16 × 4 KiB in 32 MiB | 101 | 74.6 | 1.4× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 292 | 234 | 1.2× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,297 | 1,058 | 1.2× faster | 1.8× slower |
| overwrite 4 KiB | 1.1 | 1.1 | 1.0× faster | 3.1× slower |
| truncate 4 KiB, end of 1 MiB | 5.3 | 5.6 | 1.1× slower | 2.1× slower |
| stream get 256 MiB | 180 | 202 | 1.1× slower | 16× faster |
| put 32 MiB | 97.5 | 110 | 1.1× slower | 1.2× slower |
| put 64 MiB | 188 | 213 | 1.1× slower | 1.1× slower |
| get 32 MiB | 24.4 | 27.8 | 1.1× slower | 12× faster |
| delete 4 KiB, start of 1 MiB | 4.2 | 4.9 | 1.2× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 4.2 | 4.9 | 1.2× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 5.4 | 6.3 | 1.2× slower | 1.4× slower |
| get 64 MiB | 46.3 | 54.7 | 1.2× slower | 17× faster |
| stream get 64 MiB | 42.6 | 50.7 | 1.2× slower | 15× faster |
| put 4 KiB | 1.1 | 1.3 | 1.2× slower | 2.0× slower |
| insert 4 KiB, start of 1 MiB | 4.2 | 5.4 | 1.3× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 4.7 | 6.4 | 1.4× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 4.3 | 6.1 | 1.4× slower | 1.0× faster |
| overwrite 1 MiB | 4.2 | 7.4 | 1.8× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.2 | 7.6 | 1.8× slower | 2.8× slower |
| put 1 MiB | 3.1 | 6.2 | 2.0× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.4 | 20.6 | 4.7× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.64 |  | 0.03 |  | 1.00 | 0.61 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.88 |  |  |  | 1.00 | 0.88 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.84 |  | 0.05 |  | 1.00 | 0.80 |
| insert 4 KiB, start of 64 MiB | 64 | 1.73 |  | 0.03 |  | 1.00 | 0.70 |
| write at 4 KiB in 64 MiB | 64 | 1.72 |  | 0.05 |  | 1.00 | 0.67 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| delete 4 KiB, start of 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.78 |  |  |  | 1.02 | 0.77 |
| insert 4 KiB, start of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.20 |  | 5.25 |  | 15.16 | 0.80 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.20 |  | 0.06 |  | 11.16 | 0.98 |
| append 4 KiB to 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| put 64 MiB | 64 | 29.39 |  |  |  | 28.47 | 0.92 |
| put 32 MiB | 64 | 15.42 |  |  |  | 14.44 | 0.98 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.70 |  |  |  | 1.00 | 0.70 |
| delete 4 KiB, start of 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| write at 4 KiB in 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| insert 4 KiB, start of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| put 1 MiB | 400 | 1.80 |  |  |  | 1.16 | 0.65 |
| multipart put 256 MiB × 16 MiB | 32 | 174.47 | 1.00 | 33.00 | 1.00 | 138.75 | 0.72 |
| overwrite 1 MiB | 400 | 1.91 |  |  |  | 1.19 | 0.72 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.03 | 1.00 | 17.00 | 1.00 | 41.25 | 0.78 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
