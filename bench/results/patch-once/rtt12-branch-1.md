# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T190634Z` |
| When | 2026-09-30T19:06:34Z |
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

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.9×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.1 | 0.27 | 49× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.8 | 0.32 | 40× faster | 34× faster |
| list 200 keys | 40.9 | 1.1 | 36× faster | 9.1× faster |
| get 4 KiB | 13.5 | 0.41 | 33× faster | 23× faster |
| move dir 200 × 64 KiB | 459 | 15.7 | 29× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 13.0 | 0.47 | 27× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.3 | 0.97 | 13× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.6 | 1.1 | 13× faster | 6.2× faster |
| get 1 MiB | 13.8 | 1.2 | 12× faster | 4.5× faster |
| rename 64 MiB | 125 | 13.7 | 9.1× faster | 7.9× faster |
| insert 4 KiB, start of 64 MiB | 296 | 38.2 | 7.7× faster | 6.8× faster |
| append 4 KiB to 64 MiB | 293 | 39.5 | 7.4× faster | 15× faster |
| delete 4 KiB, start of 64 MiB | 281 | 38.0 | 7.4× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 278 | 37.6 | 7.4× faster | 13× faster |
| write at 4 KiB in 64 MiB | 280 | 38.7 | 7.2× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 301 | 43.4 | 6.9× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 252 | 37.4 | 6.7× faster | 11× faster |
| delete 4 KiB, start of 32 MiB | 156 | 36.7 | 4.2× faster | 4.6× faster |
| truncate 4 KiB, end of 32 MiB | 148 | 38.2 | 3.9× faster | 5.9× faster |
| insert 4 KiB, start of 32 MiB | 144 | 37.6 | 3.8× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 154 | 40.5 | 3.8× faster | 3.8× faster |
| write at 4 KiB in 32 MiB | 141 | 38.0 | 3.7× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 153 | 42.0 | 3.6× faster | 4.8× faster |
| append 4 KiB to 32 MiB | 127 | 36.7 | 3.5× faster | 8.0× faster |
| stream get 256 MiB | 547 | 197 | 2.8× faster | 16× faster |
| stream get 64 MiB | 133 | 48.7 | 2.7× faster | 15× faster |
| get 32 MiB | 66.3 | 26.5 | 2.5× faster | 12× faster |
| get 64 MiB | 120 | 51.1 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 282 | 122 | 2.3× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 145 | 100 | 1.4× faster | 1.4× faster |
| overwrite 4 KiB | 13.8 | 16.0 | 1.2× slower | 3.1× slower |
| put 4 KiB | 13.7 | 16.7 | 1.2× slower | 2.0× slower |
| put 32 MiB | 88.8 | 109 | 1.2× slower | 1.2× slower |
| put 64 MiB | 165 | 206 | 1.3× slower | 1.1× slower |
| write at 4 KiB in 1 MiB | 28.9 | 36.4 | 1.3× slower | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 28.0 | 36.3 | 1.3× slower | 2.1× slower |
| patch 16 × 4 KiB in 1 MiB | 27.5 | 35.8 | 1.3× slower | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 27.4 | 36.1 | 1.3× slower | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 27.9 | 37.6 | 1.3× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 28.0 | 38.1 | 1.4× slower | 1.0× faster |
| delete 4 KiB, start of 1 MiB | 28.8 | 39.6 | 1.4× slower | 1.5× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.2 | 17.1 | 1.4× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.4 | 18.2 | 1.5× slower | 2.4× slower |
| insert 4 KiB, start of 1 MiB | 26.7 | 39.7 | 1.5× slower | 1.7× slower |
| multipart put 64 MiB × 8 MiB | 256 | 410 | 1.6× slower | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 695 | 1,144 | 1.6× slower | 1.8× slower |
| overwrite 1 MiB | 14.1 | 33.5 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.6 | 33.6 | 2.5× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.2 | 39.4 | 3.0× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.41 |  |  |  | 1.02 | 0.39 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.42 |  | 0.03 |  | 1.00 | 0.39 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| rename 64 MiB | 64 | 0.16 |  |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.42 |  | 0.05 |  | 1.00 | 0.38 |
| insert 4 KiB, start of 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| write at 4 KiB in 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.45 |  |  |  | 1.02 | 0.44 |
| delete 4 KiB, start of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| insert 4 KiB, start of 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.17 |  | 4.95 |  | 15.50 | 0.72 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.42 |  |  |  | 11.92 | 0.50 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.66 |  |  |  | 28.06 | 0.59 |
| put 32 MiB | 64 | 15.09 |  |  |  | 14.44 | 0.66 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.51 |  |  |  | 1.15 | 0.36 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.37 |  |  |  | 1.00 | 0.37 |
| put 1 MiB | 400 | 1.53 |  |  |  | 1.16 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 173.47 | 1.00 | 33.03 | 1.00 | 137.75 | 0.69 |
| overwrite 1 MiB | 400 | 1.55 |  |  |  | 1.18 | 0.38 |
| put 4 KiB | 400 | 0.17 |  |  |  |  | 0.17 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 61.56 | 1.00 | 17.00 | 1.00 | 42.09 | 0.47 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.08 |  |  |  | 1.00 | 0.08 |
| overwrite 4 KiB | 400 | 0.15 |  |  |  |  | 0.15 |
