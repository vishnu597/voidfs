# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T212742Z` |
| When | 2026-09-29T21:27:42Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **2.6×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.5 | 0.28 | 44× faster | 13× faster |
| get 4 KiB | 13.3 | 0.38 | 35× faster | 23× faster |
| range 64 KiB of 64 MiB | 11.9 | 0.35 | 34× faster | 34× faster |
| list 200 keys | 33.0 | 1.2 | 27× faster | 9.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.0 | 0.51 | 23× faster | 5.3× faster |
| move dir 200 × 64 KiB | 455 | 23.5 | 19× faster | 18× faster |
| get 1 MiB | 13.0 | 1.0 | 12× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.1 | 1.1 | 12× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.1 | 1.0 | 12× faster | 2.6× faster |
| truncate 4 KiB, end of 64 MiB | 292 | 35.8 | 8.2× faster | 13× faster |
| append 4 KiB to 64 MiB | 279 | 35.3 | 7.9× faster | 15× faster |
| insert 4 KiB, middle of 64 MiB | 297 | 38.4 | 7.8× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 297 | 40.1 | 7.4× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 288 | 39.6 | 7.3× faster | 6.9× faster |
| write at 4 KiB in 64 MiB | 269 | 37.6 | 7.2× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 283 | 45.9 | 6.2× faster | 11× faster |
| rename 64 MiB | 123 | 23.9 | 5.1× faster | 7.9× faster |
| append 4 KiB to 32 MiB | 144 | 33.3 | 4.3× faster | 8.0× faster |
| truncate 4 KiB, end of 32 MiB | 142 | 34.2 | 4.2× faster | 5.9× faster |
| insert 4 KiB, middle of 32 MiB | 152 | 36.6 | 4.1× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 135 | 35.6 | 3.8× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 145 | 39.2 | 3.7× faster | 4.8× faster |
| write at 4 KiB in 32 MiB | 150 | 40.6 | 3.7× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 145 | 39.5 | 3.7× faster | 3.5× faster |
| stream get 256 MiB | 556 | 198 | 2.8× faster | 16× faster |
| stream get 64 MiB | 135 | 49.8 | 2.7× faster | 15× faster |
| get 32 MiB | 65.9 | 25.4 | 2.6× faster | 12× faster |
| get 64 MiB | 123 | 51.6 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 270 | 136 | 2.0× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 148 | 104 | 1.4× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,107 | 913 | 1.2× faster | 1.8× slower |
| put 64 MiB | 167 | 209 | 1.3× slower | 1.1× slower |
| truncate 4 KiB, end of 1 MiB | 26.8 | 34.6 | 1.3× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 26.3 | 34.1 | 1.3× slower | 1.7× slower |
| put 32 MiB | 91.3 | 119 | 1.3× slower | 1.2× slower |
| append 4 KiB to 1 MiB | 26.9 | 35.9 | 1.3× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 26.4 | 35.6 | 1.3× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 26.9 | 36.2 | 1.3× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 26.8 | 36.1 | 1.3× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 27.1 | 36.8 | 1.4× slower | 1.6× slower |
| patch 16 × 4 KiB in 1 MiB | 26.5 | 45.7 | 1.7× slower | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 226 | 425 | 1.9× slower | 2.4× slower |
| put 1 MiB | 13.6 | 32.5 | 2.4× slower | 1.7× slower |
| overwrite 1 MiB | 13.5 | 32.5 | 2.4× slower | 1.9× slower |
| put 4 KiB | 13.2 | 32.6 | 2.5× slower | 2.0× slower |
| overwrite 4 KiB | 12.5 | 34.8 | 2.8× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.5 | 37.5 | 3.0× slower | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.6 | 37.7 | 3.0× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 36.9 | 3.1× slower | 2.2× slower |
