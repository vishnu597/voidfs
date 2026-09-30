# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T152100Z` |
| When | 2026-09-30T15:21:00Z |
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
| move dir 200 × 64 KiB | 384 | 0.92 | 416× faster | 18× faster |
| rename 64 MiB | 173 | 1.4 | 126× faster | 7.9× faster |
| truncate 4 KiB, end of 32 MiB | 294 | 5.8 | 51× faster | 5.9× faster |
| truncate 4 KiB, end of 64 MiB | 205 | 5.6 | 36× faster | 13× faster |
| list 200 keys | 36.5 | 1.1 | 34× faster | 9.1× faster |
| append 4 KiB to 64 MiB | 182 | 5.5 | 33× faster | 15× faster |
| write at 4 KiB in 64 MiB | 229 | 7.0 | 33× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 259 | 10.3 | 25× faster | 6.9× faster |
| insert 4 KiB, start of 64 MiB | 279 | 11.3 | 25× faster | 6.8× faster |
| insert 4 KiB, middle of 64 MiB | 259 | 11.4 | 23× faster | 11× faster |
| delete 4 KiB, start of 32 MiB | 109 | 6.4 | 17× faster | 4.6× faster |
| write at 4 KiB in 32 MiB | 91.4 | 6.7 | 14× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 107 | 8.7 | 12× faster | 8.0× faster |
| delete 4 KiB, middle of 64 MiB | 194 | 19.7 | 9.9× faster | 11× faster |
| delete 4 KiB, middle of 32 MiB | 109 | 11.9 | 9.1× faster | 4.8× faster |
| insert 4 KiB, start of 32 MiB | 106 | 12.0 | 8.8× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 92.9 | 11.1 | 8.4× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.45 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.8 | 0.91 | 5.3× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.9× faster | 34× faster |
| fanout get 200 × 256 KiB, 32 at once | 4.1 | 0.87 | 4.7× faster | 6.2× faster |
| head | 0.81 | 0.20 | 4.0× faster | 13× faster |
| get 4 KiB | 0.85 | 0.21 | 4.0× faster | 23× faster |
| multipart put 256 MiB × 16 MiB | 3,001 | 1,029 | 2.9× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 528 | 249 | 2.1× faster | 2.4× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.4 | 3.3 | 1.9× faster | 2.4× slower |
| patch 16 × 4 KiB in 64 MiB | 195 | 105 | 1.9× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.2 | 2.1 | 1.5× faster | 2.2× slower |
| get 1 MiB | 1.0 | 0.73 | 1.4× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 92.2 | 75.6 | 1.2× faster | 1.4× faster |
| overwrite 4 KiB | 1.0 | 0.92 | 1.1× faster | 3.1× slower |
| get 32 MiB | 27.4 | 28.6 | 1.0× slower | 12× faster |
| get 64 MiB | 52.6 | 56.4 | 1.1× slower | 17× faster |
| stream get 64 MiB | 47.7 | 52.6 | 1.1× slower | 15× faster |
| stream get 256 MiB | 192 | 213 | 1.1× slower | 16× faster |
| delete 4 KiB, start of 1 MiB | 4.6 | 5.1 | 1.1× slower | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 5.3 | 6.0 | 1.1× slower | 1.4× slower |
| put 32 MiB | 92.7 | 106 | 1.1× slower | 1.2× slower |
| truncate 4 KiB, end of 1 MiB | 4.1 | 4.8 | 1.2× slower | 2.1× slower |
| append 4 KiB to 1 MiB | 4.4 | 5.2 | 1.2× slower | 1.0× faster |
| put 4 KiB | 0.85 | 1.0 | 1.2× slower | 2.0× slower |
| put 64 MiB | 155 | 195 | 1.3× slower | 1.1× slower |
| write at 4 KiB in 1 MiB | 4.3 | 5.4 | 1.3× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 4.6 | 5.9 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 4.1 | 5.4 | 1.3× slower | 1.7× slower |
| overwrite 1 MiB | 3.2 | 5.9 | 1.9× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.4 | 6.3 | 1.9× slower | 2.8× slower |
| put 1 MiB | 3.1 | 5.9 | 1.9× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.3 | 19.7 | 4.6× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.69 |  | 0.02 |  | 1.00 | 0.67 |
| insert 4 KiB, start of 64 MiB | 64 | 1.70 |  | 0.02 |  | 1.00 | 0.69 |
| write at 4 KiB in 64 MiB | 64 | 1.75 |  | 0.05 |  | 1.00 | 0.70 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.80 |  |  |  | 1.00 | 0.80 |
| delete 4 KiB, start of 32 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| insert 4 KiB, start of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.66 |  | 4.72 |  | 15.02 | 0.92 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.62 |  | 0.08 |  | 11.64 | 0.91 |
| append 4 KiB to 1 MiB | 128 | 1.59 |  |  |  | 1.01 | 0.59 |
| put 64 MiB | 64 | 29.91 |  | 0.02 |  | 28.91 | 0.98 |
| put 32 MiB | 64 | 14.94 |  |  |  | 14.00 | 0.94 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.66 |  |  |  | 1.00 | 0.66 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.88 |  |  |  | 1.17 | 0.71 |
| delete 4 KiB, start of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| write at 4 KiB in 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| insert 4 KiB, start of 1 MiB | 128 | 1.66 |  |  |  | 1.00 | 0.66 |
| put 1 MiB | 400 | 1.88 |  |  |  | 1.17 | 0.71 |
| multipart put 256 MiB × 16 MiB | 32 | 174.81 | 1.00 | 33.00 | 1.00 | 138.91 | 0.91 |
| overwrite 1 MiB | 400 | 1.81 |  |  |  | 1.16 | 0.65 |
| put 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.75 | 1.00 | 17.00 | 1.00 | 42.00 | 0.75 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
