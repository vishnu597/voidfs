# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T233503Z` |
| When | 2026-09-30T23:35:03Z |
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

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.9×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.2 | 0.28 | 48× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.9 | 0.31 | 41× faster | 34× faster |
| get 4 KiB | 14.0 | 0.39 | 36× faster | 23× faster |
| list 200 keys | 35.0 | 1.1 | 31× faster | 9.1× faster |
| move dir 200 × 64 KiB | 455 | 15.6 | 29× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.7 | 0.50 | 26× faster | 5.3× faster |
| fanout get 200 × 256 KiB, 32 at once | 14.2 | 1.0 | 14× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 0.92 | 13× faster | 2.6× faster |
| get 1 MiB | 13.7 | 1.1 | 12× faster | 4.5× faster |
| rename 64 MiB | 129 | 13.7 | 9.4× faster | 7.9× faster |
| write at 4 KiB in 64 MiB | 304 | 39.2 | 7.8× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 295 | 40.0 | 7.4× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 273 | 37.1 | 7.4× faster | 11× faster |
| append 4 KiB to 64 MiB | 290 | 39.6 | 7.3× faster | 15× faster |
| delete 4 KiB, start of 64 MiB | 279 | 39.0 | 7.1× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 269 | 38.5 | 7.0× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 292 | 44.6 | 6.5× faster | 6.8× faster |
| write at 4 KiB in 32 MiB | 170 | 39.4 | 4.3× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 153 | 37.9 | 4.0× faster | 8.0× faster |
| truncate 4 KiB, end of 32 MiB | 144 | 37.2 | 3.9× faster | 5.9× faster |
| delete 4 KiB, middle of 32 MiB | 146 | 38.2 | 3.8× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 141 | 37.2 | 3.8× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 140 | 37.2 | 3.8× faster | 3.8× faster |
| insert 4 KiB, start of 32 MiB | 143 | 38.9 | 3.7× faster | 3.5× faster |
| stream get 256 MiB | 580 | 197 | 2.9× faster | 16× faster |
| stream get 64 MiB | 138 | 52.3 | 2.6× faster | 15× faster |
| get 64 MiB | 125 | 51.1 | 2.4× faster | 17× faster |
| get 32 MiB | 68.3 | 28.5 | 2.4× faster | 12× faster |
| patch 16 × 4 KiB in 64 MiB | 284 | 139 | 2.1× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 142 | 97.8 | 1.5× faster | 1.4× faster |
| overwrite 4 KiB | 13.8 | 14.9 | 1.1× slower | 3.1× slower |
| put 4 KiB | 13.7 | 16.1 | 1.2× slower | 2.0× slower |
| multipart put 256 MiB × 16 MiB | 918 | 1,146 | 1.2× slower | 1.8× slower |
| append 4 KiB to 1 MiB | 29.2 | 38.4 | 1.3× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 29.0 | 38.7 | 1.3× slower | 1.4× slower |
| put 64 MiB | 161 | 217 | 1.4× slower | 1.1× slower |
| put 32 MiB | 88.4 | 120 | 1.4× slower | 1.2× slower |
| delete 4 KiB, middle of 1 MiB | 28.2 | 38.9 | 1.4× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 28.4 | 39.4 | 1.4× slower | 1.7× slower |
| truncate 4 KiB, end of 1 MiB | 28.5 | 39.8 | 1.4× slower | 2.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.0 | 16.9 | 1.4× slower | 2.2× slower |
| write at 4 KiB in 1 MiB | 26.9 | 37.9 | 1.4× slower | 1.6× slower |
| patch 16 × 4 KiB in 1 MiB | 28.2 | 40.5 | 1.4× slower | 1.5× slower |
| delete 4 KiB, start of 1 MiB | 27.8 | 40.8 | 1.5× slower | 1.5× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.2 | 18.2 | 1.5× slower | 2.4× slower |
| multipart put 64 MiB × 8 MiB | 263 | 447 | 1.7× slower | 2.4× slower |
| put 1 MiB | 14.4 | 33.9 | 2.4× slower | 1.7× slower |
| overwrite 1 MiB | 13.9 | 33.5 | 2.4× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.7 | 41.4 | 3.0× slower | 2.8× slower |

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
| truncate 4 KiB, end of 64 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| rename 64 MiB | 64 | 0.16 |  |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.44 |  | 0.05 |  | 1.00 | 0.39 |
| insert 4 KiB, start of 64 MiB | 64 | 1.45 |  |  |  | 1.00 | 0.45 |
| write at 4 KiB in 64 MiB | 64 | 1.41 |  | 0.02 |  | 1.00 | 0.39 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| delete 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| insert 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.77 |  | 5.78 |  | 15.31 | 0.67 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.62 |  | 0.14 |  | 11.91 | 0.58 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.50 |  |  |  | 27.89 | 0.61 |
| put 32 MiB | 64 | 14.44 |  |  |  | 13.89 | 0.55 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.37 |  |  |  | 1.01 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.63 |  |  |  | 1.28 | 0.35 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.34 |  |  |  | 1.00 | 0.34 |
| put 1 MiB | 400 | 1.54 |  |  |  | 1.17 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 175.09 | 1.00 | 33.03 | 1.00 | 139.31 | 0.75 |
| overwrite 1 MiB | 400 | 1.55 |  |  |  | 1.17 | 0.38 |
| put 4 KiB | 400 | 0.14 |  |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.35 |  |  |  | 1.00 | 0.35 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 61.47 | 1.00 | 17.00 | 1.00 | 41.47 | 1.00 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.08 |  |  |  | 1.00 | 0.08 |
| overwrite 4 KiB | 400 | 0.13 |  |  |  |  | 0.13 |
