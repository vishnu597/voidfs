# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T190906Z` |
| When | 2026-09-30T19:09:06Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.0 | 0.20 | 61× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.6 | 0.30 | 46× faster | 34× faster |
| get 4 KiB | 13.6 | 0.34 | 40× faster | 23× faster |
| list 200 keys | 39.4 | 1.0 | 38× faster | 9.1× faster |
| move dir 200 × 64 KiB | 456 | 15.4 | 30× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 11.9 | 0.50 | 24× faster | 5.3× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.6 | 1.0 | 13× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 0.92 | 13× faster | 2.6× faster |
| get 1 MiB | 12.8 | 1.1 | 11× faster | 4.5× faster |
| rename 64 MiB | 133 | 12.4 | 11× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 283 | 32.7 | 8.6× faster | 13× faster |
| append 4 KiB to 64 MiB | 288 | 35.9 | 8.0× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 297 | 38.2 | 7.8× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 276 | 36.1 | 7.7× faster | 6.9× faster |
| write at 4 KiB in 64 MiB | 270 | 38.9 | 6.9× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 278 | 40.5 | 6.9× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 264 | 41.7 | 6.3× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 149 | 34.6 | 4.3× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 134 | 34.4 | 3.9× faster | 8.0× faster |
| delete 4 KiB, middle of 32 MiB | 146 | 37.8 | 3.9× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 139 | 37.5 | 3.7× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 141 | 40.9 | 3.4× faster | 3.8× faster |
| insert 4 KiB, start of 32 MiB | 127 | 36.8 | 3.4× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 125 | 36.8 | 3.4× faster | 5.1× faster |
| stream get 256 MiB | 577 | 187 | 3.1× faster | 16× faster |
| stream get 64 MiB | 130 | 49.5 | 2.6× faster | 15× faster |
| get 64 MiB | 124 | 51.0 | 2.4× faster | 17× faster |
| get 32 MiB | 59.3 | 25.8 | 2.3× faster | 12× faster |
| multipart put 256 MiB × 16 MiB | 2,032 | 1,006 | 2.0× faster | 1.8× slower |
| patch 16 × 4 KiB in 64 MiB | 270 | 139 | 1.9× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 143 | 95.8 | 1.5× faster | 1.4× faster |
| overwrite 4 KiB | 13.3 | 13.7 | 1.0× slower | 3.1× slower |
| put 4 KiB | 12.6 | 14.2 | 1.1× slower | 2.0× slower |
| multipart put 64 MiB × 8 MiB | 350 | 431 | 1.2× slower | 2.4× slower |
| put 64 MiB | 163 | 205 | 1.3× slower | 1.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 15.6 | 1.3× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.5 | 16.1 | 1.3× slower | 2.4× slower |
| put 32 MiB | 85.5 | 113 | 1.3× slower | 1.2× slower |
| write at 4 KiB in 1 MiB | 27.6 | 36.6 | 1.3× slower | 1.6× slower |
| append 4 KiB to 1 MiB | 26.9 | 35.9 | 1.3× slower | 1.0× faster |
| truncate 4 KiB, end of 1 MiB | 27.1 | 36.4 | 1.3× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 26.1 | 35.4 | 1.4× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 26.5 | 36.2 | 1.4× slower | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 26.9 | 37.0 | 1.4× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 26.6 | 38.7 | 1.5× slower | 1.5× slower |
| patch 16 × 4 KiB in 1 MiB | 25.5 | 37.7 | 1.5× slower | 1.5× slower |
| overwrite 1 MiB | 13.8 | 32.6 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.5 | 33.0 | 2.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.7 | 37.0 | 2.9× slower | 2.8× slower |

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
| truncate 4 KiB, end of 64 MiB | 64 | 1.34 |  | 0.02 |  | 1.00 | 0.33 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| rename 64 MiB | 64 | 0.16 |  |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.44 |  | 0.05 |  | 1.00 | 0.39 |
| insert 4 KiB, start of 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| write at 4 KiB in 64 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| delete 4 KiB, start of 32 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.45 |  |  |  | 1.02 | 0.44 |
| insert 4 KiB, start of 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.89 |  | 4.91 |  | 15.20 | 0.78 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.23 |  | 0.08 |  | 11.55 | 0.61 |
| append 4 KiB to 1 MiB | 128 | 1.34 |  |  |  | 1.00 | 0.34 |
| put 64 MiB | 64 | 28.72 |  |  |  | 27.92 | 0.80 |
| put 32 MiB | 64 | 14.83 |  |  |  | 14.17 | 0.66 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.37 |  |  |  | 1.02 | 0.35 |
| delete 4 KiB, start of 1 MiB | 128 | 1.35 |  |  |  | 1.00 | 0.35 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.52 |  |  |  | 1.16 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 174.72 | 1.00 | 33.03 | 1.00 | 138.88 | 0.81 |
| overwrite 1 MiB | 400 | 1.53 |  |  |  | 1.17 | 0.37 |
| put 4 KiB | 400 | 0.13 |  |  |  |  | 0.13 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 62.03 | 1.00 | 17.00 | 1.00 | 42.38 | 0.66 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  |  | 0.14 |
