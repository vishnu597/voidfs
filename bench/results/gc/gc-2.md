# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T023153Z` |
| When | 2026-09-28T02:31:53Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | a9ea745 (with uncommitted changes) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **27 of 49** scenarios and slower in the other **22**. Geometric mean speed-up over the bare bucket: **1.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.8 | 0.28 | 48× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.2 | 0.35 | 38× faster | 34× faster |
| get 4 KiB | 13.8 | 0.38 | 36× faster | 23× faster |
| list 200 keys | 34.2 | 1.00 | 34× faster | 9.1× faster |
| get 1 MiB | 14.7 | 1.3 | 11× faster | 4.5× faster |
| move dir 200 × 64 KiB | 481 | 103 | 4.7× faster | 18× faster |
| append 4 KiB to 64 MiB | 297 | 93.5 | 3.2× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 301 | 95.5 | 3.2× faster | 13× faster |
| write at 4 KiB in 64 MiB | 288 | 97.7 | 2.9× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 290 | 98.7 | 2.9× faster | 6.9× faster |
| stream get 256 MiB | 573 | 197 | 2.9× faster | 16× faster |
| insert 4 KiB, start of 64 MiB | 273 | 94.7 | 2.9× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 294 | 102 | 2.9× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 275 | 96.7 | 2.8× faster | 11× faster |
| stream get 64 MiB | 134 | 51.1 | 2.6× faster | 15× faster |
| get 64 MiB | 121 | 51.9 | 2.3× faster | 17× faster |
| multipart put 256 MiB × 16 MiB | 2,096 | 1,243 | 1.7× faster | 1.8× slower |
| rename 64 MiB | 169 | 105 | 1.6× faster | 7.9× faster |
| insert 4 KiB, middle of 32 MiB | 151 | 94.6 | 1.6× faster | 3.8× faster |
| write at 4 KiB in 32 MiB | 150 | 98.9 | 1.5× faster | 5.1× faster |
| truncate 4 KiB, end of 32 MiB | 144 | 97.2 | 1.5× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 145 | 98.1 | 1.5× faster | 8.0× faster |
| delete 4 KiB, middle of 32 MiB | 148 | 102 | 1.5× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 136 | 96.6 | 1.4× faster | 4.6× faster |
| patch 16 × 4 KiB in 64 MiB | 284 | 205 | 1.4× faster | 2.1× faster |
| insert 4 KiB, start of 32 MiB | 135 | 97.7 | 1.4× faster | 3.5× faster |
| patch 16 × 4 KiB in 32 MiB | 145 | 134 | 1.1× faster | 1.4× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 12.9 | 1.1× slower | 2.6× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.2 | 13.3 | 1.1× slower | 5.3× faster |
| get 32 MiB | 67.8 | 78.7 | 1.2× slower | 12× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.4 | 16.0 | 1.2× slower | 6.2× faster |
| multipart put 64 MiB × 8 MiB | 257 | 402 | 1.6× slower | 2.4× slower |
| put 32 MiB | 90.5 | 188 | 2.1× slower | 1.2× slower |
| put 64 MiB | 161 | 370 | 2.3× slower | 1.1× slower |
| insert 4 KiB, middle of 1 MiB | 29.5 | 101 | 3.4× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 28.5 | 98.1 | 3.4× slower | 1.5× slower |
| patch 16 × 4 KiB in 1 MiB | 26.9 | 92.5 | 3.4× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 28.3 | 98.6 | 3.5× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 27.7 | 98.1 | 3.5× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 28.0 | 100 | 3.6× slower | 1.0× faster |
| truncate 4 KiB, end of 1 MiB | 27.5 | 98.6 | 3.6× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 27.9 | 101 | 3.6× slower | 1.7× slower |
| overwrite 1 MiB | 14.6 | 87.0 | 6.0× slower | 1.9× slower |
| put 1 MiB | 14.2 | 86.8 | 6.1× slower | 1.7× slower |
| put 4 KiB | 14.9 | 111 | 7.4× slower | 2.0× slower |
| overwrite 4 KiB | 13.6 | 109 | 8.0× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.2 | 395 | 30× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.0 | 434 | 36× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.3 | 867 | 70× slower | 2.4× slower |
