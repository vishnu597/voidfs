# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T191927Z` |
| When | 2026-09-28T19:19:27Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | ae3a466 (with uncommitted changes) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **24 of 49** scenarios and slower in the other **25**. Geometric mean speed-up over the bare bucket: **2.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.4 | 0.28 | 48× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.2 | 0.32 | 41× faster | 34× faster |
| get 4 KiB | 13.8 | 0.35 | 40× faster | 23× faster |
| list 200 keys | 37.5 | 1.1 | 36× faster | 9.1× faster |
| move dir 200 × 64 KiB | 471 | 26.4 | 18× faster | 18× faster |
| get 1 MiB | 14.4 | 1.5 | 9.7× faster | 4.5× faster |
| append 4 KiB to 64 MiB | 281 | 34.9 | 8.1× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 286 | 36.6 | 7.8× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 307 | 41.2 | 7.4× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 290 | 39.6 | 7.3× faster | 6.9× faster |
| write at 4 KiB in 64 MiB | 295 | 45.7 | 6.5× faster | 6.2× faster |
| rename 64 MiB | 163 | 26.3 | 6.2× faster | 7.9× faster |
| insert 4 KiB, middle of 64 MiB | 288 | 47.7 | 6.0× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 290 | 52.8 | 5.5× faster | 11× faster |
| append 4 KiB to 32 MiB | 140 | 35.2 | 4.0× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 148 | 38.4 | 3.9× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 156 | 44.6 | 3.5× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 139 | 44.5 | 3.1× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 135 | 44.0 | 3.1× faster | 4.6× faster |
| stream get 256 MiB | 548 | 181 | 3.0× faster | 16× faster |
| write at 4 KiB in 32 MiB | 135 | 45.8 | 3.0× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 146 | 51.3 | 2.8× faster | 4.8× faster |
| stream get 64 MiB | 127 | 46.6 | 2.7× faster | 15× faster |
| get 64 MiB | 107 | 52.1 | 2.1× faster | 17× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.4 | 12.4 | 1.0× slower | 2.6× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.7 | 13.1 | 1.0× slower | 5.3× faster |
| patch 16 × 4 KiB in 32 MiB | 139 | 157 | 1.1× slower | 1.4× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.9 | 16.0 | 1.2× slower | 6.2× faster |
| get 32 MiB | 64.8 | 79.3 | 1.2× slower | 12× faster |
| append 4 KiB to 1 MiB | 28.0 | 37.0 | 1.3× slower | 1.0× faster |
| patch 16 × 4 KiB in 64 MiB | 289 | 383 | 1.3× slower | 2.1× faster |
| delete 4 KiB, start of 1 MiB | 28.1 | 37.4 | 1.3× slower | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 27.4 | 36.6 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 28.3 | 37.9 | 1.3× slower | 1.7× slower |
| truncate 4 KiB, end of 1 MiB | 27.1 | 36.6 | 1.3× slower | 2.1× slower |
| insert 4 KiB, middle of 1 MiB | 27.5 | 37.2 | 1.4× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 27.3 | 39.9 | 1.5× slower | 1.6× slower |
| multipart put 64 MiB × 8 MiB | 284 | 447 | 1.6× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 27.1 | 46.0 | 1.7× slower | 1.5× slower |
| put 32 MiB | 89.9 | 182 | 2.0× slower | 1.2× slower |
| overwrite 4 KiB | 14.0 | 28.6 | 2.0× slower | 3.1× slower |
| put 4 KiB | 14.2 | 29.3 | 2.1× slower | 2.0× slower |
| multipart put 256 MiB × 16 MiB | 580 | 1,199 | 2.1× slower | 1.8× slower |
| put 64 MiB | 162 | 362 | 2.2× slower | 1.1× slower |
| overwrite 1 MiB | 13.8 | 32.3 | 2.3× slower | 1.9× slower |
| put 1 MiB | 13.7 | 32.1 | 2.3× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.4 | 38.7 | 2.9× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.8 | 39.3 | 3.1× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.5 | 38.6 | 3.1× slower | 2.2× slower |
