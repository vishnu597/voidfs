# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T150828Z` |
| When | 2026-09-30T15:08:28Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | bc851c2 |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.9×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.3 | 0.26 | 51× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.8 | 0.29 | 44× faster | 34× faster |
| get 4 KiB | 14.1 | 0.41 | 35× faster | 23× faster |
| list 200 keys | 37.6 | 1.2 | 31× faster | 9.1× faster |
| move dir 200 × 64 KiB | 472 | 15.8 | 30× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 13.0 | 0.47 | 28× faster | 5.3× faster |
| get 1 MiB | 13.8 | 1.0 | 13× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 14.4 | 1.1 | 13× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.3 | 0.96 | 13× faster | 2.6× faster |
| rename 64 MiB | 124 | 13.9 | 8.9× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 290 | 36.4 | 8.0× faster | 13× faster |
| append 4 KiB to 64 MiB | 294 | 37.2 | 7.9× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 291 | 37.6 | 7.7× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 312 | 42.7 | 7.3× faster | 11× faster |
| write at 4 KiB in 64 MiB | 289 | 40.6 | 7.1× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 295 | 43.5 | 6.8× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 264 | 41.6 | 6.3× faster | 11× faster |
| write at 4 KiB in 32 MiB | 157 | 36.2 | 4.3× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 148 | 35.9 | 4.1× faster | 8.0× faster |
| insert 4 KiB, middle of 32 MiB | 155 | 39.2 | 3.9× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 150 | 38.6 | 3.9× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 137 | 35.3 | 3.9× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 148 | 38.3 | 3.9× faster | 5.9× faster |
| delete 4 KiB, middle of 32 MiB | 146 | 42.9 | 3.4× faster | 4.8× faster |
| stream get 256 MiB | 612 | 214 | 2.9× faster | 16× faster |
| stream get 64 MiB | 141 | 53.5 | 2.6× faster | 15× faster |
| get 32 MiB | 69.6 | 26.7 | 2.6× faster | 12× faster |
| get 64 MiB | 128 | 54.3 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 293 | 148 | 2.0× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 137 | 109 | 1.3× faster | 1.4× faster |
| put 4 KiB | 13.8 | 15.7 | 1.1× slower | 2.0× slower |
| overwrite 4 KiB | 13.5 | 15.7 | 1.2× slower | 3.1× slower |
| multipart put 256 MiB × 16 MiB | 876 | 1,055 | 1.2× slower | 1.8× slower |
| append 4 KiB to 1 MiB | 29.9 | 37.9 | 1.3× slower | 1.0× faster |
| put 32 MiB | 92.7 | 119 | 1.3× slower | 1.2× slower |
| put 64 MiB | 169 | 220 | 1.3× slower | 1.1× slower |
| insert 4 KiB, start of 1 MiB | 28.8 | 37.6 | 1.3× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 27.5 | 36.6 | 1.3× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 28.5 | 38.1 | 1.3× slower | 2.1× slower |
| delete 4 KiB, start of 1 MiB | 29.1 | 39.6 | 1.4× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 28.5 | 38.8 | 1.4× slower | 1.6× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.3 | 17.0 | 1.4× slower | 2.2× slower |
| insert 4 KiB, middle of 1 MiB | 28.1 | 39.2 | 1.4× slower | 1.4× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.5 | 17.6 | 1.4× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 28.1 | 42.3 | 1.5× slower | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 266 | 430 | 1.6× slower | 2.4× slower |
| overwrite 1 MiB | 14.1 | 33.6 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.9 | 33.7 | 2.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.7 | 38.8 | 2.8× slower | 2.8× slower |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | delete_prefix | get | list | put | put_new |
|---|--:|--:|--:|--:|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.00 |  |  |  |  |  |
| get 4 KiB | 400 | 0.00 |  |  |  |  |  |
| move dir 200 × 64 KiB | 32 | 0.19 |  |  |  |  | 0.19 |
| get 64 MiB | 64 | 0.00 |  |  |  |  |  |
| stream get 256 MiB | 32 | 0.00 |  |  |  |  |  |
| stream get 64 MiB | 64 | 0.00 |  |  |  |  |  |
| append 4 KiB to 64 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.38 |  | 0.02 |  | 1.00 | 0.36 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.39 |  |  |  | 1.02 | 0.38 |
| rename 64 MiB | 64 | 0.16 |  |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.47 |  | 0.03 |  | 1.00 | 0.44 |
| insert 4 KiB, start of 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| write at 4 KiB in 64 MiB | 64 | 1.41 |  | 0.02 |  | 1.00 | 0.39 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| delete 4 KiB, start of 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.47 |  |  |  | 1.00 | 0.47 |
| insert 4 KiB, start of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.08 |  | 5.28 |  | 15.19 | 0.61 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.36 |  | 0.19 |  | 11.45 | 0.72 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 29.25 |  |  |  | 28.55 | 0.70 |
| put 32 MiB | 64 | 15.30 |  |  |  | 14.70 | 0.59 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.86 |  |  |  | 1.43 | 0.43 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.54 |  |  |  | 1.17 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 173.78 | 1.00 | 33.03 | 1.00 | 137.91 | 0.84 |
| overwrite 1 MiB | 400 | 1.55 |  |  |  | 1.18 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 62.16 | 1.00 | 17.00 | 1.00 | 42.78 | 0.38 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  |  | 0.14 |
