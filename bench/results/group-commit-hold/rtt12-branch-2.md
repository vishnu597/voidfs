# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T151106Z` |
| When | 2026-09-30T15:11:06Z |
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
| head | 13.6 | 0.27 | 51× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.8 | 0.32 | 43× faster | 34× faster |
| list 200 keys | 52.8 | 1.4 | 38× faster | 9.1× faster |
| get 4 KiB | 13.9 | 0.37 | 38× faster | 23× faster |
| move dir 200 × 64 KiB | 471 | 15.0 | 31× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.6 | 0.45 | 28× faster | 5.3× faster |
| rename 64 MiB | 196 | 14.2 | 14× faster | 7.9× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.4 | 0.97 | 13× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.8 | 1.1 | 12× faster | 6.2× faster |
| get 1 MiB | 14.7 | 1.2 | 12× faster | 4.5× faster |
| append 4 KiB to 64 MiB | 415 | 40.7 | 10× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 369 | 39.5 | 9.3× faster | 13× faster |
| delete 4 KiB, middle of 64 MiB | 433 | 52.7 | 8.2× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 353 | 43.4 | 8.1× faster | 6.9× faster |
| insert 4 KiB, start of 64 MiB | 358 | 47.0 | 7.6× faster | 6.8× faster |
| insert 4 KiB, middle of 64 MiB | 373 | 54.5 | 6.8× faster | 11× faster |
| write at 4 KiB in 64 MiB | 319 | 49.4 | 6.5× faster | 6.2× faster |
| insert 4 KiB, start of 32 MiB | 154 | 36.1 | 4.3× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 158 | 37.7 | 4.2× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 172 | 41.2 | 4.2× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 164 | 39.5 | 4.2× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 148 | 37.0 | 4.0× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 144 | 39.0 | 3.7× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 146 | 46.6 | 3.1× faster | 3.8× faster |
| stream get 256 MiB | 754 | 287 | 2.6× faster | 16× faster |
| get 64 MiB | 131 | 53.5 | 2.5× faster | 17× faster |
| get 32 MiB | 109 | 48.6 | 2.2× faster | 12× faster |
| patch 16 × 4 KiB in 64 MiB | 299 | 154 | 1.9× faster | 2.1× faster |
| stream get 64 MiB | 225 | 117 | 1.9× faster | 15× faster |
| patch 16 × 4 KiB in 32 MiB | 156 | 126 | 1.2× faster | 1.4× faster |
| put 4 KiB | 14.4 | 15.4 | 1.1× slower | 2.0× slower |
| overwrite 4 KiB | 12.9 | 15.4 | 1.2× slower | 3.1× slower |
| put 64 MiB | 220 | 271 | 1.2× slower | 1.1× slower |
| put 32 MiB | 124 | 156 | 1.3× slower | 1.2× slower |
| delete 4 KiB, start of 1 MiB | 29.1 | 38.7 | 1.3× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 28.7 | 38.8 | 1.4× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 28.8 | 39.7 | 1.4× slower | 1.7× slower |
| truncate 4 KiB, end of 1 MiB | 28.3 | 39.1 | 1.4× slower | 2.1× slower |
| append 4 KiB to 1 MiB | 27.7 | 38.6 | 1.4× slower | 1.0× faster |
| fanout put 1000 × 4 KiB, 32 at once | 12.6 | 17.6 | 1.4× slower | 2.2× slower |
| delete 4 KiB, middle of 1 MiB | 28.8 | 40.3 | 1.4× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 27.9 | 39.2 | 1.4× slower | 1.6× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.3 | 18.2 | 1.5× slower | 2.4× slower |
| multipart put 64 MiB × 8 MiB | 294 | 438 | 1.5× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 29.4 | 46.4 | 1.6× slower | 1.5× slower |
| multipart put 256 MiB × 16 MiB | 621 | 1,090 | 1.8× slower | 1.8× slower |
| overwrite 1 MiB | 13.9 | 33.5 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.8 | 34.1 | 2.5× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.0 | 37.2 | 2.9× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.50 |  |  |  | 1.00 | 0.50 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.50 |  |  |  | 1.00 | 0.50 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| rename 64 MiB | 64 | 0.17 |  | 0.02 |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.44 |  | 0.02 |  | 1.00 | 0.42 |
| insert 4 KiB, start of 64 MiB | 64 | 1.47 |  |  |  | 1.00 | 0.47 |
| write at 4 KiB in 64 MiB | 64 | 1.45 |  | 0.03 |  | 1.00 | 0.42 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| delete 4 KiB, start of 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.53 |  |  |  | 1.02 | 0.52 |
| insert 4 KiB, start of 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.91 |  | 4.84 |  | 15.33 | 0.73 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.42 |  | 0.02 |  | 11.72 | 0.69 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.97 |  |  |  | 28.33 | 0.64 |
| put 32 MiB | 64 | 15.20 |  | 0.02 |  | 14.55 | 0.64 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.62 |  |  |  | 1.16 | 0.46 |
| delete 4 KiB, start of 1 MiB | 128 | 1.35 |  |  |  | 1.00 | 0.35 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.56 |  |  |  | 1.19 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 175.72 | 1.00 | 33.00 | 1.00 | 139.88 | 0.84 |
| overwrite 1 MiB | 400 | 1.54 |  |  |  | 1.17 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.35 |  |  |  | 1.00 | 0.35 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 61.28 | 1.00 | 17.00 | 1.00 | 41.97 | 0.31 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  |  | 0.14 |
