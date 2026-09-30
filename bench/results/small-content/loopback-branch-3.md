# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T030343Z` |
| When | 2026-09-30T03:03:43Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.5×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 387 | 0.97 | 401× faster | 18× faster |
| rename 64 MiB | 103 | 0.86 | 120× faster | 7.9× faster |
| list 200 keys | 38.4 | 1.1 | 35× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 208 | 6.3 | 33× faster | 13× faster |
| append 4 KiB to 32 MiB | 108 | 4.6 | 24× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 114 | 5.0 | 23× faster | 4.6× faster |
| delete 4 KiB, middle of 64 MiB | 226 | 10.1 | 22× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 221 | 10.0 | 22× faster | 6.8× faster |
| truncate 4 KiB, end of 32 MiB | 105 | 4.9 | 21× faster | 5.9× faster |
| insert 4 KiB, middle of 64 MiB | 258 | 12.0 | 21× faster | 11× faster |
| write at 4 KiB in 64 MiB | 211 | 10.5 | 20× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 185 | 9.3 | 20× faster | 15× faster |
| delete 4 KiB, start of 64 MiB | 199 | 12.0 | 17× faster | 6.9× faster |
| insert 4 KiB, start of 32 MiB | 105 | 7.7 | 14× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 107 | 9.9 | 11× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 103 | 11.2 | 9.1× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 111 | 12.3 | 9.0× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.46 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.8 | 0.90 | 5.4× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.6× faster | 34× faster |
| head | 0.76 | 0.19 | 4.1× faster | 13× faster |
| get 4 KiB | 0.83 | 0.22 | 3.8× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.79 | 3.5× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 256 | 109 | 2.3× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.3 | 3.5 | 1.8× faster | 2.4× slower |
| multipart put 64 MiB × 8 MiB | 354 | 250 | 1.4× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 119 | 86.4 | 1.4× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,339 | 981 | 1.4× faster | 1.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.3 | 2.5 | 1.3× faster | 2.2× slower |
| get 1 MiB | 1.0 | 0.80 | 1.3× faster | 4.5× faster |
| overwrite 4 KiB | 1.4 | 1.4 | 1.0× faster | 3.1× slower |
| get 32 MiB | 27.8 | 29.5 | 1.1× slower | 12× faster |
| truncate 4 KiB, end of 1 MiB | 5.4 | 5.8 | 1.1× slower | 2.1× slower |
| stream get 64 MiB | 48.1 | 52.9 | 1.1× slower | 15× faster |
| append 4 KiB to 1 MiB | 4.8 | 5.3 | 1.1× slower | 1.0× faster |
| stream get 256 MiB | 191 | 214 | 1.1× slower | 16× faster |
| delete 4 KiB, start of 1 MiB | 4.6 | 5.2 | 1.1× slower | 1.5× slower |
| put 32 MiB | 97.5 | 111 | 1.1× slower | 1.2× slower |
| put 64 MiB | 191 | 219 | 1.1× slower | 1.1× slower |
| get 64 MiB | 47.2 | 54.6 | 1.2× slower | 17× faster |
| write at 4 KiB in 1 MiB | 4.4 | 5.4 | 1.2× slower | 1.6× slower |
| insert 4 KiB, start of 1 MiB | 4.4 | 5.6 | 1.3× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 5.8 | 7.4 | 1.3× slower | 1.4× slower |
| put 4 KiB | 1.0 | 1.3 | 1.3× slower | 2.0× slower |
| insert 4 KiB, middle of 1 MiB | 5.2 | 6.9 | 1.3× slower | 1.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.5 | 7.9 | 1.8× slower | 2.8× slower |
| overwrite 1 MiB | 4.3 | 8.0 | 1.8× slower | 1.9× slower |
| put 1 MiB | 3.2 | 6.0 | 1.8× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.7 | 22.8 | 4.9× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.56 |  |  |  | 1.00 | 0.56 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.78 |  | 0.05 |  | 1.00 | 0.73 |
| insert 4 KiB, start of 64 MiB | 64 | 1.77 |  | 0.03 |  | 1.00 | 0.73 |
| write at 4 KiB in 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| delete 4 KiB, start of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| insert 4 KiB, start of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.88 |  | 5.25 |  | 15.80 | 0.83 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.75 |  | 0.03 |  | 11.84 | 0.88 |
| append 4 KiB to 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| put 64 MiB | 64 | 29.70 |  |  |  | 28.75 | 0.95 |
| put 32 MiB | 64 | 15.55 |  | 0.02 |  | 14.62 | 0.91 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.89 |  |  |  | 1.16 | 0.73 |
| delete 4 KiB, start of 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| write at 4 KiB in 1 MiB | 128 | 1.68 |  |  |  | 1.00 | 0.68 |
| insert 4 KiB, start of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| put 1 MiB | 400 | 1.85 |  |  |  | 1.19 | 0.67 |
| multipart put 256 MiB × 16 MiB | 32 | 174.84 | 1.00 | 33.00 | 1.00 | 138.94 | 0.91 |
| overwrite 1 MiB | 400 | 1.86 |  |  |  | 1.16 | 0.70 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 62.03 | 1.00 | 17.00 | 1.00 | 42.28 | 0.75 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.14 |  |  |  | 1.00 | 0.14 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
