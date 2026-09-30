# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T030053Z` |
| When | 2026-09-30T03:00:53Z |
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

voidfs is faster in **32 of 49** scenarios and slower in the other **17**. Geometric mean speed-up over the bare bucket: **3.4×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 390 | 1.2 | 337× faster | 18× faster |
| rename 64 MiB | 116 | 0.73 | 158× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 195 | 4.7 | 41× faster | 15× faster |
| list 200 keys | 35.5 | 1.1 | 34× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 221 | 7.0 | 32× faster | 13× faster |
| write at 4 KiB in 64 MiB | 245 | 7.8 | 31× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 221 | 8.3 | 27× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 187 | 9.2 | 20× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 193 | 11.3 | 17× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 114 | 7.9 | 14× faster | 3.5× faster |
| delete 4 KiB, middle of 64 MiB | 206 | 15.5 | 13× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 94.7 | 7.4 | 13× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 114 | 9.0 | 13× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 119 | 10.2 | 12× faster | 4.6× faster |
| write at 4 KiB in 32 MiB | 108 | 10.3 | 10× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 108 | 12.3 | 8.7× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 101 | 12.6 | 8.0× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.45 | 5.5× faster | 5.3× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.22 | 5.1× faster | 34× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.4 | 0.90 | 4.9× faster | 2.6× faster |
| head | 0.77 | 0.19 | 4.0× faster | 13× faster |
| get 4 KiB | 0.83 | 0.22 | 3.8× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.78 | 3.2× faster | 6.2× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.6 | 3.4 | 1.9× faster | 2.4× slower |
| patch 16 × 4 KiB in 64 MiB | 178 | 110 | 1.6× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.5 | 2.4 | 1.5× faster | 2.2× slower |
| multipart put 256 MiB × 16 MiB | 1,347 | 931 | 1.4× faster | 1.8× slower |
| get 1 MiB | 1.0 | 0.78 | 1.3× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 93.3 | 78.0 | 1.2× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 297 | 276 | 1.1× faster | 2.4× slower |
| overwrite 4 KiB | 1.3 | 1.2 | 1.0× faster | 3.1× slower |
| truncate 4 KiB, end of 1 MiB | 4.9 | 4.9 | 1.0× faster | 2.1× slower |
| get 64 MiB | 51.6 | 53.9 | 1.0× slower | 17× faster |
| put 64 MiB | 180 | 204 | 1.1× slower | 1.1× slower |
| stream get 256 MiB | 186 | 211 | 1.1× slower | 16× faster |
| put 32 MiB | 91.9 | 106 | 1.2× slower | 1.2× slower |
| stream get 64 MiB | 45.6 | 52.8 | 1.2× slower | 15× faster |
| get 32 MiB | 24.1 | 28.0 | 1.2× slower | 12× faster |
| put 4 KiB | 1.1 | 1.3 | 1.2× slower | 2.0× slower |
| delete 4 KiB, middle of 1 MiB | 5.5 | 6.6 | 1.2× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.2 | 5.2 | 1.2× slower | 1.6× slower |
| insert 4 KiB, start of 1 MiB | 4.3 | 5.4 | 1.3× slower | 1.7× slower |
| delete 4 KiB, start of 1 MiB | 4.3 | 5.5 | 1.3× slower | 1.5× slower |
| append 4 KiB to 1 MiB | 4.2 | 5.5 | 1.3× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 5.2 | 7.1 | 1.4× slower | 1.4× slower |
| put 1 MiB | 4.1 | 6.1 | 1.5× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.2 | 7.4 | 1.8× slower | 2.8× slower |
| overwrite 1 MiB | 4.1 | 7.8 | 1.9× slower | 1.9× slower |
| patch 16 × 4 KiB in 1 MiB | 4.4 | 22.7 | 5.1× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.69 |  | 0.03 |  | 1.00 | 0.66 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.80 |  |  |  | 1.00 | 0.80 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.75 |  | 0.02 |  | 1.00 | 0.73 |
| insert 4 KiB, start of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| write at 4 KiB in 64 MiB | 64 | 1.70 |  | 0.02 |  | 1.00 | 0.69 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| delete 4 KiB, start of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| insert 4 KiB, start of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.80 |  | 4.55 |  | 15.31 | 0.94 |
| patch 16 × 4 KiB in 32 MiB | 64 | 13.12 |  | 0.06 |  | 12.14 | 0.92 |
| append 4 KiB to 1 MiB | 128 | 1.56 |  |  |  | 1.00 | 0.56 |
| put 64 MiB | 64 | 29.39 |  |  |  | 28.44 | 0.95 |
| put 32 MiB | 64 | 15.44 |  |  |  | 14.47 | 0.97 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.63 |  |  |  | 1.01 | 0.62 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.88 |  |  |  | 1.15 | 0.73 |
| delete 4 KiB, start of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| write at 4 KiB in 1 MiB | 128 | 1.56 |  |  |  | 1.00 | 0.56 |
| insert 4 KiB, start of 1 MiB | 128 | 1.62 |  | 0.01 |  | 1.00 | 0.62 |
| put 1 MiB | 400 | 1.81 |  |  |  | 1.19 | 0.62 |
| multipart put 256 MiB × 16 MiB | 32 | 174.91 | 1.00 | 33.31 | 1.00 | 138.66 | 0.94 |
| overwrite 1 MiB | 400 | 1.91 |  |  |  | 1.23 | 0.68 |
| put 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 60.72 | 1.00 | 17.00 | 1.00 | 40.84 | 0.88 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.14 |  |  |  | 1.00 | 0.14 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
