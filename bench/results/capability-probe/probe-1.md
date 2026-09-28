# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T154523Z` |
| When | 2026-09-28T15:45:23Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | a068b2c (with uncommitted changes) |
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
| head | 12.2 | 0.23 | 52× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.2 | 0.32 | 38× faster | 34× faster |
| get 4 KiB | 12.9 | 0.34 | 38× faster | 23× faster |
| list 200 keys | 39.7 | 1.1 | 35× faster | 9.1× faster |
| get 1 MiB | 12.9 | 1.5 | 8.9× faster | 4.5× faster |
| move dir 200 × 64 KiB | 439 | 93.8 | 4.7× faster | 18× faster |
| append 4 KiB to 64 MiB | 301 | 88.8 | 3.4× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 298 | 90.0 | 3.3× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 300 | 94.4 | 3.2× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 302 | 96.7 | 3.1× faster | 6.9× faster |
| write at 4 KiB in 64 MiB | 276 | 89.2 | 3.1× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 281 | 93.5 | 3.0× faster | 11× faster |
| stream get 256 MiB | 526 | 177 | 3.0× faster | 16× faster |
| truncate 4 KiB, end of 64 MiB | 269 | 92.3 | 2.9× faster | 13× faster |
| stream get 64 MiB | 129 | 46.2 | 2.8× faster | 15× faster |
| get 64 MiB | 117 | 48.4 | 2.4× faster | 17× faster |
| write at 4 KiB in 32 MiB | 151 | 92.8 | 1.6× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 149 | 94.3 | 1.6× faster | 3.5× faster |
| delete 4 KiB, middle of 32 MiB | 147 | 94.4 | 1.6× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 140 | 90.6 | 1.5× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 141 | 92.3 | 1.5× faster | 8.0× faster |
| insert 4 KiB, middle of 32 MiB | 142 | 94.6 | 1.5× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 134 | 102 | 1.3× faster | 5.9× faster |
| rename 64 MiB | 122 | 96.9 | 1.3× faster | 7.9× faster |
| patch 16 × 4 KiB in 64 MiB | 266 | 234 | 1.1× faster | 2.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.5 | 12.2 | 1.0× faster | 5.3× faster |
| get 32 MiB | 63.8 | 62.3 | 1.0× faster | 12× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.1 | 12.4 | 1.0× slower | 2.6× faster |
| patch 16 × 4 KiB in 32 MiB | 129 | 143 | 1.1× slower | 1.4× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.1 | 15.1 | 1.2× slower | 6.2× faster |
| multipart put 64 MiB × 8 MiB | 273 | 401 | 1.5× slower | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 673 | 1,175 | 1.7× slower | 1.8× slower |
| put 32 MiB | 89.2 | 183 | 2.1× slower | 1.2× slower |
| put 64 MiB | 160 | 369 | 2.3× slower | 1.1× slower |
| delete 4 KiB, middle of 1 MiB | 28.0 | 90.1 | 3.2× slower | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | 26.9 | 89.0 | 3.3× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 27.2 | 91.8 | 3.4× slower | 1.7× slower |
| truncate 4 KiB, end of 1 MiB | 26.8 | 91.9 | 3.4× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 27.0 | 93.0 | 3.4× slower | 1.6× slower |
| append 4 KiB to 1 MiB | 27.2 | 94.0 | 3.5× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 26.5 | 92.2 | 3.5× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 26.5 | 92.9 | 3.5× slower | 1.5× slower |
| overwrite 1 MiB | 13.6 | 85.2 | 6.3× slower | 1.9× slower |
| put 1 MiB | 13.1 | 84.8 | 6.5× slower | 1.7× slower |
| overwrite 4 KiB | 13.5 | 103 | 7.6× slower | 3.1× slower |
| put 4 KiB | 12.9 | 99.2 | 7.7× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.5 | 373 | 30× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 399 | 33× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 11.9 | 798 | 67× slower | 2.4× slower |
