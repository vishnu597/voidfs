# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T151644Z` |
| When | 2026-09-30T15:16:44Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.4×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 403 | 1.0 | 400× faster | 18× faster |
| rename 64 MiB | 96.7 | 0.93 | 104× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 235 | 5.1 | 46× faster | 13× faster |
| append 4 KiB to 64 MiB | 265 | 6.9 | 39× faster | 15× faster |
| list 200 keys | 37.9 | 1.1 | 34× faster | 9.1× faster |
| delete 4 KiB, start of 64 MiB | 210 | 8.2 | 26× faster | 6.9× faster |
| truncate 4 KiB, end of 32 MiB | 111 | 4.9 | 23× faster | 5.9× faster |
| insert 4 KiB, start of 64 MiB | 206 | 10.3 | 20× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 200 | 10.8 | 19× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 228 | 12.4 | 18× faster | 11× faster |
| delete 4 KiB, start of 32 MiB | 112 | 6.2 | 18× faster | 4.6× faster |
| insert 4 KiB, middle of 64 MiB | 219 | 12.9 | 17× faster | 11× faster |
| append 4 KiB to 32 MiB | 108 | 6.7 | 16× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 116 | 8.3 | 14× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 104 | 12.3 | 8.5× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 112 | 13.3 | 8.4× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 113 | 13.4 | 8.4× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.43 | 6.0× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.9 | 0.95 | 5.1× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.2 | 0.26 | 4.8× faster | 34× faster |
| head | 0.86 | 0.20 | 4.4× faster | 13× faster |
| get 4 KiB | 0.97 | 0.23 | 4.2× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.86 | 3.0× faster | 6.2× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.4 | 3.8 | 1.7× faster | 2.4× slower |
| patch 16 × 4 KiB in 64 MiB | 189 | 113 | 1.7× faster | 2.1× faster |
| multipart put 256 MiB × 16 MiB | 1,299 | 908 | 1.4× faster | 1.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.4 | 2.5 | 1.4× faster | 2.2× slower |
| patch 16 × 4 KiB in 32 MiB | 97.9 | 78.0 | 1.3× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 288 | 230 | 1.3× faster | 2.4× slower |
| get 1 MiB | 1.2 | 0.96 | 1.2× faster | 4.5× faster |
| overwrite 4 KiB | 1.1 | 1.0 | 1.1× faster | 3.1× slower |
| truncate 4 KiB, end of 1 MiB | 4.5 | 4.6 | 1.0× slower | 2.1× slower |
| get 32 MiB | 25.5 | 26.6 | 1.0× slower | 12× faster |
| stream get 256 MiB | 189 | 205 | 1.1× slower | 16× faster |
| delete 4 KiB, start of 1 MiB | 4.5 | 4.9 | 1.1× slower | 1.5× slower |
| stream get 64 MiB | 45.8 | 52.6 | 1.1× slower | 15× faster |
| get 64 MiB | 47.9 | 55.6 | 1.2× slower | 17× faster |
| insert 4 KiB, start of 1 MiB | 4.5 | 5.3 | 1.2× slower | 1.7× slower |
| put 32 MiB | 83.3 | 102 | 1.2× slower | 1.2× slower |
| put 4 KiB | 0.93 | 1.2 | 1.3× slower | 2.0× slower |
| write at 4 KiB in 1 MiB | 4.0 | 5.0 | 1.3× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 4.5 | 5.7 | 1.3× slower | 1.4× slower |
| put 64 MiB | 163 | 212 | 1.3× slower | 1.1× slower |
| append 4 KiB to 1 MiB | 4.5 | 5.9 | 1.3× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 4.3 | 6.2 | 1.4× slower | 1.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.8 | 6.8 | 1.8× slower | 2.8× slower |
| overwrite 1 MiB | 3.5 | 6.3 | 1.8× slower | 1.9× slower |
| put 1 MiB | 3.3 | 6.4 | 2.0× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.3 | 18.2 | 4.2× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.81 |  |  |  | 1.00 | 0.81 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.83 |  | 0.05 |  | 1.00 | 0.78 |
| insert 4 KiB, start of 64 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| write at 4 KiB in 64 MiB | 64 | 1.75 |  | 0.05 |  | 1.00 | 0.70 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.83 |  |  |  | 1.00 | 0.83 |
| delete 4 KiB, start of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.69 |  |  |  | 1.02 | 0.67 |
| insert 4 KiB, start of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.42 |  | 5.25 |  | 15.34 | 0.83 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.41 |  | 0.06 |  | 11.42 | 0.92 |
| append 4 KiB to 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| put 64 MiB | 64 | 28.81 |  |  |  | 27.92 | 0.89 |
| put 32 MiB | 64 | 15.86 |  |  |  | 14.92 | 0.94 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.56 |  |  |  | 1.00 | 0.56 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.51 |  |  |  | 1.00 | 0.51 |
| patch 16 × 4 KiB in 1 MiB | 128 | 2.02 |  |  |  | 1.30 | 0.71 |
| delete 4 KiB, start of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| write at 4 KiB in 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| insert 4 KiB, start of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| put 1 MiB | 400 | 1.83 |  |  |  | 1.16 | 0.67 |
| multipart put 256 MiB × 16 MiB | 32 | 173.50 | 1.00 | 33.00 | 1.00 | 137.53 | 0.97 |
| overwrite 1 MiB | 400 | 1.93 |  |  |  | 1.21 | 0.72 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.50 | 1.00 | 17.00 | 1.00 | 41.75 | 0.75 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
