# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T025243Z` |
| When | 2026-09-30T02:52:43Z |
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

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.7×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.4 | 0.26 | 48× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.0 | 0.30 | 40× faster | 34× faster |
| list 200 keys | 34.6 | 1.1 | 32× faster | 9.1× faster |
| get 4 KiB | 13.4 | 0.45 | 30× faster | 23× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.0 | 0.49 | 25× faster | 5.3× faster |
| move dir 200 × 64 KiB | 471 | 24.4 | 19× faster | 18× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 0.97 | 13× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 12.9 | 1.1 | 12× faster | 6.2× faster |
| get 1 MiB | 13.3 | 1.1 | 12× faster | 4.5× faster |
| write at 4 KiB in 64 MiB | 307 | 36.1 | 8.5× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 303 | 37.9 | 8.0× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 292 | 36.8 | 8.0× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 292 | 37.6 | 7.8× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 279 | 36.4 | 7.7× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 277 | 37.8 | 7.3× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 268 | 41.1 | 6.5× faster | 11× faster |
| rename 64 MiB | 121 | 24.5 | 4.9× faster | 7.9× faster |
| write at 4 KiB in 32 MiB | 152 | 34.8 | 4.4× faster | 5.1× faster |
| truncate 4 KiB, end of 32 MiB | 149 | 34.6 | 4.3× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 146 | 35.5 | 4.1× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 148 | 36.2 | 4.1× faster | 3.5× faster |
| append 4 KiB to 32 MiB | 145 | 35.7 | 4.1× faster | 8.0× faster |
| insert 4 KiB, middle of 32 MiB | 150 | 38.0 | 3.9× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 142 | 37.4 | 3.8× faster | 4.8× faster |
| stream get 256 MiB | 598 | 207 | 2.9× faster | 16× faster |
| stream get 64 MiB | 135 | 54.5 | 2.5× faster | 15× faster |
| get 32 MiB | 67.5 | 28.3 | 2.4× faster | 12× faster |
| get 64 MiB | 123 | 54.8 | 2.2× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 283 | 154 | 1.8× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 139 | 100 | 1.4× faster | 1.4× faster |
| insert 4 KiB, middle of 1 MiB | 27.0 | 34.2 | 1.3× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 28.1 | 36.3 | 1.3× slower | 2.1× slower |
| append 4 KiB to 1 MiB | 27.3 | 35.4 | 1.3× slower | 1.0× faster |
| multipart put 256 MiB × 16 MiB | 847 | 1,098 | 1.3× slower | 1.8× slower |
| delete 4 KiB, middle of 1 MiB | 26.9 | 34.8 | 1.3× slower | 1.4× slower |
| put 64 MiB | 169 | 219 | 1.3× slower | 1.1× slower |
| put 32 MiB | 89.7 | 119 | 1.3× slower | 1.2× slower |
| insert 4 KiB, start of 1 MiB | 27.1 | 36.1 | 1.3× slower | 1.7× slower |
| delete 4 KiB, start of 1 MiB | 27.2 | 36.4 | 1.3× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 26.2 | 36.1 | 1.4× slower | 1.6× slower |
| patch 16 × 4 KiB in 1 MiB | 27.3 | 42.4 | 1.6× slower | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 262 | 488 | 1.9× slower | 2.4× slower |
| put 4 KiB | 12.5 | 26.2 | 2.1× slower | 2.0× slower |
| overwrite 4 KiB | 12.4 | 26.6 | 2.1× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.1 | 27.8 | 2.3× slower | 2.4× slower |
| put 1 MiB | 13.8 | 32.4 | 2.3× slower | 1.7× slower |
| overwrite 1 MiB | 14.0 | 33.0 | 2.3× slower | 1.9× slower |
| fanout put 1000 × 4 KiB, 32 at once | 11.8 | 27.9 | 2.4× slower | 2.2× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.3 | 38.7 | 2.9× slower | 2.8× slower |

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
| truncate 4 KiB, end of 64 MiB | 64 | 1.44 |  | 0.03 |  | 1.00 | 0.41 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.48 |  | 0.05 |  | 1.00 | 0.44 |
| insert 4 KiB, start of 64 MiB | 64 | 1.42 |  | 0.03 |  | 1.00 | 0.39 |
| write at 4 KiB in 64 MiB | 64 | 1.39 |  | 0.02 |  | 1.00 | 0.38 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| delete 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.41 |  |  |  | 1.02 | 0.39 |
| insert 4 KiB, start of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.73 |  | 4.89 |  | 15.22 | 0.62 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.66 |  |  |  | 10.98 | 0.67 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.92 |  |  |  | 28.28 | 0.64 |
| put 32 MiB | 64 | 15.17 |  |  |  | 14.62 | 0.55 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.73 |  |  |  | 1.29 | 0.45 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.35 |  |  |  | 1.00 | 0.35 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.55 |  |  |  | 1.19 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 174.28 | 1.00 | 33.03 | 1.00 | 138.38 | 0.88 |
| overwrite 1 MiB | 400 | 1.59 |  |  |  | 1.22 | 0.37 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.16 | 1.00 | 17.00 | 1.00 | 41.41 | 0.75 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
