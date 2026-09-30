# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T024653Z` |
| When | 2026-09-30T02:46:53Z |
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
| head | 12.3 | 0.25 | 49× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.4 | 0.27 | 46× faster | 34× faster |
| get 4 KiB | 13.1 | 0.39 | 33× faster | 23× faster |
| list 200 keys | 34.7 | 1.2 | 30× faster | 9.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.1 | 0.52 | 23× faster | 5.3× faster |
| move dir 200 × 64 KiB | 455 | 24.0 | 19× faster | 18× faster |
| get 1 MiB | 12.7 | 0.93 | 14× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.1 | 0.97 | 14× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 1.0 | 12× faster | 2.6× faster |
| append 4 KiB to 64 MiB | 285 | 34.7 | 8.2× faster | 15× faster |
| write at 4 KiB in 64 MiB | 291 | 35.7 | 8.2× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 271 | 34.9 | 7.8× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 271 | 36.1 | 7.5× faster | 6.8× faster |
| insert 4 KiB, middle of 64 MiB | 287 | 38.8 | 7.4× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 288 | 39.3 | 7.3× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 280 | 40.9 | 6.9× faster | 6.9× faster |
| rename 64 MiB | 133 | 24.6 | 5.4× faster | 7.9× faster |
| delete 4 KiB, start of 32 MiB | 155 | 33.2 | 4.7× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 149 | 34.1 | 4.4× faster | 8.0× faster |
| truncate 4 KiB, end of 32 MiB | 145 | 35.6 | 4.1× faster | 5.9× faster |
| insert 4 KiB, start of 32 MiB | 139 | 36.1 | 3.8× faster | 3.5× faster |
| delete 4 KiB, middle of 32 MiB | 146 | 38.5 | 3.8× faster | 4.8× faster |
| write at 4 KiB in 32 MiB | 141 | 38.6 | 3.6× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 146 | 43.1 | 3.4× faster | 3.8× faster |
| stream get 256 MiB | 537 | 187 | 2.9× faster | 16× faster |
| get 32 MiB | 63.9 | 26.1 | 2.4× faster | 12× faster |
| stream get 64 MiB | 117 | 48.5 | 2.4× faster | 15× faster |
| get 64 MiB | 108 | 49.6 | 2.2× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 305 | 146 | 2.1× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 144 | 115 | 1.3× faster | 1.4× faster |
| insert 4 KiB, start of 1 MiB | 28.0 | 35.1 | 1.3× slower | 1.7× slower |
| truncate 4 KiB, end of 1 MiB | 28.0 | 35.8 | 1.3× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 28.4 | 36.2 | 1.3× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 27.1 | 35.1 | 1.3× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 27.6 | 35.9 | 1.3× slower | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 27.8 | 36.2 | 1.3× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 28.1 | 36.9 | 1.3× slower | 1.5× slower |
| put 32 MiB | 90.5 | 121 | 1.3× slower | 1.2× slower |
| put 64 MiB | 173 | 244 | 1.4× slower | 1.1× slower |
| multipart put 256 MiB × 16 MiB | 850 | 1,274 | 1.5× slower | 1.8× slower |
| patch 16 × 4 KiB in 1 MiB | 27.0 | 41.4 | 1.5× slower | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 254 | 436 | 1.7× slower | 2.4× slower |
| overwrite 1 MiB | 14.2 | 33.7 | 2.4× slower | 1.9× slower |
| put 1 MiB | 14.2 | 34.3 | 2.4× slower | 1.7× slower |
| overwrite 4 KiB | 11.8 | 31.9 | 2.7× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.0 | 37.2 | 2.9× slower | 2.8× slower |
| put 4 KiB | 12.3 | 36.0 | 2.9× slower | 2.0× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.4 | 36.5 | 3.0× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.5 | 37.5 | 3.0× slower | 2.4× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.42 |  | 0.03 |  | 1.00 | 0.39 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.38 |  |  |  | 1.02 | 0.36 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| insert 4 KiB, start of 64 MiB | 64 | 1.47 |  | 0.03 |  | 1.02 | 0.42 |
| write at 4 KiB in 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| delete 4 KiB, start of 32 MiB | 64 | 1.33 |  |  |  | 1.00 | 0.33 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.47 |  |  |  | 1.00 | 0.47 |
| insert 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.83 |  | 5.62 |  | 15.67 | 0.53 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.67 |  | 0.14 |  | 10.86 | 0.67 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.59 |  |  |  | 28.08 | 0.52 |
| put 32 MiB | 64 | 15.64 |  |  |  | 14.97 | 0.67 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.86 |  |  |  | 1.44 | 0.42 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.51 |  |  |  | 1.14 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 173.09 | 1.00 | 33.03 | 1.00 | 137.12 | 0.94 |
| overwrite 1 MiB | 400 | 1.53 |  |  |  | 1.17 | 0.37 |
| put 4 KiB | 400 | 1.32 |  |  |  | 1.00 | 0.32 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 1.08 |  |  |  | 1.00 | 0.08 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 1.04 |  |  |  | 1.00 | 0.04 |
| multipart put 64 MiB × 8 MiB | 32 | 61.66 | 1.00 | 17.00 | 1.00 | 42.12 | 0.53 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 1.31 |  |  |  | 1.00 | 0.31 |
