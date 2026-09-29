# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T212457Z` |
| When | 2026-09-29T21:24:57Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 41d489e |
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
| head | 13.5 | 0.28 | 48× faster | 13× faster |
| get 4 KiB | 14.1 | 0.36 | 39× faster | 23× faster |
| range 64 KiB of 64 MiB | 12.6 | 0.33 | 39× faster | 34× faster |
| list 200 keys | 38.3 | 1.1 | 34× faster | 9.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.3 | 0.58 | 21× faster | 5.3× faster |
| move dir 200 × 64 KiB | 452 | 26.8 | 17× faster | 18× faster |
| get 1 MiB | 13.1 | 0.98 | 13× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.8 | 1.1 | 13× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 1.1 | 11× faster | 2.6× faster |
| delete 4 KiB, start of 64 MiB | 296 | 38.2 | 7.8× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 287 | 37.2 | 7.7× faster | 15× faster |
| delete 4 KiB, middle of 64 MiB | 300 | 40.0 | 7.5× faster | 11× faster |
| truncate 4 KiB, end of 64 MiB | 286 | 39.2 | 7.3× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 282 | 41.9 | 6.7× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 274 | 42.0 | 6.5× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 274 | 42.8 | 6.4× faster | 11× faster |
| rename 64 MiB | 127 | 26.6 | 4.8× faster | 7.9× faster |
| truncate 4 KiB, end of 32 MiB | 161 | 36.3 | 4.4× faster | 5.9× faster |
| insert 4 KiB, start of 32 MiB | 152 | 38.3 | 4.0× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 151 | 38.1 | 4.0× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 157 | 41.9 | 3.7× faster | 3.8× faster |
| append 4 KiB to 32 MiB | 133 | 37.0 | 3.6× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 133 | 37.8 | 3.5× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 142 | 43.6 | 3.3× faster | 4.8× faster |
| stream get 64 MiB | 135 | 51.0 | 2.6× faster | 15× faster |
| get 32 MiB | 67.0 | 25.7 | 2.6× faster | 12× faster |
| stream get 256 MiB | 526 | 204 | 2.6× faster | 16× faster |
| get 64 MiB | 117 | 51.1 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 289 | 140 | 2.1× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 136 | 92.2 | 1.5× faster | 1.4× faster |
| put 32 MiB | 87.4 | 112 | 1.3× slower | 1.2× slower |
| put 64 MiB | 164 | 211 | 1.3× slower | 1.1× slower |
| write at 4 KiB in 1 MiB | 27.3 | 35.5 | 1.3× slower | 1.6× slower |
| append 4 KiB to 1 MiB | 26.5 | 34.7 | 1.3× slower | 1.0× faster |
| truncate 4 KiB, end of 1 MiB | 27.1 | 35.5 | 1.3× slower | 2.1× slower |
| insert 4 KiB, middle of 1 MiB | 26.9 | 35.8 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 27.0 | 36.5 | 1.4× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 26.9 | 37.6 | 1.4× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 26.8 | 37.6 | 1.4× slower | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 255 | 385 | 1.5× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 25.8 | 44.4 | 1.7× slower | 1.5× slower |
| multipart put 256 MiB × 16 MiB | 550 | 1,141 | 2.1× slower | 1.8× slower |
| put 1 MiB | 13.9 | 32.4 | 2.3× slower | 1.7× slower |
| overwrite 1 MiB | 13.5 | 32.3 | 2.4× slower | 1.9× slower |
| put 4 KiB | 13.0 | 33.4 | 2.6× slower | 2.0× slower |
| overwrite 4 KiB | 12.4 | 32.0 | 2.6× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.9 | 35.7 | 2.8× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.6 | 37.4 | 3.0× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.0 | 37.9 | 3.2× slower | 2.2× slower |
