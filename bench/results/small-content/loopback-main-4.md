# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T030633Z` |
| When | 2026-09-30T03:06:33Z |
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

voidfs is faster in **28 of 49** scenarios and slower in the other **21**. Geometric mean speed-up over the bare bucket: **3.3×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 370 | 0.88 | 418× faster | 18× faster |
| rename 64 MiB | 117 | 0.86 | 136× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 191 | 4.8 | 40× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 249 | 6.9 | 36× faster | 6.8× faster |
| list 200 keys | 37.1 | 1.2 | 31× faster | 9.1× faster |
| append 4 KiB to 64 MiB | 184 | 6.5 | 28× faster | 15× faster |
| write at 4 KiB in 64 MiB | 216 | 8.5 | 25× faster | 6.2× faster |
| append 4 KiB to 32 MiB | 104 | 4.1 | 25× faster | 8.0× faster |
| insert 4 KiB, middle of 64 MiB | 215 | 10.6 | 20× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 218 | 14.6 | 15× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 206 | 13.9 | 15× faster | 6.9× faster |
| truncate 4 KiB, end of 32 MiB | 91.7 | 6.2 | 15× faster | 5.9× faster |
| insert 4 KiB, start of 32 MiB | 107 | 7.6 | 14× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 104 | 7.9 | 13× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 109 | 8.5 | 13× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 106 | 10.9 | 9.7× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 98.9 | 10.3 | 9.6× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.46 | 5.8× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 5.7 | 1.0 | 5.5× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.7× faster | 34× faster |
| head | 0.79 | 0.19 | 4.2× faster | 13× faster |
| get 4 KiB | 0.84 | 0.22 | 3.8× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.79 | 3.2× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 274 | 115 | 2.4× faster | 2.1× faster |
| get 1 MiB | 1.0 | 0.69 | 1.5× faster | 4.5× faster |
| multipart put 256 MiB × 16 MiB | 1,402 | 971 | 1.4× faster | 1.8× slower |
| patch 16 × 4 KiB in 32 MiB | 111 | 81.4 | 1.4× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 319 | 240 | 1.3× faster | 2.4× slower |
| stream get 256 MiB | 190 | 204 | 1.1× slower | 16× faster |
| put 64 MiB | 198 | 218 | 1.1× slower | 1.1× slower |
| delete 4 KiB, start of 1 MiB | 4.4 | 5.0 | 1.1× slower | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 5.4 | 6.1 | 1.1× slower | 1.4× slower |
| stream get 64 MiB | 45.7 | 52.2 | 1.1× slower | 15× faster |
| put 32 MiB | 97.0 | 111 | 1.1× slower | 1.2× slower |
| truncate 4 KiB, end of 1 MiB | 5.1 | 5.8 | 1.1× slower | 2.1× slower |
| get 32 MiB | 24.8 | 28.6 | 1.2× slower | 12× faster |
| insert 4 KiB, start of 1 MiB | 4.5 | 5.2 | 1.2× slower | 1.7× slower |
| get 64 MiB | 45.7 | 55.5 | 1.2× slower | 17× faster |
| append 4 KiB to 1 MiB | 4.5 | 5.8 | 1.3× slower | 1.0× faster |
| write at 4 KiB in 1 MiB | 4.3 | 5.7 | 1.3× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 5.1 | 7.0 | 1.4× slower | 1.4× slower |
| overwrite 4 KiB | 1.2 | 1.9 | 1.5× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.2 | 7.0 | 1.7× slower | 2.8× slower |
| overwrite 1 MiB | 4.3 | 7.6 | 1.8× slower | 1.9× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.3 | 6.0 | 1.9× slower | 2.2× slower |
| put 1 MiB | 3.1 | 5.9 | 1.9× slower | 1.7× slower |
| put 4 KiB | 1.1 | 2.2 | 1.9× slower | 2.0× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.0 | 9.9 | 2.0× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 4.4 | 21.9 | 4.9× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.73 |  |  |  | 1.02 | 0.72 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.66 |  | 0.02 |  | 1.00 | 0.64 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.80 |  |  |  | 1.02 | 0.78 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| insert 4 KiB, start of 64 MiB | 64 | 1.75 |  | 0.03 |  | 1.00 | 0.72 |
| write at 4 KiB in 64 MiB | 64 | 1.77 |  | 0.03 |  | 1.00 | 0.73 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| delete 4 KiB, start of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.80 |  |  |  | 1.00 | 0.80 |
| insert 4 KiB, start of 32 MiB | 64 | 1.80 |  |  |  | 1.00 | 0.80 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.97 |  | 4.89 |  | 15.14 | 0.94 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.78 |  | 0.12 |  | 11.80 | 0.86 |
| append 4 KiB to 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| put 64 MiB | 64 | 29.67 |  |  |  | 28.77 | 0.91 |
| put 32 MiB | 64 | 15.08 |  | 0.02 |  | 14.14 | 0.92 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.68 |  |  |  | 1.00 | 0.68 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.92 |  |  |  | 1.18 | 0.74 |
| delete 4 KiB, start of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| write at 4 KiB in 1 MiB | 128 | 1.56 |  |  |  | 1.00 | 0.56 |
| insert 4 KiB, start of 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| put 1 MiB | 400 | 1.91 |  |  |  | 1.16 | 0.75 |
| multipart put 256 MiB × 16 MiB | 32 | 174.69 | 1.00 | 33.00 | 1.00 | 138.75 | 0.94 |
| overwrite 1 MiB | 400 | 1.91 |  |  |  | 1.18 | 0.74 |
| put 4 KiB | 400 | 1.40 |  |  |  | 1.00 | 0.40 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 1.09 |  |  |  | 1.00 | 0.09 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 1.05 |  |  |  | 1.00 | 0.05 |
| multipart put 64 MiB × 8 MiB | 32 | 61.69 | 1.00 | 17.00 | 1.00 | 42.03 | 0.66 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 1.38 |  |  |  | 1.00 | 0.38 |
