# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T234135Z` |
| When | 2026-09-30T23:41:35Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.4×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 385 | 1.00 | 387× faster | 18× faster |
| rename 64 MiB | 123 | 0.84 | 148× faster | 7.9× faster |
| list 200 keys | 37.0 | 1.0 | 35× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 225 | 7.0 | 32× faster | 13× faster |
| append 4 KiB to 64 MiB | 220 | 7.7 | 28× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 216 | 8.4 | 26× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 208 | 11.0 | 19× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 209 | 11.5 | 18× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 222 | 12.3 | 18× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 201 | 11.3 | 18× faster | 6.9× faster |
| append 4 KiB to 32 MiB | 104 | 6.7 | 16× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 101 | 7.0 | 15× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 108 | 9.1 | 12× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 106 | 9.8 | 11× faster | 3.8× faster |
| write at 4 KiB in 32 MiB | 106 | 10.9 | 9.8× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 111 | 11.9 | 9.3× faster | 4.8× faster |
| truncate 4 KiB, end of 32 MiB | 93.1 | 11.5 | 8.1× faster | 5.9× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.7 | 0.42 | 6.4× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.3 | 0.85 | 5.1× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.7× faster | 34× faster |
| head | 0.76 | 0.19 | 4.0× faster | 13× faster |
| get 4 KiB | 0.82 | 0.21 | 3.9× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.78 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 207 | 96.4 | 2.1× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 4.6 | 3.3 | 1.4× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 97.6 | 68.7 | 1.4× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,269 | 920 | 1.4× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 302 | 227 | 1.3× faster | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.0 | 2.2 | 1.3× faster | 2.2× slower |
| get 1 MiB | 1.0 | 0.93 | 1.1× faster | 4.5× faster |
| overwrite 4 KiB | 1.2 | 1.1 | 1.0× faster | 3.1× slower |
| write at 4 KiB in 1 MiB | 4.1 | 4.2 | 1.0× slower | 1.6× slower |
| get 32 MiB | 24.3 | 26.6 | 1.1× slower | 12× faster |
| put 32 MiB | 93.0 | 104 | 1.1× slower | 1.2× slower |
| stream get 64 MiB | 44.0 | 49.6 | 1.1× slower | 15× faster |
| insert 4 KiB, start of 1 MiB | 4.8 | 5.4 | 1.1× slower | 1.7× slower |
| stream get 256 MiB | 179 | 204 | 1.1× slower | 16× faster |
| put 64 MiB | 182 | 209 | 1.2× slower | 1.1× slower |
| delete 4 KiB, start of 1 MiB | 4.4 | 5.1 | 1.2× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 4.8 | 5.7 | 1.2× slower | 2.1× slower |
| append 4 KiB to 1 MiB | 4.5 | 5.6 | 1.2× slower | 1.0× faster |
| put 4 KiB | 1.0 | 1.3 | 1.3× slower | 2.0× slower |
| get 64 MiB | 42.7 | 55.0 | 1.3× slower | 17× faster |
| insert 4 KiB, middle of 1 MiB | 4.8 | 6.3 | 1.3× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 5.3 | 7.0 | 1.3× slower | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | 4.5 | 6.0 | 1.4× slower | 1.5× slower |
| overwrite 1 MiB | 4.5 | 7.3 | 1.6× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.1 | 7.3 | 1.8× slower | 2.8× slower |
| put 1 MiB | 3.2 | 5.9 | 1.9× slower | 1.7× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.75 |  |  |  | 1.02 | 0.73 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.75 |  | 0.02 |  | 1.00 | 0.73 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.61 |  |  |  | 1.00 | 0.61 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.80 |  | 0.05 |  | 1.00 | 0.75 |
| insert 4 KiB, start of 64 MiB | 64 | 1.66 |  | 0.02 |  | 1.00 | 0.64 |
| write at 4 KiB in 64 MiB | 64 | 1.80 |  |  |  | 1.00 | 0.80 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| delete 4 KiB, start of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.86 |  |  |  | 1.00 | 0.86 |
| insert 4 KiB, start of 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.12 |  | 4.58 |  | 15.64 | 0.91 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.56 |  |  |  | 11.80 | 0.77 |
| append 4 KiB to 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| put 64 MiB | 64 | 28.81 |  |  |  | 27.91 | 0.91 |
| put 32 MiB | 64 | 15.45 |  |  |  | 14.50 | 0.95 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.58 |  |  |  | 1.01 | 0.57 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.59 |  | 0.01 |  | 1.01 | 0.58 |
| delete 4 KiB, start of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| write at 4 KiB in 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| insert 4 KiB, start of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| put 1 MiB | 400 | 1.81 |  |  |  | 1.19 | 0.62 |
| multipart put 256 MiB × 16 MiB | 32 | 174.84 | 1.00 | 33.00 | 1.00 | 138.88 | 0.97 |
| overwrite 1 MiB | 400 | 2.02 |  |  |  | 1.22 | 0.80 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.34 | 1.00 | 17.00 | 1.00 | 41.59 | 0.75 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
