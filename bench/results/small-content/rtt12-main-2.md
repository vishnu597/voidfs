# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T025531Z` |
| When | 2026-09-30T02:55:31Z |
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

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.6×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.3 | 0.29 | 43× faster | 13× faster |
| range 64 KiB of 64 MiB | 11.9 | 0.33 | 37× faster | 34× faster |
| list 200 keys | 36.1 | 1.1 | 34× faster | 9.1× faster |
| get 4 KiB | 12.7 | 0.37 | 34× faster | 23× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.3 | 0.53 | 23× faster | 5.3× faster |
| move dir 200 × 64 KiB | 442 | 24.4 | 18× faster | 18× faster |
| get 1 MiB | 12.5 | 0.97 | 13× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.2 | 1.1 | 12× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.0 | 1.0 | 12× faster | 2.6× faster |
| truncate 4 KiB, end of 64 MiB | 294 | 35.0 | 8.4× faster | 13× faster |
| delete 4 KiB, start of 64 MiB | 294 | 36.4 | 8.1× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 286 | 35.8 | 8.0× faster | 15× faster |
| write at 4 KiB in 64 MiB | 286 | 36.7 | 7.8× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 290 | 38.9 | 7.5× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 293 | 41.0 | 7.2× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 270 | 41.2 | 6.6× faster | 11× faster |
| rename 64 MiB | 132 | 24.1 | 5.5× faster | 7.9× faster |
| delete 4 KiB, start of 32 MiB | 158 | 35.1 | 4.5× faster | 4.6× faster |
| write at 4 KiB in 32 MiB | 152 | 35.5 | 4.3× faster | 5.1× faster |
| truncate 4 KiB, end of 32 MiB | 148 | 34.6 | 4.3× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 147 | 35.6 | 4.1× faster | 8.0× faster |
| delete 4 KiB, middle of 32 MiB | 144 | 36.2 | 4.0× faster | 4.8× faster |
| insert 4 KiB, start of 32 MiB | 145 | 37.0 | 3.9× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 150 | 38.8 | 3.9× faster | 3.8× faster |
| stream get 256 MiB | 586 | 213 | 2.7× faster | 16× faster |
| stream get 64 MiB | 134 | 50.2 | 2.7× faster | 15× faster |
| get 64 MiB | 127 | 52.6 | 2.4× faster | 17× faster |
| get 32 MiB | 64.4 | 27.6 | 2.3× faster | 12× faster |
| patch 16 × 4 KiB in 64 MiB | 277 | 158 | 1.8× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 150 | 130 | 1.1× faster | 1.4× faster |
| delete 4 KiB, middle of 1 MiB | 27.6 | 35.4 | 1.3× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 27.1 | 34.9 | 1.3× slower | 1.6× slower |
| insert 4 KiB, start of 1 MiB | 26.7 | 34.6 | 1.3× slower | 1.7× slower |
| put 32 MiB | 90.0 | 118 | 1.3× slower | 1.2× slower |
| truncate 4 KiB, end of 1 MiB | 26.6 | 35.4 | 1.3× slower | 2.1× slower |
| put 64 MiB | 167 | 225 | 1.3× slower | 1.1× slower |
| append 4 KiB to 1 MiB | 27.0 | 36.4 | 1.4× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 26.7 | 36.2 | 1.4× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 26.4 | 36.2 | 1.4× slower | 1.5× slower |
| patch 16 × 4 KiB in 1 MiB | 27.4 | 45.0 | 1.6× slower | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 238 | 416 | 1.7× slower | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 595 | 1,088 | 1.8× slower | 1.8× slower |
| put 1 MiB | 13.9 | 32.6 | 2.4× slower | 1.7× slower |
| overwrite 1 MiB | 13.5 | 32.8 | 2.4× slower | 1.9× slower |
| put 4 KiB | 13.1 | 32.2 | 2.5× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.3 | 36.8 | 2.8× slower | 2.8× slower |
| overwrite 4 KiB | 13.0 | 36.6 | 2.8× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 11.9 | 36.8 | 3.1× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.1 | 37.7 | 3.1× slower | 2.4× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| insert 4 KiB, start of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| write at 4 KiB in 64 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| delete 4 KiB, start of 32 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| insert 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.33 |  | 5.27 |  | 15.34 | 0.72 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.28 |  |  |  | 11.77 | 0.52 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.58 |  |  |  | 28.08 | 0.50 |
| put 32 MiB | 64 | 14.94 |  |  |  | 14.36 | 0.58 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.63 |  |  |  | 1.15 | 0.48 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.35 |  |  |  | 1.00 | 0.35 |
| put 1 MiB | 400 | 1.57 |  |  |  | 1.20 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 173.72 | 1.00 | 33.03 | 1.00 | 137.88 | 0.81 |
| overwrite 1 MiB | 400 | 1.54 |  |  |  | 1.17 | 0.37 |
| put 4 KiB | 400 | 1.31 |  |  |  | 1.00 | 0.31 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 1.08 |  |  |  | 1.00 | 0.08 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 1.04 |  |  |  | 1.00 | 0.04 |
| multipart put 64 MiB × 8 MiB | 32 | 61.47 | 1.00 | 17.00 | 1.00 | 42.16 | 0.31 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 1.32 |  |  |  | 1.00 | 0.32 |
