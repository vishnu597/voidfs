# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T234703Z` |
| When | 2026-09-30T23:47:03Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.5×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 386 | 0.98 | 393× faster | 18× faster |
| rename 64 MiB | 110 | 0.81 | 136× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 231 | 5.8 | 40× faster | 15× faster |
| list 200 keys | 35.8 | 0.98 | 37× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 213 | 6.8 | 31× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 223 | 10.7 | 21× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 184 | 9.5 | 19× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 196 | 10.1 | 19× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 171 | 10.0 | 17× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 187 | 11.0 | 17× faster | 6.9× faster |
| truncate 4 KiB, end of 32 MiB | 112 | 6.7 | 17× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 105 | 6.9 | 15× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 100 | 7.3 | 14× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 107 | 10.0 | 11× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 117 | 11.4 | 10× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 110 | 11.5 | 9.6× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 102 | 13.1 | 7.8× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.43 | 5.9× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.5 | 0.86 | 5.3× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.22 | 5.1× faster | 34× faster |
| head | 0.75 | 0.18 | 4.1× faster | 13× faster |
| get 4 KiB | 0.83 | 0.21 | 4.0× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.75 | 3.6× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 181 | 88.8 | 2.0× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.3 | 3.2 | 2.0× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 98.2 | 56.8 | 1.7× faster | 1.4× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.2 | 2.0 | 1.6× faster | 2.2× slower |
| get 1 MiB | 1.2 | 0.74 | 1.6× faster | 4.5× faster |
| multipart put 256 MiB × 16 MiB | 1,035 | 733 | 1.4× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 268 | 201 | 1.3× faster | 2.4× slower |
| overwrite 4 KiB | 1.0 | 0.89 | 1.1× faster | 3.1× slower |
| stream get 256 MiB | 187 | 198 | 1.1× slower | 16× faster |
| stream get 64 MiB | 45.8 | 49.9 | 1.1× slower | 15× faster |
| delete 4 KiB, middle of 1 MiB | 4.0 | 4.5 | 1.1× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 4.1 | 4.6 | 1.1× slower | 1.5× slower |
| get 64 MiB | 46.7 | 53.1 | 1.1× slower | 17× faster |
| truncate 4 KiB, end of 1 MiB | 4.3 | 4.9 | 1.1× slower | 2.1× slower |
| put 4 KiB | 0.83 | 0.96 | 1.2× slower | 2.0× slower |
| get 32 MiB | 24.4 | 28.2 | 1.2× slower | 12× faster |
| put 64 MiB | 146 | 175 | 1.2× slower | 1.1× slower |
| insert 4 KiB, start of 1 MiB | 4.6 | 5.6 | 1.2× slower | 1.7× slower |
| write at 4 KiB in 1 MiB | 3.9 | 4.7 | 1.2× slower | 1.6× slower |
| put 32 MiB | 74.5 | 91.3 | 1.2× slower | 1.2× slower |
| append 4 KiB to 1 MiB | 4.3 | 5.3 | 1.2× slower | 1.0× faster |
| patch 16 × 4 KiB in 1 MiB | 4.2 | 5.8 | 1.4× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 4.1 | 5.5 | 1.4× slower | 1.4× slower |
| overwrite 1 MiB | 3.5 | 6.2 | 1.8× slower | 1.9× slower |
| put 1 MiB | 2.9 | 5.4 | 1.8× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.7 | 7.0 | 1.9× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.72 |  |  | 1.00 | 0.72 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.69 |  | 0.03 | 1.00 | 0.66 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.70 |  |  | 1.00 | 0.70 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.73 |  |  | 1.00 | 0.73 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.70 |  |  | 1.00 | 0.70 |
| rename 64 MiB | 64 | 0.25 |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.80 |  | 0.03 | 1.00 | 0.77 |
| insert 4 KiB, start of 64 MiB | 64 | 1.73 |  | 0.03 | 1.00 | 0.70 |
| write at 4 KiB in 64 MiB | 64 | 1.69 |  |  | 1.00 | 0.69 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.72 |  |  | 1.00 | 0.72 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.64 |  |  | 1.00 | 0.64 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.58 |  |  | 1.00 | 0.58 |
| delete 4 KiB, start of 32 MiB | 64 | 1.77 |  |  | 1.00 | 0.77 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.73 |  |  | 1.00 | 0.73 |
| insert 4 KiB, start of 32 MiB | 64 | 1.66 |  |  | 1.00 | 0.66 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.98 |  | 4.88 | 15.25 | 0.86 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.78 |  | 0.05 | 11.88 | 0.86 |
| append 4 KiB to 1 MiB | 128 | 1.59 |  |  | 1.00 | 0.59 |
| put 64 MiB | 64 | 29.23 |  |  | 28.30 | 0.94 |
| put 32 MiB | 64 | 15.45 |  |  | 14.53 | 0.92 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.60 |  |  | 1.00 | 0.60 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.57 |  |  | 1.00 | 0.57 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.61 |  |  | 1.01 | 0.60 |
| delete 4 KiB, start of 1 MiB | 128 | 1.60 |  |  | 1.00 | 0.60 |
| write at 4 KiB in 1 MiB | 128 | 1.59 |  |  | 1.00 | 0.59 |
| insert 4 KiB, start of 1 MiB | 128 | 1.60 |  |  | 1.00 | 0.60 |
| put 1 MiB | 400 | 1.95 |  |  | 1.20 | 0.75 |
| multipart put 256 MiB × 16 MiB | 32 | 172.66 | 0.97 | 33.03 | 137.84 | 0.81 |
| overwrite 1 MiB | 400 | 1.90 |  |  | 1.16 | 0.73 |
| put 4 KiB | 400 | 0.26 |  |  |  | 0.26 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.55 |  |  | 1.00 | 0.55 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 62.19 | 0.75 | 17.00 | 43.66 | 0.78 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.26 |  |  |  | 0.26 |
