# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T025818Z` |
| When | 2026-09-30T02:58:18Z |
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

voidfs is faster in **28 of 49** scenarios and slower in the other **21**. Geometric mean speed-up over the bare bucket: **3.2×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 364 | 0.93 | 391× faster | 18× faster |
| rename 64 MiB | 115 | 0.75 | 153× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 203 | 4.7 | 44× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 195 | 4.7 | 41× faster | 13× faster |
| list 200 keys | 35.4 | 1.1 | 33× faster | 9.1× faster |
| insert 4 KiB, start of 64 MiB | 223 | 7.9 | 28× faster | 6.8× faster |
| append 4 KiB to 32 MiB | 115 | 4.6 | 25× faster | 8.0× faster |
| truncate 4 KiB, end of 32 MiB | 88.5 | 4.0 | 22× faster | 5.9× faster |
| write at 4 KiB in 64 MiB | 196 | 10.2 | 19× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 186 | 10.0 | 19× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 224 | 15.5 | 14× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 212 | 16.2 | 13× faster | 11× faster |
| write at 4 KiB in 32 MiB | 101 | 7.9 | 13× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 109 | 11.1 | 9.8× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 111 | 11.8 | 9.4× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 100 | 11.0 | 9.1× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 109 | 12.2 | 9.0× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.46 | 5.5× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.4 | 0.89 | 5.0× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.7× faster | 34× faster |
| get 4 KiB | 0.83 | 0.21 | 3.9× faster | 23× faster |
| head | 0.77 | 0.20 | 3.8× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.82 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 185 | 100.0 | 1.8× faster | 2.1× faster |
| multipart put 256 MiB × 16 MiB | 1,127 | 783 | 1.4× faster | 1.8× slower |
| get 1 MiB | 1.1 | 0.78 | 1.4× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 97.2 | 82.7 | 1.2× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 261 | 227 | 1.1× faster | 2.4× slower |
| get 64 MiB | 49.1 | 52.6 | 1.1× slower | 17× faster |
| stream get 64 MiB | 46.3 | 50.1 | 1.1× slower | 15× faster |
| stream get 256 MiB | 189 | 207 | 1.1× slower | 16× faster |
| write at 4 KiB in 1 MiB | 4.3 | 5.1 | 1.2× slower | 1.6× slower |
| put 32 MiB | 81.3 | 96.6 | 1.2× slower | 1.2× slower |
| get 32 MiB | 24.2 | 28.8 | 1.2× slower | 12× faster |
| truncate 4 KiB, end of 1 MiB | 4.0 | 4.8 | 1.2× slower | 2.1× slower |
| insert 4 KiB, middle of 1 MiB | 4.3 | 5.2 | 1.2× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 4.1 | 5.0 | 1.2× slower | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 4.4 | 5.5 | 1.2× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 4.1 | 5.1 | 1.2× slower | 1.5× slower |
| put 64 MiB | 152 | 191 | 1.3× slower | 1.1× slower |
| insert 4 KiB, start of 1 MiB | 4.1 | 5.4 | 1.3× slower | 1.7× slower |
| overwrite 4 KiB | 0.97 | 1.5 | 1.5× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.0 | 5.0 | 1.7× slower | 2.2× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.4 | 6.2 | 1.8× slower | 2.8× slower |
| put 4 KiB | 0.82 | 1.5 | 1.9× slower | 2.0× slower |
| put 1 MiB | 2.9 | 5.5 | 1.9× slower | 1.7× slower |
| overwrite 1 MiB | 3.1 | 5.9 | 1.9× slower | 1.9× slower |
| fanout put 1000 × 4 KiB, 64 at once | 4.7 | 8.8 | 1.9× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 4.0 | 17.7 | 4.4× slower | 1.5× slower |

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
| truncate 4 KiB, end of 64 MiB | 64 | 1.69 |  | 0.02 |  | 1.00 | 0.67 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| insert 4 KiB, start of 64 MiB | 64 | 1.80 |  | 0.03 |  | 1.00 | 0.77 |
| write at 4 KiB in 64 MiB | 64 | 1.78 |  | 0.05 |  | 1.00 | 0.73 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| delete 4 KiB, start of 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.80 |  |  |  | 1.00 | 0.80 |
| insert 4 KiB, start of 32 MiB | 64 | 1.62 |  |  |  | 1.00 | 0.62 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 22.31 |  | 5.75 |  | 15.66 | 0.91 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.69 |  | 0.09 |  | 11.70 | 0.89 |
| append 4 KiB to 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| put 64 MiB | 64 | 29.36 |  |  |  | 28.44 | 0.92 |
| put 32 MiB | 64 | 15.50 |  |  |  | 14.55 | 0.95 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| patch 16 × 4 KiB in 1 MiB | 128 | 2.09 |  |  |  | 1.32 | 0.77 |
| delete 4 KiB, start of 1 MiB | 128 | 1.57 |  |  |  | 1.01 | 0.56 |
| write at 4 KiB in 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| insert 4 KiB, start of 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| put 1 MiB | 400 | 1.91 |  |  |  | 1.17 | 0.74 |
| multipart put 256 MiB × 16 MiB | 32 | 175.00 | 1.00 | 33.06 | 1.00 | 138.97 | 0.97 |
| overwrite 1 MiB | 400 | 1.82 |  |  |  | 1.17 | 0.66 |
| put 4 KiB | 400 | 1.39 |  |  |  | 1.00 | 0.39 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.56 |  |  |  | 1.00 | 0.56 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 1.09 |  |  |  | 1.00 | 0.09 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 1.05 |  |  |  | 1.00 | 0.05 |
| multipart put 64 MiB × 8 MiB | 32 | 62.66 | 1.00 | 17.00 | 1.00 | 42.91 | 0.75 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 1.39 |  |  |  | 1.00 | 0.39 |
