# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T232700Z` |
| When | 2026-09-30T23:27:00Z |
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
| head | 13.3 | 0.23 | 59× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.2 | 0.30 | 44× faster | 34× faster |
| get 4 KiB | 13.6 | 0.33 | 41× faster | 23× faster |
| list 200 keys | 42.2 | 1.1 | 39× faster | 9.1× faster |
| move dir 200 × 64 KiB | 468 | 16.4 | 29× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 13.0 | 0.49 | 26× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.4 | 1.0 | 12× faster | 2.6× faster |
| get 1 MiB | 14.1 | 1.2 | 12× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.5 | 1.3 | 10× faster | 6.2× faster |
| rename 64 MiB | 121 | 13.9 | 8.7× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 292 | 36.9 | 7.9× faster | 13× faster |
| delete 4 KiB, middle of 64 MiB | 295 | 39.7 | 7.4× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 296 | 40.4 | 7.3× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 292 | 39.8 | 7.3× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 267 | 37.4 | 7.1× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 253 | 38.0 | 6.6× faster | 15× faster |
| insert 4 KiB, middle of 64 MiB | 306 | 46.4 | 6.6× faster | 11× faster |
| delete 4 KiB, middle of 32 MiB | 156 | 37.8 | 4.1× faster | 4.8× faster |
| insert 4 KiB, start of 32 MiB | 169 | 42.6 | 4.0× faster | 3.5× faster |
| delete 4 KiB, start of 32 MiB | 150 | 38.6 | 3.9× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 145 | 39.0 | 3.7× faster | 3.8× faster |
| append 4 KiB to 32 MiB | 137 | 37.2 | 3.7× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 141 | 39.4 | 3.6× faster | 5.1× faster |
| truncate 4 KiB, end of 32 MiB | 137 | 38.9 | 3.5× faster | 5.9× faster |
| stream get 256 MiB | 588 | 193 | 3.1× faster | 16× faster |
| stream get 64 MiB | 137 | 49.8 | 2.7× faster | 15× faster |
| get 32 MiB | 63.3 | 25.3 | 2.5× faster | 12× faster |
| get 64 MiB | 122 | 50.8 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 303 | 136 | 2.2× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 141 | 93.3 | 1.5× faster | 1.4× faster |
| overwrite 4 KiB | 13.3 | 15.4 | 1.2× slower | 3.1× slower |
| put 4 KiB | 13.4 | 15.6 | 1.2× slower | 2.0× slower |
| delete 4 KiB, start of 1 MiB | 29.3 | 37.5 | 1.3× slower | 1.5× slower |
| put 32 MiB | 85.6 | 114 | 1.3× slower | 1.2× slower |
| append 4 KiB to 1 MiB | 29.1 | 38.8 | 1.3× slower | 1.0× faster |
| put 64 MiB | 160 | 214 | 1.3× slower | 1.1× slower |
| insert 4 KiB, start of 1 MiB | 28.2 | 38.0 | 1.3× slower | 1.7× slower |
| write at 4 KiB in 1 MiB | 28.7 | 38.9 | 1.4× slower | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 29.3 | 39.8 | 1.4× slower | 2.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.5 | 17.6 | 1.4× slower | 2.4× slower |
| insert 4 KiB, middle of 1 MiB | 29.1 | 40.8 | 1.4× slower | 1.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.3 | 17.2 | 1.4× slower | 2.2× slower |
| delete 4 KiB, middle of 1 MiB | 27.1 | 38.1 | 1.4× slower | 1.4× slower |
| multipart put 64 MiB × 8 MiB | 286 | 415 | 1.4× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 27.4 | 40.9 | 1.5× slower | 1.5× slower |
| multipart put 256 MiB × 16 MiB | 729 | 1,153 | 1.6× slower | 1.8× slower |
| put 1 MiB | 14.1 | 33.7 | 2.4× slower | 1.7× slower |
| overwrite 1 MiB | 13.9 | 34.0 | 2.4× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.2 | 39.9 | 3.0× slower | 2.8× slower |

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
| truncate 4 KiB, end of 64 MiB | 64 | 1.45 |  | 0.03 |  | 1.00 | 0.42 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  |  | 1.03 | 0.39 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| rename 64 MiB | 64 | 0.16 |  |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| insert 4 KiB, start of 64 MiB | 64 | 1.42 |  | 0.03 |  | 1.00 | 0.39 |
| write at 4 KiB in 64 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| delete 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| insert 4 KiB, start of 32 MiB | 64 | 1.48 |  |  |  | 1.03 | 0.45 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.56 |  | 4.70 |  | 15.19 | 0.67 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.48 |  | 0.08 |  | 11.78 | 0.62 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 29.58 |  |  |  | 29.05 | 0.53 |
| put 32 MiB | 64 | 15.03 |  |  |  | 14.42 | 0.61 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.35 |  |  |  | 1.00 | 0.35 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.35 |  |  |  | 1.00 | 0.35 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.54 |  |  |  | 1.17 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 174.59 | 1.00 | 33.06 | 1.00 | 138.66 | 0.88 |
| overwrite 1 MiB | 400 | 1.57 |  |  |  | 1.20 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 61.09 | 1.00 | 17.00 | 1.00 | 41.59 | 0.50 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  |  | 0.14 |
