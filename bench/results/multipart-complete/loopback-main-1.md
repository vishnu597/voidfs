# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T233741Z` |
| When | 2026-09-30T23:37:41Z |
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
| move dir 200 × 64 KiB | 384 | 0.95 | 402× faster | 18× faster |
| rename 64 MiB | 95.2 | 0.82 | 117× faster | 7.9× faster |
| list 200 keys | 36.0 | 1.0 | 35× faster | 9.1× faster |
| append 4 KiB to 64 MiB | 226 | 6.8 | 33× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 273 | 10.7 | 25× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 211 | 9.2 | 23× faster | 13× faster |
| delete 4 KiB, start of 64 MiB | 205 | 9.0 | 23× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 222 | 11.0 | 20× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 103 | 5.3 | 19× faster | 5.9× faster |
| write at 4 KiB in 64 MiB | 183 | 10.0 | 18× faster | 6.2× faster |
| delete 4 KiB, start of 32 MiB | 104 | 5.8 | 18× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 126 | 8.6 | 15× faster | 3.5× faster |
| insert 4 KiB, middle of 64 MiB | 175 | 13.0 | 13× faster | 11× faster |
| append 4 KiB to 32 MiB | 103 | 8.2 | 13× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 111 | 8.9 | 13× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 111 | 10.4 | 11× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 109 | 13.4 | 8.1× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.43 | 6.0× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.7 | 0.86 | 5.5× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.22 | 5.0× faster | 34× faster |
| get 4 KiB | 0.84 | 0.19 | 4.4× faster | 23× faster |
| head | 0.76 | 0.17 | 4.4× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.73 | 3.6× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 186 | 94.7 | 2.0× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.4 | 3.3 | 1.9× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,144 | 714 | 1.6× faster | 1.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.2 | 2.1 | 1.6× faster | 2.2× slower |
| patch 16 × 4 KiB in 32 MiB | 90.7 | 68.5 | 1.3× faster | 1.4× faster |
| get 1 MiB | 1.0 | 0.85 | 1.2× faster | 4.5× faster |
| multipart put 64 MiB × 8 MiB | 254 | 218 | 1.2× faster | 2.4× slower |
| overwrite 4 KiB | 0.95 | 0.84 | 1.1× faster | 3.1× slower |
| append 4 KiB to 1 MiB | 4.5 | 4.8 | 1.1× slower | 1.0× faster |
| get 64 MiB | 46.2 | 49.9 | 1.1× slower | 17× faster |
| truncate 4 KiB, end of 1 MiB | 4.3 | 4.8 | 1.1× slower | 2.1× slower |
| stream get 256 MiB | 183 | 203 | 1.1× slower | 16× faster |
| delete 4 KiB, start of 1 MiB | 4.4 | 4.8 | 1.1× slower | 1.5× slower |
| get 32 MiB | 23.6 | 26.4 | 1.1× slower | 12× faster |
| put 4 KiB | 0.80 | 0.90 | 1.1× slower | 2.0× slower |
| stream get 64 MiB | 44.4 | 50.7 | 1.1× slower | 15× faster |
| put 32 MiB | 80.4 | 92.7 | 1.2× slower | 1.2× slower |
| insert 4 KiB, start of 1 MiB | 4.3 | 5.0 | 1.2× slower | 1.7× slower |
| put 64 MiB | 155 | 180 | 1.2× slower | 1.1× slower |
| delete 4 KiB, middle of 1 MiB | 4.3 | 5.0 | 1.2× slower | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 4.4 | 5.5 | 1.3× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.1 | 5.3 | 1.3× slower | 1.6× slower |
| patch 16 × 4 KiB in 1 MiB | 4.0 | 5.4 | 1.4× slower | 1.5× slower |
| overwrite 1 MiB | 3.4 | 5.6 | 1.6× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.5 | 6.5 | 1.9× slower | 2.8× slower |
| put 1 MiB | 2.9 | 5.5 | 1.9× slower | 1.7× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.73 |  | 0.03 |  | 1.00 | 0.70 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| insert 4 KiB, start of 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| write at 4 KiB in 64 MiB | 64 | 1.84 |  | 0.03 |  | 1.00 | 0.81 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.81 |  |  |  | 1.00 | 0.81 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| delete 4 KiB, start of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| insert 4 KiB, start of 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.55 |  | 5.36 |  | 15.34 | 0.84 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.98 |  | 0.05 |  | 11.16 | 0.78 |
| append 4 KiB to 1 MiB | 128 | 1.61 |  |  |  | 1.01 | 0.60 |
| put 64 MiB | 64 | 28.97 |  |  |  | 28.00 | 0.97 |
| put 32 MiB | 64 | 15.27 |  |  |  | 14.33 | 0.94 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.59 |  |  |  | 1.01 | 0.58 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.77 |  |  |  | 1.15 | 0.62 |
| delete 4 KiB, start of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| write at 4 KiB in 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| insert 4 KiB, start of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| put 1 MiB | 400 | 1.87 |  |  |  | 1.18 | 0.69 |
| multipart put 256 MiB × 16 MiB | 32 | 174.41 | 1.00 | 33.25 | 1.00 | 138.19 | 0.97 |
| overwrite 1 MiB | 400 | 1.92 |  |  |  | 1.16 | 0.76 |
| put 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 62.34 | 1.00 | 17.00 | 1.00 | 42.59 | 0.75 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
