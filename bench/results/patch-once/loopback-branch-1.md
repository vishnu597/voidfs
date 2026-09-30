# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T191542Z` |
| When | 2026-09-30T19:15:42Z |
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

voidfs is faster in **32 of 49** scenarios and slower in the other **17**. Geometric mean speed-up over the bare bucket: **3.6×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 392 | 1.00 | 393× faster | 18× faster |
| rename 64 MiB | 105 | 0.75 | 139× faster | 7.9× faster |
| write at 4 KiB in 64 MiB | 240 | 5.8 | 42× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 218 | 6.1 | 35× faster | 6.8× faster |
| list 200 keys | 35.3 | 1.1 | 32× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 202 | 6.4 | 32× faster | 13× faster |
| append 4 KiB to 64 MiB | 192 | 7.5 | 26× faster | 15× faster |
| append 4 KiB to 32 MiB | 117 | 4.8 | 24× faster | 8.0× faster |
| delete 4 KiB, start of 64 MiB | 222 | 10.7 | 21× faster | 6.9× faster |
| truncate 4 KiB, end of 32 MiB | 111 | 5.9 | 19× faster | 5.9× faster |
| delete 4 KiB, middle of 64 MiB | 216 | 12.5 | 17× faster | 11× faster |
| write at 4 KiB in 32 MiB | 104 | 7.5 | 14× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 111 | 9.5 | 12× faster | 3.8× faster |
| insert 4 KiB, middle of 64 MiB | 175 | 15.2 | 12× faster | 11× faster |
| delete 4 KiB, start of 32 MiB | 117 | 10.9 | 11× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 94.6 | 9.7 | 9.8× faster | 3.5× faster |
| delete 4 KiB, middle of 32 MiB | 94.3 | 11.3 | 8.3× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.43 | 6.1× faster | 5.3× faster |
| range 64 KiB of 64 MiB | 1.2 | 0.22 | 5.2× faster | 34× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.5 | 0.87 | 5.1× faster | 2.6× faster |
| head | 0.75 | 0.20 | 3.8× faster | 13× faster |
| get 4 KiB | 0.83 | 0.22 | 3.8× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.76 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 194 | 94.3 | 2.1× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.1 | 3.3 | 1.9× faster | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.3 | 2.1 | 1.6× faster | 2.2× slower |
| patch 16 × 4 KiB in 32 MiB | 86.3 | 62.8 | 1.4× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,078 | 793 | 1.4× faster | 1.8× slower |
| get 1 MiB | 1.1 | 0.82 | 1.3× faster | 4.5× faster |
| overwrite 4 KiB | 0.97 | 0.86 | 1.1× faster | 3.1× slower |
| multipart put 64 MiB × 8 MiB | 245 | 221 | 1.1× faster | 2.4× slower |
| append 4 KiB to 1 MiB | 4.2 | 4.1 | 1.0× faster | 1.0× faster |
| truncate 4 KiB, end of 1 MiB | 4.2 | 4.4 | 1.0× slower | 2.1× slower |
| stream get 64 MiB | 47.7 | 51.1 | 1.1× slower | 15× faster |
| stream get 256 MiB | 182 | 203 | 1.1× slower | 16× faster |
| insert 4 KiB, middle of 1 MiB | 4.6 | 5.3 | 1.1× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 4.3 | 4.9 | 1.1× slower | 1.5× slower |
| put 4 KiB | 0.80 | 0.91 | 1.1× slower | 2.0× slower |
| delete 4 KiB, middle of 1 MiB | 4.2 | 4.8 | 1.2× slower | 1.4× slower |
| get 32 MiB | 23.5 | 27.7 | 1.2× slower | 12× faster |
| insert 4 KiB, start of 1 MiB | 4.5 | 5.4 | 1.2× slower | 1.7× slower |
| get 64 MiB | 48.6 | 59.3 | 1.2× slower | 17× faster |
| put 64 MiB | 148 | 184 | 1.2× slower | 1.1× slower |
| put 32 MiB | 75.5 | 94.5 | 1.3× slower | 1.2× slower |
| write at 4 KiB in 1 MiB | 3.9 | 5.1 | 1.3× slower | 1.6× slower |
| patch 16 × 4 KiB in 1 MiB | 4.0 | 5.2 | 1.3× slower | 1.5× slower |
| overwrite 1 MiB | 3.3 | 5.7 | 1.7× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.6 | 6.6 | 1.9× slower | 2.8× slower |
| put 1 MiB | 2.9 | 5.7 | 1.9× slower | 1.7× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.67 |  | 0.03 |  | 1.00 | 0.64 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.84 |  | 0.05 |  | 1.00 | 0.80 |
| insert 4 KiB, start of 64 MiB | 64 | 1.70 |  |  |  | 1.02 | 0.69 |
| write at 4 KiB in 64 MiB | 64 | 1.62 |  |  |  | 1.00 | 0.62 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.72 |  |  |  | 1.02 | 0.70 |
| delete 4 KiB, start of 32 MiB | 64 | 1.72 |  |  |  | 1.02 | 0.70 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.84 |  |  |  | 1.02 | 0.83 |
| insert 4 KiB, start of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.31 |  | 4.47 |  | 14.91 | 0.94 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.69 |  | 0.03 |  | 10.81 | 0.84 |
| append 4 KiB to 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| put 64 MiB | 64 | 29.02 |  |  |  | 28.06 | 0.95 |
| put 32 MiB | 64 | 15.23 |  |  |  | 14.27 | 0.97 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.56 |  |  |  | 1.00 | 0.56 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.92 |  |  |  | 1.32 | 0.60 |
| delete 4 KiB, start of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| write at 4 KiB in 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| insert 4 KiB, start of 1 MiB | 128 | 1.64 |  |  |  | 1.00 | 0.64 |
| put 1 MiB | 400 | 1.84 |  |  |  | 1.16 | 0.68 |
| multipart put 256 MiB × 16 MiB | 32 | 173.34 | 1.00 | 33.00 | 1.00 | 137.53 | 0.81 |
| overwrite 1 MiB | 400 | 1.87 |  |  |  | 1.17 | 0.70 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 62.06 | 1.00 | 17.00 | 1.00 | 42.19 | 0.88 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
