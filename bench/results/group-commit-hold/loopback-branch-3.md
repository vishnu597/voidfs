# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T152236Z` |
| When | 2026-09-30T15:22:36Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.9×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 855 | 1.1 | 808× faster | 18× faster |
| rename 64 MiB | 309 | 0.96 | 321× faster | 7.9× faster |
| delete 4 KiB, start of 64 MiB | 446 | 7.3 | 61× faster | 6.9× faster |
| write at 4 KiB in 64 MiB | 270 | 5.4 | 50× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 187 | 5.9 | 32× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 218 | 7.1 | 31× faster | 6.8× faster |
| list 200 keys | 34.9 | 1.2 | 30× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 204 | 7.8 | 26× faster | 13× faster |
| append 4 KiB to 32 MiB | 107 | 4.6 | 23× faster | 8.0× faster |
| truncate 4 KiB, end of 32 MiB | 104 | 4.9 | 21× faster | 5.9× faster |
| insert 4 KiB, middle of 32 MiB | 277 | 13.4 | 21× faster | 3.8× faster |
| insert 4 KiB, middle of 64 MiB | 256 | 13.5 | 19× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 218 | 11.7 | 19× faster | 11× faster |
| write at 4 KiB in 32 MiB | 109 | 6.2 | 17× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 112 | 7.5 | 15× faster | 3.5× faster |
| delete 4 KiB, start of 32 MiB | 96.2 | 8.4 | 11× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 105 | 10.5 | 10.0× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.44 | 5.8× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.7 | 0.90 | 5.2× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.6× faster | 34× faster |
| head | 0.76 | 0.18 | 4.2× faster | 13× faster |
| get 4 KiB | 0.84 | 0.21 | 4.0× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.74 | 3.5× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 269 | 115 | 2.3× faster | 2.1× faster |
| multipart put 64 MiB × 8 MiB | 504 | 221 | 2.3× faster | 2.4× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.0 | 3.5 | 1.7× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 107 | 67.0 | 1.6× faster | 1.4× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.2 | 2.2 | 1.5× faster | 2.2× slower |
| multipart put 256 MiB × 16 MiB | 3,287 | 2,268 | 1.4× faster | 1.8× slower |
| put 64 MiB | 291 | 255 | 1.1× faster | 1.1× slower |
| get 1 MiB | 1.1 | 0.98 | 1.1× faster | 4.5× faster |
| overwrite 4 KiB | 1.0 | 1.1 | 1.1× slower | 3.1× slower |
| stream get 256 MiB | 196 | 215 | 1.1× slower | 16× faster |
| get 32 MiB | 26.0 | 28.8 | 1.1× slower | 12× faster |
| delete 4 KiB, start of 1 MiB | 4.4 | 4.8 | 1.1× slower | 1.5× slower |
| put 4 KiB | 0.85 | 0.96 | 1.1× slower | 2.0× slower |
| get 64 MiB | 48.8 | 56.1 | 1.2× slower | 17× faster |
| truncate 4 KiB, end of 1 MiB | 4.1 | 4.8 | 1.2× slower | 2.1× slower |
| stream get 64 MiB | 46.5 | 53.9 | 1.2× slower | 15× faster |
| write at 4 KiB in 1 MiB | 4.3 | 5.1 | 1.2× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 4.4 | 5.2 | 1.2× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 4.7 | 5.6 | 1.2× slower | 1.0× faster |
| put 32 MiB | 106 | 131 | 1.2× slower | 1.2× slower |
| insert 4 KiB, start of 1 MiB | 4.2 | 5.3 | 1.3× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 4.1 | 6.0 | 1.5× slower | 1.4× slower |
| overwrite 1 MiB | 3.2 | 5.8 | 1.8× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.9 | 7.4 | 1.9× slower | 2.8× slower |
| put 1 MiB | 3.0 | 5.7 | 1.9× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.3 | 18.4 | 4.2× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.62 |  |  |  | 1.00 | 0.62 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.69 |  | 0.03 |  | 1.00 | 0.66 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.62 |  |  |  | 1.00 | 0.62 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.77 |  | 0.05 |  | 1.00 | 0.72 |
| insert 4 KiB, start of 64 MiB | 64 | 1.75 |  | 0.03 |  | 1.00 | 0.72 |
| write at 4 KiB in 64 MiB | 64 | 1.75 |  | 0.05 |  | 1.00 | 0.70 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.62 |  |  |  | 1.00 | 0.62 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| delete 4 KiB, start of 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| insert 4 KiB, start of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.53 |  | 5.02 |  | 15.56 | 0.95 |
| patch 16 × 4 KiB in 32 MiB | 64 | 13.08 |  |  |  | 12.16 | 0.92 |
| append 4 KiB to 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| put 64 MiB | 64 | 29.55 |  |  |  | 28.70 | 0.84 |
| put 32 MiB | 64 | 15.22 |  |  |  | 14.47 | 0.75 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.54 |  |  |  | 1.00 | 0.54 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.95 |  |  |  | 1.17 | 0.78 |
| delete 4 KiB, start of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| write at 4 KiB in 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| insert 4 KiB, start of 1 MiB | 128 | 1.54 |  |  |  | 1.00 | 0.54 |
| put 1 MiB | 400 | 1.88 |  |  |  | 1.18 | 0.70 |
| multipart put 256 MiB × 16 MiB | 32 | 175.50 | 1.00 | 33.00 | 1.00 | 139.78 | 0.72 |
| overwrite 1 MiB | 400 | 1.92 |  |  |  | 1.19 | 0.73 |
| put 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.41 | 1.00 | 17.00 | 1.00 | 41.53 | 0.88 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
