# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T140616Z` |
| When | 2026-09-29T14:06:16Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **2.6×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.5 | 0.29 | 44× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.1 | 0.32 | 38× faster | 34× faster |
| get 4 KiB | 13.3 | 0.35 | 38× faster | 23× faster |
| list 200 keys | 38.8 | 1.1 | 35× faster | 9.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.3 | 0.54 | 23× faster | 5.3× faster |
| move dir 200 × 64 KiB | 452 | 24.0 | 19× faster | 18× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.1 | 0.98 | 13× faster | 6.2× faster |
| get 1 MiB | 12.6 | 0.98 | 13× faster | 4.5× faster |
| fanout get 1000 × 4 KiB, 64 at once | 11.9 | 1.1 | 11× faster | 2.6× faster |
| insert 4 KiB, start of 64 MiB | 308 | 38.3 | 8.1× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 302 | 38.0 | 8.0× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 290 | 36.8 | 7.9× faster | 11× faster |
| write at 4 KiB in 64 MiB | 293 | 37.4 | 7.8× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 291 | 37.3 | 7.8× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 278 | 38.2 | 7.3× faster | 13× faster |
| delete 4 KiB, middle of 64 MiB | 281 | 39.3 | 7.1× faster | 11× faster |
| rename 64 MiB | 132 | 24.6 | 5.4× faster | 7.9× faster |
| truncate 4 KiB, end of 32 MiB | 149 | 34.7 | 4.3× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 158 | 37.1 | 4.3× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 145 | 36.7 | 4.0× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 145 | 36.9 | 3.9× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 146 | 39.5 | 3.7× faster | 3.8× faster |
| insert 4 KiB, start of 32 MiB | 136 | 37.1 | 3.7× faster | 3.5× faster |
| delete 4 KiB, start of 32 MiB | 137 | 39.5 | 3.5× faster | 4.6× faster |
| stream get 256 MiB | 507 | 186 | 2.7× faster | 16× faster |
| stream get 64 MiB | 120 | 44.9 | 2.7× faster | 15× faster |
| get 32 MiB | 61.0 | 24.7 | 2.5× faster | 12× faster |
| get 64 MiB | 112 | 47.9 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 282 | 149 | 1.9× faster | 2.1× faster |
| multipart put 256 MiB × 16 MiB | 2,002 | 1,298 | 1.5× faster | 1.8× slower |
| patch 16 × 4 KiB in 32 MiB | 138 | 97.7 | 1.4× faster | 1.4× faster |
| delete 4 KiB, start of 1 MiB | 28.2 | 35.7 | 1.3× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 27.4 | 35.3 | 1.3× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 27.7 | 35.8 | 1.3× slower | 1.0× faster |
| write at 4 KiB in 1 MiB | 27.4 | 35.6 | 1.3× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 26.6 | 34.9 | 1.3× slower | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 27.2 | 36.3 | 1.3× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 26.8 | 37.0 | 1.4× slower | 2.1× slower |
| multipart put 64 MiB × 8 MiB | 287 | 442 | 1.5× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 27.4 | 48.6 | 1.8× slower | 1.5× slower |
| put 32 MiB | 90.6 | 196 | 2.2× slower | 1.2× slower |
| put 64 MiB | 173 | 382 | 2.2× slower | 1.1× slower |
| overwrite 4 KiB | 13.3 | 30.0 | 2.3× slower | 3.1× slower |
| overwrite 1 MiB | 13.6 | 32.4 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.5 | 32.3 | 2.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.1 | 37.4 | 2.8× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.0 | 35.6 | 3.0× slower | 2.4× slower |
| put 4 KiB | 12.4 | 37.2 | 3.0× slower | 2.0× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 37.8 | 3.1× slower | 2.2× slower |
