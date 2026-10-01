# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T234256Z` |
| When | 2026-09-30T23:42:56Z |
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
| move dir 200 × 64 KiB | 384 | 0.96 | 400× faster | 18× faster |
| rename 64 MiB | 105 | 0.76 | 139× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 206 | 4.5 | 46× faster | 13× faster |
| list 200 keys | 35.8 | 0.98 | 36× faster | 9.1× faster |
| write at 4 KiB in 64 MiB | 239 | 7.2 | 33× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 172 | 6.5 | 26× faster | 15× faster |
| append 4 KiB to 32 MiB | 112 | 4.3 | 26× faster | 8.0× faster |
| delete 4 KiB, start of 64 MiB | 191 | 8.5 | 22× faster | 6.9× faster |
| truncate 4 KiB, end of 32 MiB | 112 | 5.2 | 22× faster | 5.9× faster |
| insert 4 KiB, start of 64 MiB | 194 | 11.4 | 17× faster | 6.8× faster |
| insert 4 KiB, start of 32 MiB | 107 | 6.4 | 17× faster | 3.5× faster |
| delete 4 KiB, start of 32 MiB | 110 | 7.0 | 16× faster | 4.6× faster |
| delete 4 KiB, middle of 64 MiB | 185 | 12.4 | 15× faster | 11× faster |
| write at 4 KiB in 32 MiB | 93.8 | 6.6 | 14× faster | 5.1× faster |
| insert 4 KiB, middle of 64 MiB | 192 | 16.1 | 12× faster | 11× faster |
| insert 4 KiB, middle of 32 MiB | 108 | 12.1 | 8.9× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 103 | 13.5 | 7.6× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.43 | 5.9× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.6 | 0.86 | 5.3× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.7× faster | 34× faster |
| head | 0.78 | 0.19 | 4.1× faster | 13× faster |
| get 4 KiB | 0.83 | 0.21 | 3.9× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.4 | 0.77 | 3.0× faster | 6.2× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.2 | 3.2 | 1.9× faster | 2.4× slower |
| patch 16 × 4 KiB in 64 MiB | 184 | 102 | 1.8× faster | 2.1× faster |
| multipart put 64 MiB × 8 MiB | 289 | 200 | 1.4× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 95.3 | 68.2 | 1.4× faster | 1.4× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.0 | 2.2 | 1.4× faster | 2.2× slower |
| multipart put 256 MiB × 16 MiB | 1,200 | 896 | 1.3× faster | 1.8× slower |
| get 1 MiB | 1.0 | 0.90 | 1.2× faster | 4.5× faster |
| overwrite 4 KiB | 1.1 | 1.0 | 1.1× faster | 3.1× slower |
| truncate 4 KiB, end of 1 MiB | 4.6 | 5.0 | 1.1× slower | 2.1× slower |
| stream get 256 MiB | 188 | 207 | 1.1× slower | 16× faster |
| put 64 MiB | 183 | 204 | 1.1× slower | 1.1× slower |
| stream get 64 MiB | 45.5 | 51.3 | 1.1× slower | 15× faster |
| put 32 MiB | 90.7 | 106 | 1.2× slower | 1.2× slower |
| get 32 MiB | 23.4 | 27.4 | 1.2× slower | 12× faster |
| get 64 MiB | 43.3 | 51.5 | 1.2× slower | 17× faster |
| put 4 KiB | 0.97 | 1.2 | 1.2× slower | 2.0× slower |
| delete 4 KiB, start of 1 MiB | 4.4 | 5.5 | 1.2× slower | 1.5× slower |
| patch 16 × 4 KiB in 1 MiB | 4.4 | 5.6 | 1.3× slower | 1.5× slower |
| append 4 KiB to 1 MiB | 4.5 | 5.7 | 1.3× slower | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 4.8 | 6.2 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 4.2 | 5.6 | 1.3× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 4.4 | 6.0 | 1.3× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.5 | 6.4 | 1.4× slower | 1.6× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.0 | 6.8 | 1.7× slower | 2.8× slower |
| put 1 MiB | 3.2 | 5.7 | 1.8× slower | 1.7× slower |
| overwrite 1 MiB | 4.0 | 7.2 | 1.8× slower | 1.9× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.75 |  |  | 1.02 | 0.73 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.75 |  |  | 1.00 | 0.75 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.67 |  |  | 1.00 | 0.67 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.72 |  |  | 1.00 | 0.72 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.72 |  |  | 1.02 | 0.70 |
| rename 64 MiB | 64 | 0.25 |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.77 |  |  | 1.00 | 0.77 |
| insert 4 KiB, start of 64 MiB | 64 | 1.77 |  | 0.03 | 1.02 | 0.72 |
| write at 4 KiB in 64 MiB | 64 | 1.80 |  | 0.05 | 1.00 | 0.75 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.67 |  |  | 1.00 | 0.67 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.69 |  |  | 1.00 | 0.69 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.77 |  |  | 1.00 | 0.77 |
| delete 4 KiB, start of 32 MiB | 64 | 1.64 |  |  | 1.00 | 0.64 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.78 |  |  | 1.00 | 0.78 |
| insert 4 KiB, start of 32 MiB | 64 | 1.81 |  |  | 1.00 | 0.81 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.81 |  | 4.56 | 15.28 | 0.97 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.80 |  |  | 11.00 | 0.80 |
| append 4 KiB to 1 MiB | 128 | 1.59 |  |  | 1.00 | 0.59 |
| put 64 MiB | 64 | 29.22 |  |  | 28.30 | 0.92 |
| put 32 MiB | 64 | 15.25 |  |  | 14.34 | 0.91 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.58 |  |  | 1.00 | 0.58 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.55 |  |  | 1.00 | 0.55 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.77 |  |  | 1.16 | 0.61 |
| delete 4 KiB, start of 1 MiB | 128 | 1.55 |  |  | 1.00 | 0.55 |
| write at 4 KiB in 1 MiB | 128 | 1.58 |  |  | 1.00 | 0.58 |
| insert 4 KiB, start of 1 MiB | 128 | 1.56 |  |  | 1.01 | 0.55 |
| put 1 MiB | 400 | 1.88 |  |  | 1.20 | 0.68 |
| multipart put 256 MiB × 16 MiB | 32 | 173.16 | 0.91 | 33.00 | 138.47 | 0.78 |
| overwrite 1 MiB | 400 | 1.89 |  |  | 1.20 | 0.69 |
| put 4 KiB | 400 | 0.25 |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.66 |  |  | 1.00 | 0.66 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 59.81 | 0.94 | 17.00 | 41.09 | 0.78 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  | 0.25 |
