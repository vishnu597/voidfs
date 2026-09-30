# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T030508Z` |
| When | 2026-09-30T03:05:08Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 8ce1a94 |
| distance to the bucket | none (loopback) |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **28 of 49** scenarios and slower in the other **21**. Geometric mean speed-up over the bare bucket: **3.2×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 396 | 0.94 | 421× faster | 18× faster |
| rename 64 MiB | 110 | 0.80 | 138× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 183 | 3.1 | 60× faster | 15× faster |
| list 200 keys | 36.8 | 1.1 | 33× faster | 9.1× faster |
| write at 4 KiB in 64 MiB | 212 | 8.3 | 26× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 204 | 8.4 | 24× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 222 | 9.5 | 23× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 202 | 10.2 | 20× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 222 | 11.3 | 20× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 109 | 6.0 | 18× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 99.7 | 7.1 | 14× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 112 | 8.4 | 13× faster | 4.6× faster |
| insert 4 KiB, middle of 64 MiB | 219 | 16.6 | 13× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 114 | 9.4 | 12× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 107 | 9.0 | 12× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 110 | 11.8 | 9.3× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 109 | 12.7 | 8.6× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.7 | 0.47 | 5.6× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.9 | 0.93 | 5.2× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.25 | 4.4× faster | 34× faster |
| get 4 KiB | 0.86 | 0.21 | 4.0× faster | 23× faster |
| head | 0.77 | 0.21 | 3.7× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.82 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 217 | 113 | 1.9× faster | 2.1× faster |
| get 1 MiB | 1.1 | 0.63 | 1.7× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 117 | 78.6 | 1.5× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,393 | 1,027 | 1.4× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 306 | 241 | 1.3× faster | 2.4× slower |
| stream get 256 MiB | 199 | 208 | 1.0× slower | 16× faster |
| get 64 MiB | 49.1 | 53.4 | 1.1× slower | 17× faster |
| delete 4 KiB, start of 1 MiB | 4.6 | 5.1 | 1.1× slower | 1.5× slower |
| put 32 MiB | 98.5 | 111 | 1.1× slower | 1.2× slower |
| stream get 64 MiB | 48.6 | 54.8 | 1.1× slower | 15× faster |
| put 64 MiB | 189 | 215 | 1.1× slower | 1.1× slower |
| write at 4 KiB in 1 MiB | 4.3 | 5.2 | 1.2× slower | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 4.9 | 6.0 | 1.2× slower | 2.1× slower |
| get 32 MiB | 23.6 | 29.1 | 1.2× slower | 12× faster |
| insert 4 KiB, middle of 1 MiB | 4.7 | 6.0 | 1.3× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 5.0 | 6.5 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 4.4 | 5.8 | 1.3× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 4.9 | 6.7 | 1.4× slower | 1.0× faster |
| fanout put 200 × 256 KiB, 32 at once | 4.6 | 7.6 | 1.6× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.5 | 9.1 | 1.7× slower | 2.4× slower |
| overwrite 4 KiB | 1.3 | 2.1 | 1.7× slower | 3.1× slower |
| overwrite 1 MiB | 4.4 | 7.7 | 1.8× slower | 1.9× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.2 | 5.8 | 1.8× slower | 2.2× slower |
| put 1 MiB | 3.2 | 6.2 | 1.9× slower | 1.7× slower |
| put 4 KiB | 1.1 | 2.2 | 2.1× slower | 2.0× slower |
| patch 16 × 4 KiB in 1 MiB | 4.5 | 20.4 | 4.6× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.58 |  |  |  | 1.00 | 0.58 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.70 |  | 0.02 |  | 1.00 | 0.69 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.69 |  |  |  | 1.02 | 0.67 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.83 |  | 0.03 |  | 1.00 | 0.80 |
| insert 4 KiB, start of 64 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| write at 4 KiB in 64 MiB | 64 | 1.83 |  | 0.03 |  | 1.00 | 0.80 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| delete 4 KiB, start of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| insert 4 KiB, start of 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.47 |  | 5.03 |  | 15.53 | 0.91 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.56 |  | 0.09 |  | 11.53 | 0.94 |
| append 4 KiB to 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| put 64 MiB | 64 | 28.98 |  |  |  | 28.06 | 0.92 |
| put 32 MiB | 64 | 15.75 |  | 0.02 |  | 14.77 | 0.97 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.76 |  |  |  | 1.01 | 0.75 |
| delete 4 KiB, start of 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| write at 4 KiB in 1 MiB | 128 | 1.67 |  |  |  | 1.00 | 0.67 |
| insert 4 KiB, start of 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| put 1 MiB | 400 | 1.82 |  |  |  | 1.14 | 0.68 |
| multipart put 256 MiB × 16 MiB | 32 | 175.12 | 1.00 | 33.00 | 1.00 | 139.19 | 0.94 |
| overwrite 1 MiB | 400 | 1.99 |  |  |  | 1.22 | 0.77 |
| put 4 KiB | 400 | 1.39 |  |  |  | 1.00 | 0.39 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 1.09 |  |  |  | 1.00 | 0.09 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 1.05 |  |  |  | 1.00 | 0.05 |
| multipart put 64 MiB × 8 MiB | 32 | 61.16 | 1.00 | 17.00 | 1.00 | 41.41 | 0.75 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 1.39 |  |  |  | 1.00 | 0.39 |
