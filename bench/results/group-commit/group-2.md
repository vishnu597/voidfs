# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T192225Z` |
| When | 2026-09-28T19:22:25Z |
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

voidfs is faster in **26 of 49** scenarios and slower in the other **23**. Geometric mean speed-up over the bare bucket: **2.1×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.0 | 0.22 | 55× faster | 13× faster |
| get 4 KiB | 13.7 | 0.28 | 50× faster | 23× faster |
| range 64 KiB of 64 MiB | 13.0 | 0.30 | 44× faster | 34× faster |
| list 200 keys | 40.4 | 1.0 | 39× faster | 9.1× faster |
| move dir 200 × 64 KiB | 479 | 25.9 | 18× faster | 18× faster |
| get 1 MiB | 13.9 | 1.3 | 11× faster | 4.5× faster |
| append 4 KiB to 64 MiB | 290 | 34.6 | 8.4× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 257 | 36.3 | 7.1× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 318 | 45.2 | 7.0× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 277 | 40.9 | 6.8× faster | 13× faster |
| insert 4 KiB, middle of 64 MiB | 271 | 47.3 | 5.7× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 264 | 46.3 | 5.7× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 276 | 49.6 | 5.6× faster | 11× faster |
| rename 64 MiB | 131 | 24.0 | 5.5× faster | 7.9× faster |
| append 4 KiB to 32 MiB | 141 | 33.7 | 4.2× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 151 | 37.1 | 4.1× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 151 | 42.0 | 3.6× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 153 | 44.4 | 3.4× faster | 4.6× faster |
| truncate 4 KiB, end of 32 MiB | 143 | 45.0 | 3.2× faster | 5.9× faster |
| stream get 256 MiB | 553 | 180 | 3.1× faster | 16× faster |
| insert 4 KiB, middle of 32 MiB | 149 | 50.5 | 3.0× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 144 | 51.5 | 2.8× faster | 4.8× faster |
| stream get 64 MiB | 137 | 51.8 | 2.6× faster | 15× faster |
| get 64 MiB | 122 | 51.1 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 284 | 211 | 1.3× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 138 | 137 | 1.0× faster | 1.4× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.0 | 12.5 | 1.0× slower | 2.6× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.2 | 12.7 | 1.0× slower | 5.3× faster |
| get 32 MiB | 60.8 | 65.7 | 1.1× slower | 12× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.1 | 14.3 | 1.1× slower | 6.2× faster |
| write at 4 KiB in 1 MiB | 27.0 | 34.2 | 1.3× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 26.9 | 34.2 | 1.3× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 27.5 | 35.2 | 1.3× slower | 1.5× slower |
| append 4 KiB to 1 MiB | 27.2 | 35.2 | 1.3× slower | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 27.3 | 35.6 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 27.3 | 35.9 | 1.3× slower | 1.7× slower |
| truncate 4 KiB, end of 1 MiB | 26.2 | 34.5 | 1.3× slower | 2.1× slower |
| patch 16 × 4 KiB in 1 MiB | 27.9 | 44.9 | 1.6× slower | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 243 | 414 | 1.7× slower | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 658 | 1,199 | 1.8× slower | 1.8× slower |
| put 4 KiB | 13.1 | 27.3 | 2.1× slower | 2.0× slower |
| put 64 MiB | 166 | 373 | 2.3× slower | 1.1× slower |
| put 32 MiB | 86.4 | 196 | 2.3× slower | 1.2× slower |
| overwrite 1 MiB | 14.1 | 32.1 | 2.3× slower | 1.9× slower |
| put 1 MiB | 13.5 | 32.3 | 2.4× slower | 1.7× slower |
| overwrite 4 KiB | 12.8 | 31.4 | 2.5× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.9 | 36.7 | 2.8× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.3 | 36.7 | 3.0× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.0 | 36.2 | 3.0× slower | 2.2× slower |
