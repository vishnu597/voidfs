# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T233857Z` |
| When | 2026-09-30T23:38:57Z |
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

voidfs is faster in **32 of 49** scenarios and slower in the other **17**. Geometric mean speed-up over the bare bucket: **3.5×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 364 | 1.0 | 361× faster | 18× faster |
| rename 64 MiB | 107 | 0.86 | 125× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 221 | 6.0 | 37× faster | 15× faster |
| list 200 keys | 35.9 | 1.00 | 36× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 213 | 6.4 | 34× faster | 13× faster |
| write at 4 KiB in 64 MiB | 184 | 8.2 | 23× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 186 | 8.6 | 22× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 194 | 9.6 | 20× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 196 | 10.2 | 19× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 96.3 | 5.6 | 17× faster | 5.9× faster |
| insert 4 KiB, middle of 64 MiB | 219 | 13.3 | 16× faster | 11× faster |
| append 4 KiB to 32 MiB | 123 | 8.5 | 15× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 115 | 8.3 | 14× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 110 | 8.7 | 13× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 92.2 | 7.9 | 12× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 110 | 10.0 | 11× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 106 | 11.2 | 9.4× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.43 | 5.9× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.2 | 0.86 | 4.9× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.7× faster | 34× faster |
| get 4 KiB | 0.85 | 0.20 | 4.2× faster | 23× faster |
| head | 0.75 | 0.19 | 4.0× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.78 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 188 | 98.3 | 1.9× faster | 2.1× faster |
| get 1 MiB | 1.1 | 0.64 | 1.7× faster | 4.5× faster |
| fanout put 1000 × 4 KiB, 64 at once | 5.2 | 3.4 | 1.5× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 104 | 69.3 | 1.5× faster | 1.4× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.0 | 2.1 | 1.5× faster | 2.2× slower |
| multipart put 256 MiB × 16 MiB | 1,050 | 753 | 1.4× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 257 | 207 | 1.2× faster | 2.4× slower |
| overwrite 4 KiB | 0.97 | 0.87 | 1.1× faster | 3.1× slower |
| delete 4 KiB, start of 1 MiB | 4.8 | 4.7 | 1.0× faster | 1.5× slower |
| put 4 KiB | 0.82 | 0.87 | 1.1× slower | 2.0× slower |
| delete 4 KiB, middle of 1 MiB | 4.5 | 4.9 | 1.1× slower | 1.4× slower |
| stream get 256 MiB | 190 | 206 | 1.1× slower | 16× faster |
| truncate 4 KiB, end of 1 MiB | 4.3 | 4.8 | 1.1× slower | 2.1× slower |
| stream get 64 MiB | 47.7 | 54.3 | 1.1× slower | 15× faster |
| get 64 MiB | 45.6 | 52.5 | 1.2× slower | 17× faster |
| put 32 MiB | 78.8 | 93.7 | 1.2× slower | 1.2× slower |
| put 64 MiB | 144 | 175 | 1.2× slower | 1.1× slower |
| insert 4 KiB, start of 1 MiB | 4.4 | 5.4 | 1.2× slower | 1.7× slower |
| get 32 MiB | 22.7 | 28.1 | 1.2× slower | 12× faster |
| write at 4 KiB in 1 MiB | 4.1 | 5.2 | 1.3× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 4.3 | 5.5 | 1.3× slower | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | 3.9 | 5.1 | 1.3× slower | 1.5× slower |
| append 4 KiB to 1 MiB | 4.0 | 5.4 | 1.3× slower | 1.0× faster |
| overwrite 1 MiB | 3.1 | 5.5 | 1.8× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.6 | 7.0 | 1.9× slower | 2.8× slower |
| put 1 MiB | 2.9 | 5.7 | 2.0× slower | 1.7× slower |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | delete_prefix | get | put | put_new |
|---|--:|--:|--:|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.00 |  |  |  |  |
| get 4 KiB | 400 | 0.00 |  |  |  |  |
| move dir 200 × 64 KiB | 32 | 0.25 |  |  |  | 0.25 |
| get 64 MiB | 64 | 0.00 |  |  |  |  |
| stream get 256 MiB | 32 | 0.00 |  |  |  |  |
| stream get 64 MiB | 64 | 0.00 |  |  |  |  |
| append 4 KiB to 64 MiB | 64 | 1.64 |  |  | 1.02 | 0.62 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.61 |  |  | 1.00 | 0.61 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.78 |  |  | 1.00 | 0.78 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.73 |  |  | 1.00 | 0.73 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.62 |  |  | 1.00 | 0.62 |
| rename 64 MiB | 64 | 0.25 |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.72 |  | 0.02 | 1.00 | 0.70 |
| insert 4 KiB, start of 64 MiB | 64 | 1.70 |  |  | 1.00 | 0.70 |
| write at 4 KiB in 64 MiB | 64 | 1.78 |  | 0.05 | 1.00 | 0.73 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.62 |  |  | 1.00 | 0.62 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.69 |  |  | 1.00 | 0.69 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.70 |  |  | 1.00 | 0.70 |
| delete 4 KiB, start of 32 MiB | 64 | 1.73 |  |  | 1.00 | 0.73 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.73 |  |  | 1.00 | 0.73 |
| insert 4 KiB, start of 32 MiB | 64 | 1.67 |  |  | 1.00 | 0.67 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.16 |  | 4.66 | 15.67 | 0.83 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.09 |  | 0.08 | 10.17 | 0.84 |
| append 4 KiB to 1 MiB | 128 | 1.61 |  |  | 1.00 | 0.61 |
| put 64 MiB | 64 | 29.53 |  |  | 28.53 | 1.00 |
| put 32 MiB | 64 | 15.50 |  |  | 14.55 | 0.95 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.63 |  |  | 1.00 | 0.63 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.59 |  |  | 1.00 | 0.59 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.91 |  |  | 1.30 | 0.60 |
| delete 4 KiB, start of 1 MiB | 128 | 1.65 |  |  | 1.00 | 0.65 |
| write at 4 KiB in 1 MiB | 128 | 1.55 |  |  | 1.00 | 0.55 |
| insert 4 KiB, start of 1 MiB | 128 | 1.53 |  |  | 1.00 | 0.53 |
| put 1 MiB | 400 | 1.84 |  |  | 1.17 | 0.68 |
| multipart put 256 MiB × 16 MiB | 32 | 172.56 | 0.81 | 33.03 | 138.03 | 0.69 |
| overwrite 1 MiB | 400 | 1.99 |  |  | 1.19 | 0.80 |
| put 4 KiB | 400 | 0.26 |  |  |  | 0.26 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.55 |  |  | 1.00 | 0.55 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 60.00 | 0.78 | 17.00 | 41.41 | 0.81 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 0.26 |  |  |  | 0.26 |
