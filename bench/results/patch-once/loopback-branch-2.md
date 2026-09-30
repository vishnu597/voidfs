# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T191659Z` |
| When | 2026-09-30T19:16:59Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.4×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 384 | 0.97 | 394× faster | 18× faster |
| rename 64 MiB | 109 | 0.95 | 115× faster | 7.9× faster |
| list 200 keys | 35.9 | 1.0 | 36× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 218 | 6.9 | 32× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 220 | 8.7 | 25× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 229 | 11.0 | 21× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 205 | 9.9 | 21× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 204 | 10.0 | 20× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 97.4 | 5.0 | 20× faster | 5.9× faster |
| append 4 KiB to 64 MiB | 178 | 9.5 | 19× faster | 15× faster |
| append 4 KiB to 32 MiB | 105 | 7.4 | 14× faster | 8.0× faster |
| insert 4 KiB, middle of 64 MiB | 173 | 12.3 | 14× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 102 | 9.7 | 11× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 104 | 10.0 | 10× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 111 | 11.4 | 9.7× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 109 | 11.4 | 9.6× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 93.6 | 13.0 | 7.2× faster | 4.6× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.45 | 5.8× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.5 | 0.89 | 5.0× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.8× faster | 34× faster |
| head | 0.76 | 0.18 | 4.2× faster | 13× faster |
| get 4 KiB | 0.83 | 0.20 | 4.1× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.75 | 3.6× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 199 | 96.2 | 2.1× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.1 | 3.3 | 1.9× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 105 | 68.4 | 1.5× faster | 1.4× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.5 | 2.4 | 1.5× faster | 2.2× slower |
| multipart put 256 MiB × 16 MiB | 1,351 | 1,005 | 1.3× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 310 | 237 | 1.3× faster | 2.4× slower |
| get 1 MiB | 1.00 | 0.92 | 1.1× faster | 4.5× faster |
| overwrite 4 KiB | 1.2 | 1.2 | 1.0× faster | 3.1× slower |
| stream get 256 MiB | 184 | 194 | 1.1× slower | 16× faster |
| put 64 MiB | 193 | 207 | 1.1× slower | 1.1× slower |
| get 32 MiB | 23.3 | 26.0 | 1.1× slower | 12× faster |
| stream get 64 MiB | 45.6 | 51.0 | 1.1× slower | 15× faster |
| truncate 4 KiB, end of 1 MiB | 5.3 | 5.9 | 1.1× slower | 2.1× slower |
| put 32 MiB | 94.9 | 109 | 1.1× slower | 1.2× slower |
| delete 4 KiB, middle of 1 MiB | 5.5 | 6.3 | 1.2× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 5.3 | 6.2 | 1.2× slower | 1.0× faster |
| insert 4 KiB, start of 1 MiB | 4.5 | 5.4 | 1.2× slower | 1.7× slower |
| put 4 KiB | 1.0 | 1.3 | 1.2× slower | 2.0× slower |
| get 64 MiB | 41.6 | 51.3 | 1.2× slower | 17× faster |
| insert 4 KiB, middle of 1 MiB | 4.8 | 6.1 | 1.3× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.1 | 5.4 | 1.3× slower | 1.6× slower |
| patch 16 × 4 KiB in 1 MiB | 4.4 | 5.8 | 1.3× slower | 1.5× slower |
| delete 4 KiB, start of 1 MiB | 4.1 | 5.6 | 1.3× slower | 1.5× slower |
| overwrite 1 MiB | 4.2 | 7.5 | 1.8× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.2 | 7.6 | 1.8× slower | 2.8× slower |
| put 1 MiB | 3.1 | 5.7 | 1.8× slower | 1.7× slower |

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
| truncate 4 KiB, end of 64 MiB | 64 | 1.70 |  | 0.03 |  | 1.00 | 0.67 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.81 |  | 0.03 |  | 1.00 | 0.78 |
| insert 4 KiB, start of 64 MiB | 64 | 1.77 |  | 0.03 |  | 1.00 | 0.73 |
| write at 4 KiB in 64 MiB | 64 | 1.83 |  | 0.05 |  | 1.00 | 0.78 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| delete 4 KiB, start of 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| insert 4 KiB, start of 32 MiB | 64 | 1.70 |  |  |  | 1.02 | 0.69 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.92 |  | 5.33 |  | 15.64 | 0.95 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.48 |  | 0.03 |  | 10.58 | 0.88 |
| append 4 KiB to 1 MiB | 128 | 1.65 |  |  |  | 1.00 | 0.65 |
| put 64 MiB | 64 | 29.95 |  |  |  | 29.05 | 0.91 |
| put 32 MiB | 64 | 15.05 |  |  |  | 14.09 | 0.95 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.62 |  |  |  | 1.01 | 0.62 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.77 |  |  |  | 1.16 | 0.61 |
| delete 4 KiB, start of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| write at 4 KiB in 1 MiB | 128 | 1.60 |  | 0.01 |  | 1.00 | 0.59 |
| insert 4 KiB, start of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| put 1 MiB | 400 | 1.93 |  |  |  | 1.18 | 0.74 |
| multipart put 256 MiB × 16 MiB | 32 | 173.03 | 1.00 | 33.00 | 1.00 | 137.06 | 0.97 |
| overwrite 1 MiB | 400 | 1.92 |  |  |  | 1.17 | 0.75 |
| put 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 60.69 | 1.00 | 17.00 | 1.00 | 40.91 | 0.78 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
