# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T141201Z` |
| When | 2026-09-29T14:12:01Z |
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
| head | 12.6 | 0.28 | 44× faster | 13× faster |
| get 4 KiB | 13.4 | 0.35 | 38× faster | 23× faster |
| range 64 KiB of 64 MiB | 11.5 | 0.32 | 36× faster | 34× faster |
| list 200 keys | 33.8 | 1.1 | 30× faster | 9.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.2 | 0.53 | 23× faster | 5.3× faster |
| move dir 200 × 64 KiB | 451 | 23.5 | 19× faster | 18× faster |
| get 1 MiB | 12.8 | 1.0 | 13× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 12.7 | 1.0 | 12× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 11.9 | 1.0 | 12× faster | 2.6× faster |
| append 4 KiB to 64 MiB | 291 | 35.4 | 8.2× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 297 | 36.3 | 8.2× faster | 13× faster |
| write at 4 KiB in 64 MiB | 298 | 36.9 | 8.1× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 286 | 37.5 | 7.6× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 287 | 38.6 | 7.4× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 280 | 38.8 | 7.2× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 272 | 41.7 | 6.5× faster | 11× faster |
| rename 64 MiB | 120 | 24.2 | 5.0× faster | 7.9× faster |
| insert 4 KiB, start of 32 MiB | 152 | 35.2 | 4.3× faster | 3.5× faster |
| append 4 KiB to 32 MiB | 150 | 35.7 | 4.2× faster | 8.0× faster |
| truncate 4 KiB, end of 32 MiB | 144 | 36.6 | 3.9× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 142 | 36.8 | 3.9× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 142 | 36.9 | 3.8× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 150 | 39.1 | 3.8× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 144 | 45.8 | 3.1× faster | 4.8× faster |
| stream get 256 MiB | 547 | 190 | 2.9× faster | 16× faster |
| stream get 64 MiB | 126 | 48.8 | 2.6× faster | 15× faster |
| get 32 MiB | 65.9 | 26.0 | 2.5× faster | 12× faster |
| get 64 MiB | 116 | 49.3 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 295 | 130 | 2.3× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 137 | 103 | 1.3× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,244 | 1,213 | 1.0× faster | 1.8× slower |
| append 4 KiB to 1 MiB | 27.9 | 35.1 | 1.3× slower | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 27.1 | 36.0 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 27.3 | 36.3 | 1.3× slower | 1.7× slower |
| write at 4 KiB in 1 MiB | 27.1 | 37.0 | 1.4× slower | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 26.6 | 37.0 | 1.4× slower | 2.1× slower |
| delete 4 KiB, start of 1 MiB | 26.2 | 36.7 | 1.4× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 26.7 | 37.9 | 1.4× slower | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | 26.5 | 46.7 | 1.8× slower | 1.5× slower |
| put 32 MiB | 87.5 | 190 | 2.2× slower | 1.2× slower |
| overwrite 1 MiB | 13.7 | 31.7 | 2.3× slower | 1.9× slower |
| put 64 MiB | 162 | 376 | 2.3× slower | 1.1× slower |
| multipart put 64 MiB × 8 MiB | 189 | 447 | 2.4× slower | 2.4× slower |
| overwrite 4 KiB | 13.7 | 33.0 | 2.4× slower | 3.1× slower |
| put 1 MiB | 13.3 | 32.2 | 2.4× slower | 1.7× slower |
| put 4 KiB | 13.2 | 34.1 | 2.6× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.6 | 36.4 | 2.7× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.3 | 34.9 | 2.8× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 11.7 | 35.9 | 3.1× slower | 2.2× slower |
