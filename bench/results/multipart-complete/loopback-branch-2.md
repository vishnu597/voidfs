# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T234014Z` |
| When | 2026-09-30T23:40:14Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | a03a25d |
| distance to the bucket | none (loopback) |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **3.5×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 383 | 0.97 | 393× faster | 18× faster |
| rename 64 MiB | 93.7 | 0.80 | 117× faster | 7.9× faster |
| list 200 keys | 35.5 | 0.93 | 38× faster | 9.1× faster |
| insert 4 KiB, start of 64 MiB | 186 | 6.1 | 30× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 185 | 6.2 | 30× faster | 13× faster |
| delete 4 KiB, middle of 64 MiB | 275 | 10.5 | 26× faster | 11× faster |
| append 4 KiB to 64 MiB | 178 | 6.8 | 26× faster | 15× faster |
| append 4 KiB to 32 MiB | 116 | 4.9 | 24× faster | 8.0× faster |
| delete 4 KiB, start of 64 MiB | 200 | 10.0 | 20× faster | 6.9× faster |
| write at 4 KiB in 64 MiB | 211 | 13.4 | 16× faster | 6.2× faster |
| insert 4 KiB, start of 32 MiB | 121 | 8.4 | 14× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 98.7 | 8.0 | 12× faster | 5.9× faster |
| delete 4 KiB, middle of 32 MiB | 104 | 8.9 | 12× faster | 4.8× faster |
| insert 4 KiB, middle of 64 MiB | 178 | 15.8 | 11× faster | 11× faster |
| delete 4 KiB, start of 32 MiB | 96.9 | 8.6 | 11× faster | 4.6× faster |
| write at 4 KiB in 32 MiB | 106 | 11.2 | 9.5× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 108 | 12.8 | 8.4× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.43 | 5.9× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.5 | 0.84 | 5.4× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.8× faster | 34× faster |
| get 4 KiB | 0.85 | 0.19 | 4.4× faster | 23× faster |
| head | 0.75 | 0.17 | 4.3× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.75 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 185 | 108 | 1.7× faster | 2.1× faster |
| multipart put 64 MiB × 8 MiB | 312 | 199 | 1.6× faster | 2.4× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.9 | 3.8 | 1.6× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,345 | 968 | 1.4× faster | 1.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.3 | 2.4 | 1.4× faster | 2.2× slower |
| patch 16 × 4 KiB in 32 MiB | 96.0 | 76.8 | 1.2× faster | 1.4× faster |
| get 1 MiB | 1.0 | 0.92 | 1.1× faster | 4.5× faster |
| stream get 256 MiB | 194 | 194 | 1.0× slower | 16× faster |
| overwrite 4 KiB | 1.2 | 1.3 | 1.0× slower | 3.1× slower |
| stream get 64 MiB | 47.2 | 49.5 | 1.0× slower | 15× faster |
| truncate 4 KiB, end of 1 MiB | 4.8 | 5.1 | 1.1× slower | 2.1× slower |
| put 64 MiB | 187 | 207 | 1.1× slower | 1.1× slower |
| get 64 MiB | 46.3 | 51.2 | 1.1× slower | 17× faster |
| get 32 MiB | 24.8 | 27.9 | 1.1× slower | 12× faster |
| put 32 MiB | 93.2 | 105 | 1.1× slower | 1.2× slower |
| delete 4 KiB, start of 1 MiB | 4.8 | 5.6 | 1.2× slower | 1.5× slower |
| append 4 KiB to 1 MiB | 4.9 | 5.8 | 1.2× slower | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 5.3 | 6.4 | 1.2× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 4.6 | 5.8 | 1.3× slower | 1.7× slower |
| put 4 KiB | 1.0 | 1.3 | 1.3× slower | 2.0× slower |
| write at 4 KiB in 1 MiB | 4.5 | 5.7 | 1.3× slower | 1.6× slower |
| patch 16 × 4 KiB in 1 MiB | 4.5 | 6.2 | 1.4× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 4.6 | 6.7 | 1.4× slower | 1.4× slower |
| put 1 MiB | 3.4 | 5.9 | 1.8× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.3 | 8.0 | 1.9× slower | 2.8× slower |
| overwrite 1 MiB | 4.0 | 7.6 | 1.9× slower | 1.9× slower |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | delete_prefix | get | put | put_new |
|---|--:|--:|--:|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.00 |  |  |  |  |
| get 4 KiB | 400 | 0.00 |  |  |  |  |
| move dir 200 × 64 KiB | 32 | 0.25 |  |  |  | 0.25 |
| get 64 MiB | 64 | 0.00 |  |  |  |  |
| stream get 256 MiB | 32 | 0.00 |  |  |  |  |
| stream get 64 MiB | 64 | 0.00 |  |  |  |  |
| append 4 KiB to 64 MiB | 64 | 1.64 |  |  | 1.00 | 0.64 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.78 |  | 0.03 | 1.00 | 0.75 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.77 |  |  | 1.00 | 0.77 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.59 |  |  | 1.00 | 0.59 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.80 |  |  | 1.00 | 0.80 |
| rename 64 MiB | 64 | 0.25 |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.70 |  | 0.05 | 1.00 | 0.66 |
| insert 4 KiB, start of 64 MiB | 64 | 1.77 |  | 0.03 | 1.00 | 0.73 |
| write at 4 KiB in 64 MiB | 64 | 1.70 |  |  | 1.00 | 0.70 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.64 |  |  | 1.00 | 0.64 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.75 |  |  | 1.00 | 0.75 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.72 |  |  | 1.00 | 0.72 |
| delete 4 KiB, start of 32 MiB | 64 | 1.69 |  |  | 1.00 | 0.69 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.80 |  |  | 1.00 | 0.80 |
| insert 4 KiB, start of 32 MiB | 64 | 1.67 |  |  | 1.00 | 0.67 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.78 |  | 4.83 | 15.17 | 0.78 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.47 |  |  | 10.75 | 0.72 |
| append 4 KiB to 1 MiB | 128 | 1.57 |  |  | 1.00 | 0.57 |
| put 64 MiB | 64 | 29.41 |  |  | 28.45 | 0.95 |
| put 32 MiB | 64 | 15.33 |  |  | 14.34 | 0.98 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.66 |  |  | 1.00 | 0.66 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.62 |  |  | 1.00 | 0.62 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.74 |  |  | 1.15 | 0.59 |
| delete 4 KiB, start of 1 MiB | 128 | 1.60 |  | 0.01 | 1.00 | 0.59 |
| write at 4 KiB in 1 MiB | 128 | 1.58 |  |  | 1.00 | 0.58 |
| insert 4 KiB, start of 1 MiB | 128 | 1.59 |  |  | 1.00 | 0.59 |
| put 1 MiB | 400 | 1.88 |  |  | 1.20 | 0.68 |
| multipart put 256 MiB × 16 MiB | 32 | 172.91 | 0.97 | 33.00 | 138.03 | 0.91 |
| overwrite 1 MiB | 400 | 1.84 |  |  | 1.17 | 0.67 |
| put 4 KiB | 400 | 0.25 |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.55 |  |  | 1.00 | 0.55 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 60.34 | 0.91 | 17.00 | 41.56 | 0.88 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  | 0.25 |
