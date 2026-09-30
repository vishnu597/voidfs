# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T151407Z` |
| When | 2026-09-30T15:14:07Z |
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

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.7×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.7 | 0.28 | 49× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.4 | 0.31 | 43× faster | 34× faster |
| list 200 keys | 39.4 | 1.0 | 39× faster | 9.1× faster |
| get 4 KiB | 13.8 | 0.38 | 36× faster | 23× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.8 | 0.52 | 25× faster | 5.3× faster |
| move dir 200 × 64 KiB | 468 | 26.7 | 18× faster | 18× faster |
| get 1 MiB | 14.1 | 0.99 | 14× faster | 4.5× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 0.95 | 13× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.7 | 1.1 | 13× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 300 | 36.3 | 8.3× faster | 13× faster |
| delete 4 KiB, middle of 64 MiB | 316 | 38.5 | 8.2× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 277 | 34.8 | 7.9× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 284 | 36.5 | 7.8× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 300 | 39.0 | 7.7× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 290 | 40.4 | 7.2× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 274 | 42.8 | 6.4× faster | 11× faster |
| rename 64 MiB | 125 | 26.6 | 4.7× faster | 7.9× faster |
| append 4 KiB to 32 MiB | 140 | 37.3 | 3.8× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 145 | 39.1 | 3.7× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 151 | 41.0 | 3.7× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 150 | 40.7 | 3.7× faster | 3.8× faster |
| write at 4 KiB in 32 MiB | 148 | 41.9 | 3.5× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 143 | 41.0 | 3.5× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 133 | 38.6 | 3.5× faster | 5.9× faster |
| stream get 256 MiB | 571 | 203 | 2.8× faster | 16× faster |
| stream get 64 MiB | 134 | 50.1 | 2.7× faster | 15× faster |
| get 32 MiB | 67.2 | 25.4 | 2.6× faster | 12× faster |
| get 64 MiB | 122 | 52.4 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 285 | 127 | 2.2× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 141 | 108 | 1.3× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,003 | 1,161 | 1.2× slower | 1.8× slower |
| put 64 MiB | 169 | 213 | 1.3× slower | 1.1× slower |
| put 32 MiB | 90.3 | 113 | 1.3× slower | 1.2× slower |
| append 4 KiB to 1 MiB | 28.0 | 37.5 | 1.3× slower | 1.0× faster |
| insert 4 KiB, start of 1 MiB | 28.8 | 38.7 | 1.3× slower | 1.7× slower |
| delete 4 KiB, start of 1 MiB | 27.8 | 37.5 | 1.4× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 28.4 | 38.4 | 1.4× slower | 2.1× slower |
| multipart put 64 MiB × 8 MiB | 306 | 420 | 1.4× slower | 2.4× slower |
| insert 4 KiB, middle of 1 MiB | 27.1 | 37.2 | 1.4× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 27.9 | 38.9 | 1.4× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 28.2 | 39.6 | 1.4× slower | 1.6× slower |
| patch 16 × 4 KiB in 1 MiB | 29.3 | 45.1 | 1.5× slower | 1.5× slower |
| put 4 KiB | 14.5 | 28.5 | 2.0× slower | 2.0× slower |
| overwrite 4 KiB | 13.5 | 28.4 | 2.1× slower | 3.1× slower |
| overwrite 1 MiB | 14.5 | 33.5 | 2.3× slower | 1.9× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.6 | 30.2 | 2.4× slower | 2.4× slower |
| put 1 MiB | 14.0 | 33.7 | 2.4× slower | 1.7× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 30.4 | 2.5× slower | 2.2× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.9 | 37.8 | 2.9× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.33 |  |  |  | 1.00 | 0.33 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.47 |  | 0.02 |  | 1.00 | 0.45 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.25 |  |  |  | 1.00 | 0.25 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| insert 4 KiB, start of 64 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| write at 4 KiB in 64 MiB | 64 | 1.42 |  | 0.02 |  | 1.00 | 0.41 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.48 |  |  |  | 1.02 | 0.47 |
| delete 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.45 |  |  |  | 1.00 | 0.45 |
| insert 4 KiB, start of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.98 |  | 5.75 |  | 15.45 | 0.78 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.83 |  | 0.09 |  | 12.06 | 0.67 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 29.14 |  |  |  | 28.50 | 0.64 |
| put 32 MiB | 64 | 15.06 |  |  |  | 14.45 | 0.61 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.65 |  |  |  | 1.16 | 0.48 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.54 |  |  |  | 1.17 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 175.75 | 1.00 | 33.03 | 1.00 | 140.03 | 0.69 |
| overwrite 1 MiB | 400 | 1.56 |  |  |  | 1.19 | 0.37 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 60.81 | 1.00 | 17.00 | 1.00 | 41.34 | 0.47 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
