# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T020757Z` |
| When | 2026-10-01T02:07:57Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 202c38a |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **32 of 49** scenarios and slower in the other **17**. Geometric mean speed-up over the bare bucket: **3.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.6 | 0.23 | 59× faster | 13× faster |
| get 4 KiB | 14.2 | 0.31 | 45× faster | 23× faster |
| range 64 KiB of 64 MiB | 13.1 | 0.29 | 45× faster | 34× faster |
| list 200 keys | 38.3 | 0.98 | 39× faster | 9.1× faster |
| move dir 200 × 64 KiB | 447 | 15.7 | 28× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.9 | 0.47 | 28× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.4 | 0.96 | 13× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.2 | 1.1 | 12× faster | 6.2× faster |
| get 1 MiB | 14.6 | 1.3 | 11× faster | 4.5× faster |
| rename 64 MiB | 119 | 13.8 | 8.6× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 284 | 37.3 | 7.6× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 264 | 35.3 | 7.5× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 282 | 39.0 | 7.2× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 277 | 38.5 | 7.2× faster | 13× faster |
| delete 4 KiB, start of 64 MiB | 278 | 39.9 | 7.0× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 288 | 43.5 | 6.6× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 265 | 40.2 | 6.6× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 157 | 36.0 | 4.4× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 158 | 38.0 | 4.1× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 159 | 38.8 | 4.1× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 145 | 36.3 | 4.0× faster | 4.6× faster |
| truncate 4 KiB, end of 32 MiB | 143 | 37.8 | 3.8× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 148 | 40.3 | 3.7× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 138 | 38.9 | 3.5× faster | 8.0× faster |
| stream get 256 MiB | 563 | 191 | 2.9× faster | 16× faster |
| stream get 64 MiB | 132 | 49.4 | 2.7× faster | 15× faster |
| get 32 MiB | 66.5 | 27.9 | 2.4× faster | 12× faster |
| get 64 MiB | 123 | 52.9 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 286 | 141 | 2.0× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 143 | 87.1 | 1.6× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,051 | 879 | 1.2× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 313 | 297 | 1.1× faster | 2.4× slower |
| put 4 KiB | 14.5 | 15.2 | 1.0× slower | 2.0× slower |
| overwrite 4 KiB | 14.1 | 15.8 | 1.1× slower | 3.1× slower |
| put 32 MiB | 92.5 | 113 | 1.2× slower | 1.2× slower |
| patch 16 × 4 KiB in 1 MiB | 28.4 | 35.9 | 1.3× slower | 1.5× slower |
| put 64 MiB | 162 | 217 | 1.3× slower | 1.1× slower |
| truncate 4 KiB, end of 1 MiB | 28.1 | 38.1 | 1.4× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 27.4 | 37.4 | 1.4× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 27.0 | 37.6 | 1.4× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 28.3 | 39.7 | 1.4× slower | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 27.8 | 39.1 | 1.4× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 27.8 | 39.1 | 1.4× slower | 1.0× faster |
| fanout put 1000 × 4 KiB, 64 at once | 12.4 | 17.6 | 1.4× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.3 | 17.5 | 1.4× slower | 2.2× slower |
| insert 4 KiB, start of 1 MiB | 27.6 | 39.9 | 1.4× slower | 1.7× slower |
| overwrite 1 MiB | 14.5 | 33.9 | 2.3× slower | 1.9× slower |
| put 1 MiB | 13.7 | 33.9 | 2.5× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.2 | 39.5 | 3.0× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.42 |  | 0.03 | 1.00 | 0.39 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.44 |  |  | 1.00 | 0.44 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| insert 4 KiB, start of 64 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| write at 4 KiB in 64 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.45 |  |  | 1.00 | 0.45 |
| delete 4 KiB, start of 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| insert 4 KiB, start of 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.38 |  | 5.73 | 15.06 | 0.58 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.94 |  | 0.16 | 12.09 | 0.69 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.86 |  |  | 28.28 | 0.58 |
| put 32 MiB | 64 | 15.14 |  |  | 14.47 | 0.67 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.74 |  |  | 1.38 | 0.36 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.35 |  |  | 1.00 | 0.35 |
| put 1 MiB | 400 | 1.57 |  |  | 1.20 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 173.25 | 0.94 | 33.03 | 138.44 | 0.84 |
| overwrite 1 MiB | 400 | 1.54 |  |  | 1.17 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 60.25 | 0.94 | 17.00 | 41.69 | 0.62 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.08 |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
