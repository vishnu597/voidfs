# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T212206Z` |
| When | 2026-09-29T21:22:06Z |
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
| head | 14.0 | 0.29 | 49× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.7 | 0.33 | 42× faster | 34× faster |
| get 4 KiB | 13.9 | 0.35 | 40× faster | 23× faster |
| list 200 keys | 35.3 | 1.2 | 31× faster | 9.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.8 | 0.52 | 25× faster | 5.3× faster |
| move dir 200 × 64 KiB | 446 | 26.5 | 17× faster | 18× faster |
| get 1 MiB | 14.7 | 1.0 | 14× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.7 | 1.1 | 13× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.4 | 1.1 | 11× faster | 2.6× faster |
| append 4 KiB to 64 MiB | 284 | 37.1 | 7.7× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 290 | 38.5 | 7.5× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 282 | 37.9 | 7.4× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 267 | 36.1 | 7.4× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 279 | 40.3 | 6.9× faster | 13× faster |
| insert 4 KiB, middle of 64 MiB | 268 | 38.8 | 6.9× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 279 | 43.6 | 6.4× faster | 11× faster |
| rename 64 MiB | 121 | 26.4 | 4.6× faster | 7.9× faster |
| write at 4 KiB in 32 MiB | 152 | 34.8 | 4.4× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 161 | 39.5 | 4.1× faster | 4.8× faster |
| truncate 4 KiB, end of 32 MiB | 152 | 37.9 | 4.0× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 145 | 36.4 | 4.0× faster | 8.0× faster |
| insert 4 KiB, middle of 32 MiB | 147 | 38.6 | 3.8× faster | 3.8× faster |
| insert 4 KiB, start of 32 MiB | 147 | 39.3 | 3.7× faster | 3.5× faster |
| delete 4 KiB, start of 32 MiB | 142 | 40.4 | 3.5× faster | 4.6× faster |
| stream get 256 MiB | 506 | 191 | 2.7× faster | 16× faster |
| stream get 64 MiB | 121 | 47.3 | 2.6× faster | 15× faster |
| get 32 MiB | 60.9 | 25.6 | 2.4× faster | 12× faster |
| get 64 MiB | 112 | 47.9 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 279 | 128 | 2.2× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 138 | 89.3 | 1.5× faster | 1.4× faster |
| insert 4 KiB, middle of 1 MiB | 28.1 | 37.4 | 1.3× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 28.6 | 38.1 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 27.7 | 37.6 | 1.4× slower | 1.7× slower |
| write at 4 KiB in 1 MiB | 28.0 | 38.3 | 1.4× slower | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 28.0 | 38.8 | 1.4× slower | 2.1× slower |
| append 4 KiB to 1 MiB | 27.7 | 38.7 | 1.4× slower | 1.0× faster |
| delete 4 KiB, start of 1 MiB | 28.2 | 39.7 | 1.4× slower | 1.5× slower |
| multipart put 256 MiB × 16 MiB | 878 | 1,261 | 1.4× slower | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 316 | 460 | 1.5× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 27.8 | 43.2 | 1.6× slower | 1.5× slower |
| overwrite 4 KiB | 14.6 | 30.5 | 2.1× slower | 3.1× slower |
| put 32 MiB | 92.0 | 196 | 2.1× slower | 1.2× slower |
| put 64 MiB | 167 | 376 | 2.2× slower | 1.1× slower |
| overwrite 1 MiB | 14.2 | 32.5 | 2.3× slower | 1.9× slower |
| put 1 MiB | 13.8 | 32.5 | 2.3× slower | 1.7× slower |
| put 4 KiB | 13.5 | 31.7 | 2.4× slower | 2.0× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.6 | 37.4 | 3.0× slower | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.8 | 38.4 | 3.0× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 37.8 | 3.1× slower | 2.2× slower |
