# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T150543Z` |
| When | 2026-09-30T15:05:43Z |
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
| head | 12.2 | 0.24 | 52× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.2 | 0.26 | 48× faster | 34× faster |
| list 200 keys | 45.2 | 1.1 | 42× faster | 9.1× faster |
| get 4 KiB | 12.7 | 0.31 | 41× faster | 23× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.4 | 0.48 | 26× faster | 5.3× faster |
| move dir 200 × 64 KiB | 456 | 24.1 | 19× faster | 18× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.3 | 1.0 | 12× faster | 2.6× faster |
| get 1 MiB | 12.9 | 1.2 | 11× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.3 | 1.2 | 11× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 290 | 34.9 | 8.3× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 289 | 35.2 | 8.2× faster | 15× faster |
| write at 4 KiB in 64 MiB | 293 | 36.9 | 7.9× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 279 | 36.4 | 7.7× faster | 11× faster |
| truncate 4 KiB, end of 64 MiB | 275 | 36.5 | 7.5× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 276 | 39.5 | 7.0× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 259 | 43.0 | 6.0× faster | 11× faster |
| rename 64 MiB | 132 | 24.3 | 5.4× faster | 7.9× faster |
| delete 4 KiB, start of 32 MiB | 155 | 35.5 | 4.4× faster | 4.6× faster |
| truncate 4 KiB, end of 32 MiB | 152 | 36.1 | 4.2× faster | 5.9× faster |
| insert 4 KiB, start of 32 MiB | 143 | 34.4 | 4.2× faster | 3.5× faster |
| append 4 KiB to 32 MiB | 144 | 35.2 | 4.1× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 154 | 39.2 | 3.9× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 141 | 39.2 | 3.6× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 126 | 36.4 | 3.5× faster | 4.8× faster |
| stream get 256 MiB | 532 | 187 | 2.8× faster | 16× faster |
| stream get 64 MiB | 121 | 51.6 | 2.3× faster | 15× faster |
| get 64 MiB | 120 | 53.7 | 2.2× faster | 17× faster |
| get 32 MiB | 56.6 | 26.5 | 2.1× faster | 12× faster |
| patch 16 × 4 KiB in 64 MiB | 277 | 142 | 1.9× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 149 | 108 | 1.4× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 909 | 1,047 | 1.2× slower | 1.8× slower |
| delete 4 KiB, middle of 1 MiB | 27.6 | 36.0 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 28.2 | 36.9 | 1.3× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 26.7 | 35.1 | 1.3× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 26.2 | 34.9 | 1.3× slower | 1.0× faster |
| put 32 MiB | 87.7 | 120 | 1.4× slower | 1.2× slower |
| write at 4 KiB in 1 MiB | 26.9 | 37.0 | 1.4× slower | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 27.6 | 38.0 | 1.4× slower | 2.1× slower |
| delete 4 KiB, start of 1 MiB | 26.6 | 37.1 | 1.4× slower | 1.5× slower |
| put 64 MiB | 166 | 236 | 1.4× slower | 1.1× slower |
| multipart put 64 MiB × 8 MiB | 286 | 454 | 1.6× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 26.3 | 46.3 | 1.8× slower | 1.5× slower |
| overwrite 4 KiB | 14.3 | 28.5 | 2.0× slower | 3.1× slower |
| put 4 KiB | 14.2 | 28.8 | 2.0× slower | 2.0× slower |
| overwrite 1 MiB | 14.3 | 33.7 | 2.3× slower | 1.9× slower |
| put 1 MiB | 13.5 | 32.8 | 2.4× slower | 1.7× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.3 | 30.5 | 2.5× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.3 | 31.0 | 2.5× slower | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.6 | 39.3 | 2.9× slower | 2.8× slower |

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
| truncate 4 KiB, end of 64 MiB | 64 | 1.41 |  | 0.02 |  | 1.00 | 0.39 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| insert 4 KiB, start of 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| write at 4 KiB in 64 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.45 |  |  |  | 1.00 | 0.45 |
| delete 4 KiB, start of 32 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| insert 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.84 |  | 5.36 |  | 15.78 | 0.70 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.02 |  | 0.05 |  | 11.41 | 0.56 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.94 |  |  |  | 28.34 | 0.59 |
| put 32 MiB | 64 | 15.72 |  |  |  | 15.14 | 0.58 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.37 |  |  |  | 1.01 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.62 |  |  |  | 1.14 | 0.48 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.55 |  |  |  | 1.18 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 175.38 | 1.00 | 33.56 | 1.00 | 138.88 | 0.94 |
| overwrite 1 MiB | 400 | 1.58 |  |  |  | 1.21 | 0.37 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 60.34 | 1.00 | 17.00 | 1.00 | 40.97 | 0.38 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
