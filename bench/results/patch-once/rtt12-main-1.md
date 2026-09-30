# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T190356Z` |
| When | 2026-09-30T19:03:56Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 10feda4 |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.8×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.2 | 0.28 | 48× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.3 | 0.30 | 44× faster | 34× faster |
| list 200 keys | 40.4 | 1.1 | 38× faster | 9.1× faster |
| get 4 KiB | 13.9 | 0.40 | 34× faster | 23× faster |
| move dir 200 × 64 KiB | 449 | 16.3 | 28× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.4 | 0.55 | 23× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 0.88 | 14× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.7 | 1.1 | 13× faster | 6.2× faster |
| get 1 MiB | 14.3 | 1.3 | 11× faster | 4.5× faster |
| rename 64 MiB | 122 | 13.8 | 8.9× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 306 | 37.6 | 8.1× faster | 15× faster |
| write at 4 KiB in 64 MiB | 293 | 38.4 | 7.6× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 291 | 38.5 | 7.6× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 287 | 39.7 | 7.2× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 286 | 40.5 | 7.0× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 280 | 41.1 | 6.8× faster | 11× faster |
| truncate 4 KiB, end of 64 MiB | 272 | 41.0 | 6.6× faster | 13× faster |
| insert 4 KiB, start of 32 MiB | 153 | 38.4 | 4.0× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 162 | 42.1 | 3.8× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 139 | 37.4 | 3.7× faster | 5.9× faster |
| delete 4 KiB, middle of 32 MiB | 148 | 40.4 | 3.7× faster | 4.8× faster |
| write at 4 KiB in 32 MiB | 151 | 41.4 | 3.7× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 139 | 39.4 | 3.5× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 120 | 38.6 | 3.1× faster | 8.0× faster |
| stream get 256 MiB | 603 | 208 | 2.9× faster | 16× faster |
| stream get 64 MiB | 137 | 51.6 | 2.6× faster | 15× faster |
| get 32 MiB | 66.5 | 26.9 | 2.5× faster | 12× faster |
| get 64 MiB | 122 | 53.8 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 248 | 141 | 1.8× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 136 | 116 | 1.2× faster | 1.4× faster |
| put 4 KiB | 13.9 | 16.0 | 1.1× slower | 2.0× slower |
| overwrite 4 KiB | 13.4 | 15.5 | 1.2× slower | 3.1× slower |
| put 32 MiB | 89.0 | 116 | 1.3× slower | 1.2× slower |
| insert 4 KiB, start of 1 MiB | 29.5 | 38.5 | 1.3× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 28.1 | 36.9 | 1.3× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 27.7 | 36.6 | 1.3× slower | 1.0× faster |
| put 64 MiB | 162 | 217 | 1.3× slower | 1.1× slower |
| write at 4 KiB in 1 MiB | 28.0 | 38.6 | 1.4× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 27.5 | 37.9 | 1.4× slower | 1.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.2 | 17.1 | 1.4× slower | 2.2× slower |
| truncate 4 KiB, end of 1 MiB | 27.0 | 38.6 | 1.4× slower | 2.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.3 | 17.6 | 1.4× slower | 2.4× slower |
| delete 4 KiB, start of 1 MiB | 28.5 | 40.9 | 1.4× slower | 1.5× slower |
| patch 16 × 4 KiB in 1 MiB | 28.5 | 44.9 | 1.6× slower | 1.5× slower |
| multipart put 256 MiB × 16 MiB | 633 | 1,108 | 1.8× slower | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 195 | 461 | 2.4× slower | 2.4× slower |
| overwrite 1 MiB | 14.1 | 33.6 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.9 | 33.6 | 2.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.8 | 40.7 | 3.0× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| rename 64 MiB | 64 | 0.16 |  |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.45 |  | 0.02 |  | 1.00 | 0.44 |
| insert 4 KiB, start of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| write at 4 KiB in 64 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| delete 4 KiB, start of 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| insert 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.45 |  | 5.23 |  | 15.53 | 0.69 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.44 |  | 0.11 |  | 11.72 | 0.61 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.78 |  |  |  | 28.19 | 0.59 |
| put 32 MiB | 64 | 15.08 |  |  |  | 14.48 | 0.59 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.63 |  |  |  | 1.16 | 0.48 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.35 |  |  |  | 1.00 | 0.35 |
| insert 4 KiB, start of 1 MiB | 128 | 1.34 |  |  |  | 1.00 | 0.34 |
| put 1 MiB | 400 | 1.54 |  |  |  | 1.17 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 174.28 | 1.00 | 33.03 | 1.00 | 138.34 | 0.91 |
| overwrite 1 MiB | 400 | 1.54 |  |  |  | 1.18 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.35 |  |  |  | 1.00 | 0.35 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 61.31 | 1.00 | 17.00 | 1.00 | 41.81 | 0.50 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  |  | 0.14 |
