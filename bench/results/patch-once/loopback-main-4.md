# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T192222Z` |
| When | 2026-09-30T19:22:22Z |
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

voidfs is faster in **32 of 49** scenarios and slower in the other **17**. Geometric mean speed-up over the bare bucket: **3.4×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 385 | 0.96 | 403× faster | 18× faster |
| rename 64 MiB | 108 | 0.82 | 131× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 197 | 4.6 | 43× faster | 13× faster |
| append 4 KiB to 64 MiB | 236 | 5.8 | 41× faster | 15× faster |
| list 200 keys | 35.2 | 1.0 | 34× faster | 9.1× faster |
| write at 4 KiB in 64 MiB | 175 | 7.4 | 24× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 184 | 7.8 | 24× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 216 | 9.7 | 22× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 208 | 12.8 | 16× faster | 6.8× faster |
| write at 4 KiB in 32 MiB | 99.5 | 6.6 | 15× faster | 5.1× faster |
| delete 4 KiB, middle of 64 MiB | 175 | 12.0 | 15× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 94.7 | 8.8 | 11× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 106 | 10.6 | 10.0× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 117 | 12.2 | 9.5× faster | 3.5× faster |
| delete 4 KiB, start of 32 MiB | 96.7 | 11.1 | 8.7× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 101 | 11.7 | 8.6× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 105 | 16.6 | 6.3× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.43 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.6 | 0.85 | 5.3× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.24 | 4.8× faster | 34× faster |
| get 4 KiB | 0.82 | 0.21 | 3.8× faster | 23× faster |
| head | 0.73 | 0.20 | 3.7× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.80 | 3.1× faster | 6.2× faster |
| multipart put 256 MiB × 16 MiB | 2,508 | 848 | 3.0× faster | 1.8× slower |
| patch 16 × 4 KiB in 64 MiB | 198 | 92.8 | 2.1× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 5.1 | 3.1 | 1.6× faster | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.1 | 2.0 | 1.6× faster | 2.2× slower |
| patch 16 × 4 KiB in 32 MiB | 103 | 71.2 | 1.4× faster | 1.4× faster |
| get 1 MiB | 1.0 | 0.78 | 1.3× faster | 4.5× faster |
| multipart put 64 MiB × 8 MiB | 253 | 209 | 1.2× faster | 2.4× slower |
| overwrite 4 KiB | 0.95 | 0.80 | 1.2× faster | 3.1× slower |
| put 32 MiB | 110 | 108 | 1.0× faster | 1.2× slower |
| put 4 KiB | 0.82 | 0.87 | 1.1× slower | 2.0× slower |
| get 64 MiB | 46.8 | 50.0 | 1.1× slower | 17× faster |
| put 64 MiB | 207 | 225 | 1.1× slower | 1.1× slower |
| stream get 256 MiB | 166 | 182 | 1.1× slower | 16× faster |
| stream get 64 MiB | 42.6 | 48.1 | 1.1× slower | 15× faster |
| get 32 MiB | 22.5 | 25.5 | 1.1× slower | 12× faster |
| append 4 KiB to 1 MiB | 4.3 | 5.0 | 1.2× slower | 1.0× faster |
| truncate 4 KiB, end of 1 MiB | 4.1 | 4.9 | 1.2× slower | 2.1× slower |
| delete 4 KiB, middle of 1 MiB | 4.0 | 5.1 | 1.3× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.0 | 5.1 | 1.3× slower | 1.6× slower |
| delete 4 KiB, start of 1 MiB | 4.1 | 5.3 | 1.3× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 4.2 | 5.7 | 1.3× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 4.2 | 5.8 | 1.4× slower | 1.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.5 | 6.1 | 1.7× slower | 2.8× slower |
| overwrite 1 MiB | 3.3 | 5.8 | 1.8× slower | 1.9× slower |
| put 1 MiB | 2.9 | 5.5 | 1.9× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.2 | 19.5 | 4.7× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.66 |  | 0.03 |  | 1.00 | 0.62 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| insert 4 KiB, start of 64 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| write at 4 KiB in 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.64 |  |  |  | 1.02 | 0.62 |
| delete 4 KiB, start of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| insert 4 KiB, start of 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.53 |  | 4.98 |  | 15.66 | 0.89 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.64 |  | 0.17 |  | 11.58 | 0.89 |
| append 4 KiB to 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| put 64 MiB | 64 | 28.83 |  |  |  | 28.02 | 0.81 |
| put 32 MiB | 64 | 15.22 |  |  |  | 14.34 | 0.88 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.84 |  |  |  | 1.17 | 0.66 |
| delete 4 KiB, start of 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| write at 4 KiB in 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| insert 4 KiB, start of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| put 1 MiB | 400 | 1.90 |  |  |  | 1.21 | 0.69 |
| multipart put 256 MiB × 16 MiB | 32 | 174.75 | 1.00 | 33.00 | 1.00 | 138.91 | 0.84 |
| overwrite 1 MiB | 400 | 1.78 |  |  |  | 1.15 | 0.64 |
| put 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.59 | 1.00 | 17.00 | 1.00 | 41.69 | 0.91 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
