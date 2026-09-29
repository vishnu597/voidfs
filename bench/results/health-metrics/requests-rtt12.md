# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T142446Z` |
| When | 2026-09-29T14:24:46Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | acfb616 (with uncommitted changes) |
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
| head | 12.3 | 0.29 | 42× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.5 | 0.31 | 41× faster | 34× faster |
| get 4 KiB | 12.4 | 0.35 | 36× faster | 23× faster |
| list 200 keys | 40.4 | 1.1 | 35× faster | 9.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.3 | 0.50 | 25× faster | 5.3× faster |
| move dir 200 × 64 KiB | 482 | 24.4 | 20× faster | 18× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.1 | 1.1 | 12× faster | 6.2× faster |
| get 1 MiB | 12.7 | 1.1 | 12× faster | 4.5× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 1.1 | 11× faster | 2.6× faster |
| append 4 KiB to 64 MiB | 298 | 34.5 | 8.6× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 287 | 34.9 | 8.2× faster | 13× faster |
| insert 4 KiB, middle of 64 MiB | 305 | 37.6 | 8.1× faster | 11× faster |
| write at 4 KiB in 64 MiB | 285 | 38.1 | 7.5× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 289 | 38.9 | 7.4× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 281 | 42.1 | 6.7× faster | 6.9× faster |
| insert 4 KiB, start of 64 MiB | 264 | 41.5 | 6.4× faster | 6.8× faster |
| rename 64 MiB | 128 | 23.9 | 5.4× faster | 7.9× faster |
| truncate 4 KiB, end of 32 MiB | 152 | 35.7 | 4.2× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 142 | 33.7 | 4.2× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 147 | 35.0 | 4.2× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 142 | 35.6 | 4.0× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 150 | 41.2 | 3.6× faster | 3.8× faster |
| write at 4 KiB in 32 MiB | 136 | 38.4 | 3.6× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 152 | 42.9 | 3.5× faster | 4.8× faster |
| stream get 256 MiB | 568 | 198 | 2.9× faster | 16× faster |
| stream get 64 MiB | 135 | 48.5 | 2.8× faster | 15× faster |
| get 32 MiB | 64.7 | 25.2 | 2.6× faster | 12× faster |
| get 64 MiB | 128 | 54.0 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 275 | 133 | 2.1× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 138 | 96.0 | 1.4× faster | 1.4× faster |
| append 4 KiB to 1 MiB | 27.2 | 34.4 | 1.3× slower | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 27.0 | 34.4 | 1.3× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 27.2 | 34.9 | 1.3× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 27.0 | 34.7 | 1.3× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 26.7 | 34.9 | 1.3× slower | 1.4× slower |
| multipart put 64 MiB × 8 MiB | 324 | 442 | 1.4× slower | 2.4× slower |
| write at 4 KiB in 1 MiB | 27.3 | 37.9 | 1.4× slower | 1.6× slower |
| delete 4 KiB, start of 1 MiB | 26.4 | 37.0 | 1.4× slower | 1.5× slower |
| patch 16 × 4 KiB in 1 MiB | 27.0 | 41.9 | 1.6× slower | 1.5× slower |
| multipart put 256 MiB × 16 MiB | 637 | 1,238 | 1.9× slower | 1.8× slower |
| put 32 MiB | 90.2 | 199 | 2.2× slower | 1.2× slower |
| put 64 MiB | 169 | 384 | 2.3× slower | 1.1× slower |
| put 1 MiB | 13.9 | 32.0 | 2.3× slower | 1.7× slower |
| overwrite 1 MiB | 13.5 | 32.2 | 2.4× slower | 1.9× slower |
| put 4 KiB | 13.1 | 33.3 | 2.5× slower | 2.0× slower |
| overwrite 4 KiB | 13.0 | 34.0 | 2.6× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.0 | 35.4 | 2.7× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 11.9 | 37.0 | 3.1× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.0 | 37.4 | 3.1× slower | 2.4× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.41 |  | 0.03 |  | 1.00 | 0.38 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.44 |  |  |  | 1.02 | 0.42 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.41 |  |  |  | 1.03 | 0.38 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.45 |  | 0.03 |  | 1.00 | 0.42 |
| insert 4 KiB, start of 64 MiB | 64 | 1.42 |  | 0.02 |  | 1.00 | 0.41 |
| write at 4 KiB in 64 MiB | 64 | 1.44 |  | 0.05 |  | 1.00 | 0.39 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| delete 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.45 |  |  |  | 1.00 | 0.45 |
| insert 4 KiB, start of 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.72 |  | 5.39 |  | 15.78 | 0.55 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.81 |  |  |  | 12.19 | 0.62 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 29.22 |  |  |  | 28.45 | 0.77 |
| put 32 MiB | 64 | 15.25 |  |  |  | 14.72 | 0.53 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.87 |  |  |  | 1.43 | 0.44 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.37 |  | 0.01 |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.62 |  |  |  | 1.24 | 0.38 |
| multipart put 256 MiB × 16 MiB | 32 | 174.81 | 1.00 | 33.00 | 1.00 | 138.91 | 0.91 |
| overwrite 1 MiB | 400 | 1.57 |  |  |  | 1.19 | 0.38 |
| put 4 KiB | 400 | 1.31 |  |  |  | 1.00 | 0.31 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 1.08 |  |  |  | 1.00 | 0.09 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 1.04 |  |  |  | 1.00 | 0.04 |
| multipart put 64 MiB × 8 MiB | 32 | 60.94 | 1.00 | 17.00 | 1.00 | 41.22 | 0.72 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 1.32 |  |  |  | 1.00 | 0.32 |
