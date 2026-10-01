# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T234415Z` |
| When | 2026-09-30T23:44:15Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.6×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 384 | 0.90 | 429× faster | 18× faster |
| rename 64 MiB | 101 | 0.77 | 131× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 178 | 4.4 | 40× faster | 15× faster |
| list 200 keys | 35.7 | 1.0 | 36× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 246 | 7.8 | 32× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 263 | 11.3 | 23× faster | 6.8× faster |
| write at 4 KiB in 32 MiB | 115 | 5.1 | 22× faster | 5.1× faster |
| delete 4 KiB, middle of 64 MiB | 220 | 10.4 | 21× faster | 11× faster |
| append 4 KiB to 32 MiB | 113 | 5.7 | 20× faster | 8.0× faster |
| write at 4 KiB in 64 MiB | 177 | 9.1 | 19× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 209 | 10.8 | 19× faster | 6.9× faster |
| insert 4 KiB, start of 32 MiB | 99.2 | 5.9 | 17× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 104 | 7.9 | 13× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 109 | 8.5 | 13× faster | 4.6× faster |
| insert 4 KiB, middle of 64 MiB | 194 | 16.7 | 12× faster | 11× faster |
| insert 4 KiB, middle of 32 MiB | 114 | 10.8 | 11× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 95.7 | 12.4 | 7.7× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.43 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.7 | 0.90 | 5.2× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.8× faster | 34× faster |
| head | 0.76 | 0.18 | 4.2× faster | 13× faster |
| get 4 KiB | 0.84 | 0.21 | 4.0× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.78 | 3.2× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 197 | 97.2 | 2.0× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.2 | 3.1 | 2.0× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 93.6 | 63.2 | 1.5× faster | 1.4× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.1 | 2.2 | 1.5× faster | 2.2× slower |
| multipart put 256 MiB × 16 MiB | 1,138 | 831 | 1.4× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 273 | 218 | 1.3× faster | 2.4× slower |
| get 1 MiB | 1.0 | 0.85 | 1.2× faster | 4.5× faster |
| overwrite 4 KiB | 1.1 | 1.0 | 1.1× faster | 3.1× slower |
| delete 4 KiB, start of 1 MiB | 4.3 | 4.4 | 1.0× slower | 1.5× slower |
| put 32 MiB | 82.5 | 90.9 | 1.1× slower | 1.2× slower |
| stream get 64 MiB | 45.5 | 50.7 | 1.1× slower | 15× faster |
| stream get 256 MiB | 184 | 205 | 1.1× slower | 16× faster |
| append 4 KiB to 1 MiB | 4.2 | 4.9 | 1.2× slower | 1.0× faster |
| put 64 MiB | 152 | 176 | 1.2× slower | 1.1× slower |
| delete 4 KiB, middle of 1 MiB | 4.3 | 5.0 | 1.2× slower | 1.4× slower |
| get 64 MiB | 46.9 | 54.7 | 1.2× slower | 17× faster |
| insert 4 KiB, start of 1 MiB | 4.2 | 5.0 | 1.2× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 4.2 | 5.0 | 1.2× slower | 1.4× slower |
| get 32 MiB | 23.3 | 27.9 | 1.2× slower | 12× faster |
| write at 4 KiB in 1 MiB | 4.2 | 5.2 | 1.2× slower | 1.6× slower |
| put 4 KiB | 0.96 | 1.2 | 1.2× slower | 2.0× slower |
| patch 16 × 4 KiB in 1 MiB | 4.1 | 5.2 | 1.3× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 4.2 | 5.4 | 1.3× slower | 2.1× slower |
| overwrite 1 MiB | 3.9 | 6.9 | 1.8× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.7 | 7.0 | 1.9× slower | 2.8× slower |
| put 1 MiB | 3.0 | 5.6 | 1.9× slower | 1.7× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.66 |  | 0.03 |  | 1.00 | 0.62 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.77 |  |  |  | 1.02 | 0.75 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.81 |  |  |  | 1.02 | 0.80 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.77 |  | 0.02 |  | 1.00 | 0.75 |
| insert 4 KiB, start of 64 MiB | 64 | 1.77 |  | 0.02 |  | 1.00 | 0.75 |
| write at 4 KiB in 64 MiB | 64 | 1.62 |  |  |  | 1.00 | 0.62 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| delete 4 KiB, start of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.78 |  |  |  | 1.02 | 0.77 |
| insert 4 KiB, start of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 22.16 |  | 5.84 |  | 15.44 | 0.88 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.91 |  | 0.08 |  | 11.98 | 0.84 |
| append 4 KiB to 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| put 64 MiB | 64 | 29.31 |  |  |  | 28.38 | 0.94 |
| put 32 MiB | 64 | 15.55 |  |  |  | 14.56 | 0.98 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.56 |  |  |  | 1.00 | 0.56 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.76 |  |  |  | 1.15 | 0.61 |
| delete 4 KiB, start of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| write at 4 KiB in 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| insert 4 KiB, start of 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| put 1 MiB | 400 | 1.85 |  |  |  | 1.17 | 0.68 |
| multipart put 256 MiB × 16 MiB | 32 | 174.75 | 1.00 | 33.00 | 1.00 | 138.91 | 0.84 |
| overwrite 1 MiB | 400 | 1.89 |  |  |  | 1.17 | 0.72 |
| put 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.47 | 1.00 | 17.00 | 1.00 | 41.59 | 0.88 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
