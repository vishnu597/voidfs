# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T232947Z` |
| When | 2026-09-30T23:29:47Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | a03a25d |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.4 | 0.21 | 65× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.4 | 0.28 | 44× faster | 34× faster |
| get 4 KiB | 14.0 | 0.33 | 42× faster | 23× faster |
| list 200 keys | 37.8 | 1.1 | 35× faster | 9.1× faster |
| move dir 200 × 64 KiB | 475 | 15.7 | 30× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.4 | 0.51 | 24× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.5 | 0.92 | 14× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.9 | 1.1 | 13× faster | 6.2× faster |
| get 1 MiB | 14.3 | 1.2 | 12× faster | 4.5× faster |
| rename 64 MiB | 124 | 13.8 | 9.0× faster | 7.9× faster |
| insert 4 KiB, start of 64 MiB | 305 | 38.5 | 7.9× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 299 | 38.1 | 7.8× faster | 13× faster |
| append 4 KiB to 64 MiB | 297 | 38.0 | 7.8× faster | 15× faster |
| delete 4 KiB, start of 64 MiB | 293 | 37.8 | 7.7× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 288 | 40.9 | 7.0× faster | 11× faster |
| write at 4 KiB in 64 MiB | 267 | 38.6 | 6.9× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 279 | 40.9 | 6.8× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 156 | 36.5 | 4.3× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 147 | 37.1 | 4.0× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 152 | 38.5 | 3.9× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 157 | 41.0 | 3.8× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 148 | 38.8 | 3.8× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 156 | 42.0 | 3.7× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 143 | 38.6 | 3.7× faster | 8.0× faster |
| stream get 256 MiB | 569 | 200 | 2.8× faster | 16× faster |
| stream get 64 MiB | 135 | 52.6 | 2.6× faster | 15× faster |
| get 32 MiB | 67.9 | 27.3 | 2.5× faster | 12× faster |
| get 64 MiB | 128 | 53.9 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 280 | 128 | 2.2× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 134 | 96.7 | 1.4× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 294 | 217 | 1.4× faster | 2.4× slower |
| put 4 KiB | 13.8 | 15.4 | 1.1× slower | 2.0× slower |
| overwrite 4 KiB | 13.0 | 15.3 | 1.2× slower | 3.1× slower |
| put 32 MiB | 89.4 | 112 | 1.3× slower | 1.2× slower |
| put 64 MiB | 162 | 206 | 1.3× slower | 1.1× slower |
| append 4 KiB to 1 MiB | 29.4 | 37.7 | 1.3× slower | 1.0× faster |
| patch 16 × 4 KiB in 1 MiB | 29.5 | 37.9 | 1.3× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 28.5 | 37.2 | 1.3× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 29.2 | 38.2 | 1.3× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 29.3 | 38.8 | 1.3× slower | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 27.8 | 37.6 | 1.4× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 27.9 | 38.8 | 1.4× slower | 1.6× slower |
| delete 4 KiB, start of 1 MiB | 28.0 | 39.7 | 1.4× slower | 1.5× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.0 | 17.4 | 1.4× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.2 | 17.9 | 1.5× slower | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 625 | 940 | 1.5× slower | 1.8× slower |
| overwrite 1 MiB | 14.5 | 33.5 | 2.3× slower | 1.9× slower |
| put 1 MiB | 13.9 | 33.4 | 2.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.2 | 37.8 | 2.9× slower | 2.8× slower |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | delete_prefix | get | put | put_new |
|---|--:|--:|--:|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.00 |  |  |  |  |
| get 4 KiB | 400 | 0.00 |  |  |  |  |
| move dir 200 × 64 KiB | 32 | 0.19 |  |  |  | 0.19 |
| get 64 MiB | 64 | 0.00 |  |  |  |  |
| stream get 256 MiB | 32 | 0.00 |  |  |  |  |
| stream get 64 MiB | 64 | 0.00 |  |  |  |  |
| append 4 KiB to 64 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.38 |  | 0.02 | 1.00 | 0.36 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  | 1.02 | 0.41 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| insert 4 KiB, start of 64 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| write at 4 KiB in 64 MiB | 64 | 1.44 |  | 0.02 | 1.00 | 0.42 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| delete 4 KiB, start of 32 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.44 |  |  | 1.00 | 0.44 |
| insert 4 KiB, start of 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 22.11 |  | 5.66 | 15.77 | 0.69 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.80 |  | 0.23 | 11.92 | 0.64 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.95 |  |  | 28.34 | 0.61 |
| put 32 MiB | 64 | 15.53 |  |  | 14.86 | 0.67 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.37 |  |  | 1.01 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.74 |  |  | 1.38 | 0.36 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.35 |  |  | 1.00 | 0.35 |
| insert 4 KiB, start of 1 MiB | 128 | 1.37 |  |  | 1.00 | 0.37 |
| put 1 MiB | 400 | 1.55 |  |  | 1.19 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 171.59 | 0.78 | 33.03 | 137.16 | 0.62 |
| overwrite 1 MiB | 400 | 1.54 |  |  | 1.17 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.35 |  |  | 1.00 | 0.35 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 61.47 | 0.91 | 17.00 | 42.62 | 0.94 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.13 |  |  |  | 0.13 |
