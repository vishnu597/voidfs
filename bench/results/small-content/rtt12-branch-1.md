# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T024947Z` |
| When | 2026-09-30T02:49:47Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 8ce1a94 |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.8×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.8 | 0.27 | 48× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.0 | 0.28 | 43× faster | 34× faster |
| get 4 KiB | 12.5 | 0.31 | 40× faster | 23× faster |
| list 200 keys | 43.9 | 1.3 | 35× faster | 9.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.1 | 0.54 | 22× faster | 5.3× faster |
| move dir 200 × 64 KiB | 489 | 24.0 | 20× faster | 18× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.1 | 1.0 | 13× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.3 | 1.0 | 12× faster | 2.6× faster |
| append 4 KiB to 64 MiB | 435 | 37.2 | 12× faster | 15× faster |
| get 1 MiB | 13.2 | 1.2 | 11× faster | 4.5× faster |
| write at 4 KiB in 64 MiB | 335 | 39.0 | 8.6× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 399 | 46.4 | 8.6× faster | 11× faster |
| truncate 4 KiB, end of 64 MiB | 352 | 41.7 | 8.4× faster | 13× faster |
| delete 4 KiB, start of 64 MiB | 338 | 40.1 | 8.4× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 384 | 46.3 | 8.3× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 352 | 46.5 | 7.6× faster | 6.8× faster |
| rename 64 MiB | 161 | 25.0 | 6.4× faster | 7.9× faster |
| append 4 KiB to 32 MiB | 175 | 35.2 | 5.0× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 157 | 35.7 | 4.4× faster | 4.6× faster |
| write at 4 KiB in 32 MiB | 158 | 36.3 | 4.4× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 146 | 36.6 | 4.0× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 139 | 35.9 | 3.9× faster | 5.9× faster |
| delete 4 KiB, middle of 32 MiB | 148 | 40.8 | 3.6× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 133 | 38.3 | 3.5× faster | 3.8× faster |
| stream get 256 MiB | 960 | 345 | 2.8× faster | 16× faster |
| stream get 64 MiB | 206 | 84.7 | 2.4× faster | 15× faster |
| get 64 MiB | 160 | 68.0 | 2.4× faster | 17× faster |
| get 32 MiB | 100 | 43.3 | 2.3× faster | 12× faster |
| patch 16 × 4 KiB in 64 MiB | 292 | 131 | 2.2× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 137 | 99.7 | 1.4× faster | 1.4× faster |
| put 32 MiB | 113 | 135 | 1.2× slower | 1.2× slower |
| multipart put 256 MiB × 16 MiB | 883 | 1,056 | 1.2× slower | 1.8× slower |
| insert 4 KiB, middle of 1 MiB | 27.7 | 34.6 | 1.2× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 27.2 | 34.7 | 1.3× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 27.3 | 35.3 | 1.3× slower | 1.0× faster |
| write at 4 KiB in 1 MiB | 27.3 | 35.8 | 1.3× slower | 1.6× slower |
| insert 4 KiB, start of 1 MiB | 27.2 | 37.4 | 1.4× slower | 1.7× slower |
| delete 4 KiB, start of 1 MiB | 26.1 | 36.3 | 1.4× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 26.2 | 36.9 | 1.4× slower | 2.1× slower |
| put 64 MiB | 172 | 246 | 1.4× slower | 1.1× slower |
| patch 16 × 4 KiB in 1 MiB | 27.5 | 42.9 | 1.6× slower | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 291 | 501 | 1.7× slower | 2.4× slower |
| overwrite 4 KiB | 12.4 | 26.2 | 2.1× slower | 3.1× slower |
| put 4 KiB | 12.8 | 27.2 | 2.1× slower | 2.0× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.2 | 27.3 | 2.2× slower | 2.2× slower |
| overwrite 1 MiB | 14.0 | 32.6 | 2.3× slower | 1.9× slower |
| fanout put 1000 × 4 KiB, 64 at once | 11.8 | 27.9 | 2.4× slower | 2.4× slower |
| put 1 MiB | 13.5 | 33.2 | 2.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.3 | 36.9 | 2.8× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.39 |  |  |  | 1.02 | 0.38 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.47 |  |  |  | 1.00 | 0.47 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.50 |  |  |  | 1.02 | 0.48 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| rename 64 MiB | 64 | 0.27 |  | 0.02 |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.48 |  | 0.05 |  | 1.00 | 0.44 |
| insert 4 KiB, start of 64 MiB | 64 | 1.52 |  | 0.03 |  | 1.00 | 0.48 |
| write at 4 KiB in 64 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.33 |  |  |  | 1.00 | 0.33 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.47 |  |  |  | 1.00 | 0.47 |
| delete 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| insert 4 KiB, start of 32 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.39 |  | 5.08 |  | 15.62 | 0.69 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.38 |  | 0.12 |  | 11.61 | 0.64 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.84 |  |  |  | 28.30 | 0.55 |
| put 32 MiB | 64 | 15.23 |  | 0.02 |  | 14.53 | 0.69 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 2.09 |  |  |  | 1.65 | 0.44 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.56 |  |  |  | 1.19 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 175.41 | 1.00 | 33.00 | 1.00 | 139.59 | 0.81 |
| overwrite 1 MiB | 400 | 1.56 |  |  |  | 1.19 | 0.37 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.84 | 1.00 | 17.00 | 1.00 | 42.19 | 0.66 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
