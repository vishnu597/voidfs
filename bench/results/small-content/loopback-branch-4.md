# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T030800Z` |
| When | 2026-09-30T03:08:00Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.4×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 394 | 0.92 | 427× faster | 18× faster |
| rename 64 MiB | 103 | 0.84 | 122× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 261 | 6.2 | 42× faster | 13× faster |
| list 200 keys | 36.9 | 0.98 | 38× faster | 9.1× faster |
| insert 4 KiB, start of 64 MiB | 224 | 6.5 | 34× faster | 6.8× faster |
| append 4 KiB to 64 MiB | 222 | 7.7 | 29× faster | 15× faster |
| write at 4 KiB in 64 MiB | 219 | 10.0 | 22× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 196 | 11.2 | 18× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 208 | 12.7 | 16× faster | 11× faster |
| append 4 KiB to 32 MiB | 106 | 6.8 | 16× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 111 | 7.7 | 14× faster | 5.1× faster |
| delete 4 KiB, middle of 64 MiB | 179 | 12.5 | 14× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 110 | 8.0 | 14× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 102 | 7.7 | 13× faster | 5.9× faster |
| insert 4 KiB, middle of 32 MiB | 118 | 8.9 | 13× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 113 | 8.9 | 13× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 113 | 9.3 | 12× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.7 | 0.45 | 5.9× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.6 | 0.91 | 5.0× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 5.0× faster | 34× faster |
| head | 0.75 | 0.18 | 4.2× faster | 13× faster |
| get 4 KiB | 0.85 | 0.22 | 3.8× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.73 | 3.5× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 221 | 110 | 2.0× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.3 | 3.4 | 1.9× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 109 | 71.4 | 1.5× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 335 | 237 | 1.4× faster | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.5 | 2.6 | 1.4× faster | 2.2× slower |
| multipart put 256 MiB × 16 MiB | 1,356 | 1,011 | 1.3× faster | 1.8× slower |
| get 1 MiB | 1.0 | 0.98 | 1.1× faster | 4.5× faster |
| overwrite 4 KiB | 1.2 | 1.1 | 1.0× faster | 3.1× slower |
| delete 4 KiB, middle of 1 MiB | 6.0 | 6.2 | 1.0× slower | 1.4× slower |
| stream get 64 MiB | 47.1 | 50.9 | 1.1× slower | 15× faster |
| get 32 MiB | 24.7 | 27.7 | 1.1× slower | 12× faster |
| get 64 MiB | 49.5 | 55.7 | 1.1× slower | 17× faster |
| put 64 MiB | 198 | 224 | 1.1× slower | 1.1× slower |
| stream get 256 MiB | 181 | 210 | 1.2× slower | 16× faster |
| put 4 KiB | 1.1 | 1.3 | 1.2× slower | 2.0× slower |
| insert 4 KiB, start of 1 MiB | 4.5 | 5.4 | 1.2× slower | 1.7× slower |
| put 32 MiB | 98.9 | 120 | 1.2× slower | 1.2× slower |
| truncate 4 KiB, end of 1 MiB | 5.0 | 6.3 | 1.3× slower | 2.1× slower |
| delete 4 KiB, start of 1 MiB | 4.4 | 5.6 | 1.3× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 4.0 | 5.2 | 1.3× slower | 1.6× slower |
| append 4 KiB to 1 MiB | 4.8 | 6.3 | 1.3× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 4.9 | 6.8 | 1.4× slower | 1.4× slower |
| overwrite 1 MiB | 4.4 | 7.9 | 1.8× slower | 1.9× slower |
| put 1 MiB | 3.4 | 6.4 | 1.9× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.0 | 7.4 | 1.9× slower | 2.8× slower |
| patch 16 × 4 KiB in 1 MiB | 4.4 | 23.0 | 5.3× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.58 |  |  |  | 1.00 | 0.58 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.67 |  | 0.03 |  | 1.00 | 0.64 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.78 |  | 0.03 |  | 1.00 | 0.75 |
| insert 4 KiB, start of 64 MiB | 64 | 1.75 |  | 0.03 |  | 1.00 | 0.72 |
| write at 4 KiB in 64 MiB | 64 | 1.78 |  | 0.05 |  | 1.00 | 0.73 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.81 |  |  |  | 1.00 | 0.81 |
| delete 4 KiB, start of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| insert 4 KiB, start of 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.42 |  | 4.98 |  | 15.53 | 0.91 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.86 |  |  |  | 11.92 | 0.94 |
| append 4 KiB to 1 MiB | 128 | 1.64 |  |  |  | 1.01 | 0.63 |
| put 64 MiB | 64 | 28.83 |  |  |  | 27.92 | 0.91 |
| put 32 MiB | 64 | 15.23 |  | 0.02 |  | 14.25 | 0.97 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.87 |  |  |  | 1.17 | 0.70 |
| delete 4 KiB, start of 1 MiB | 128 | 1.54 |  |  |  | 1.00 | 0.54 |
| write at 4 KiB in 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| insert 4 KiB, start of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| put 1 MiB | 400 | 1.88 |  |  |  | 1.18 | 0.69 |
| multipart put 256 MiB × 16 MiB | 32 | 171.62 | 1.00 | 33.00 | 1.00 | 135.72 | 0.91 |
| overwrite 1 MiB | 400 | 1.95 |  |  |  | 1.20 | 0.75 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.62 | 1.00 | 17.00 | 1.00 | 41.78 | 0.84 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
