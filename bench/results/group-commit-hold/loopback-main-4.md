# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T152635Z` |
| When | 2026-09-30T15:26:35Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.3×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 389 | 1.0 | 372× faster | 18× faster |
| truncate 4 KiB, end of 64 MiB | 246 | 6.6 | 37× faster | 13× faster |
| list 200 keys | 39.7 | 1.1 | 36× faster | 9.1× faster |
| write at 4 KiB in 64 MiB | 258 | 7.2 | 36× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 211 | 6.3 | 33× faster | 15× faster |
| rename 64 MiB | 104 | 3.2 | 33× faster | 7.9× faster |
| insert 4 KiB, start of 64 MiB | 220 | 7.6 | 29× faster | 6.8× faster |
| insert 4 KiB, start of 32 MiB | 231 | 10.9 | 21× faster | 3.5× faster |
| delete 4 KiB, middle of 64 MiB | 214 | 11.5 | 19× faster | 11× faster |
| append 4 KiB to 32 MiB | 97.0 | 5.5 | 18× faster | 8.0× faster |
| truncate 4 KiB, end of 32 MiB | 104 | 7.0 | 15× faster | 5.9× faster |
| insert 4 KiB, middle of 64 MiB | 215 | 14.5 | 15× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 178 | 12.3 | 15× faster | 6.9× faster |
| insert 4 KiB, middle of 32 MiB | 116 | 9.7 | 12× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 94.7 | 10.5 | 9.1× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 107 | 11.9 | 9.0× faster | 4.8× faster |
| write at 4 KiB in 32 MiB | 91.7 | 10.5 | 8.7× faster | 5.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.45 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.6 | 0.89 | 5.2× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.25 | 4.4× faster | 34× faster |
| head | 0.80 | 0.20 | 4.0× faster | 13× faster |
| get 4 KiB | 0.86 | 0.23 | 3.8× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.79 | 3.3× faster | 6.2× faster |
| multipart put 64 MiB × 8 MiB | 459 | 229 | 2.0× faster | 2.4× slower |
| patch 16 × 4 KiB in 64 MiB | 179 | 103 | 1.7× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.2 | 4.0 | 1.6× faster | 2.4× slower |
| get 1 MiB | 1.2 | 0.81 | 1.5× faster | 4.5× faster |
| multipart put 256 MiB × 16 MiB | 2,343 | 1,693 | 1.4× faster | 1.8× slower |
| patch 16 × 4 KiB in 32 MiB | 91.8 | 77.2 | 1.2× faster | 1.4× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.3 | 2.8 | 1.2× faster | 2.2× slower |
| overwrite 4 KiB | 1.6 | 1.5 | 1.1× faster | 3.1× slower |
| delete 4 KiB, start of 1 MiB | 4.5 | 4.9 | 1.1× slower | 1.5× slower |
| stream get 256 MiB | 200 | 222 | 1.1× slower | 16× faster |
| put 4 KiB | 1.3 | 1.4 | 1.1× slower | 2.0× slower |
| get 64 MiB | 48.0 | 55.8 | 1.2× slower | 17× faster |
| stream get 64 MiB | 49.1 | 58.3 | 1.2× slower | 15× faster |
| get 32 MiB | 25.8 | 30.6 | 1.2× slower | 12× faster |
| append 4 KiB to 1 MiB | 4.2 | 5.0 | 1.2× slower | 1.0× faster |
| insert 4 KiB, start of 1 MiB | 4.4 | 5.4 | 1.2× slower | 1.7× slower |
| put 32 MiB | 82.9 | 105 | 1.3× slower | 1.2× slower |
| put 64 MiB | 156 | 199 | 1.3× slower | 1.1× slower |
| truncate 4 KiB, end of 1 MiB | 4.2 | 5.4 | 1.3× slower | 2.1× slower |
| insert 4 KiB, middle of 1 MiB | 5.2 | 6.8 | 1.3× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 5.1 | 7.1 | 1.4× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.1 | 5.8 | 1.4× slower | 1.6× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.7 | 6.5 | 1.7× slower | 2.8× slower |
| overwrite 1 MiB | 3.2 | 6.5 | 2.0× slower | 1.9× slower |
| put 1 MiB | 2.9 | 5.9 | 2.0× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 5.0 | 19.7 | 4.0× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.67 |  |  |  | 1.02 | 0.66 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| insert 4 KiB, start of 64 MiB | 64 | 1.78 |  | 0.02 |  | 1.00 | 0.77 |
| write at 4 KiB in 64 MiB | 64 | 1.77 |  | 0.03 |  | 1.00 | 0.73 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| delete 4 KiB, start of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| insert 4 KiB, start of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.53 |  | 4.27 |  | 15.30 | 0.97 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.89 |  | 0.08 |  | 11.97 | 0.84 |
| append 4 KiB to 1 MiB | 128 | 1.56 |  |  |  | 1.00 | 0.56 |
| put 64 MiB | 64 | 28.97 |  |  |  | 28.06 | 0.91 |
| put 32 MiB | 64 | 15.42 |  |  |  | 14.53 | 0.89 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.54 |  |  |  | 1.00 | 0.54 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.91 |  |  |  | 1.16 | 0.76 |
| delete 4 KiB, start of 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| write at 4 KiB in 1 MiB | 128 | 1.48 |  |  |  | 1.00 | 0.48 |
| insert 4 KiB, start of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| put 1 MiB | 400 | 1.82 |  |  |  | 1.15 | 0.68 |
| multipart put 256 MiB × 16 MiB | 32 | 173.44 | 1.00 | 33.09 | 1.00 | 137.50 | 0.84 |
| overwrite 1 MiB | 400 | 1.79 |  |  |  | 1.20 | 0.60 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.52 |  |  |  | 1.00 | 0.52 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.56 | 1.00 | 17.00 | 1.00 | 41.66 | 0.91 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
