# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T204209Z` |
| When | 2026-09-28T20:42:09Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 634267e with this branch's changes (shard-cache) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | release build of the shard-cache branch (cargo build --release -p voidfs-server), s3: store in that bucket, 512 MiB shard cache, least recently used first |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.6×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.5 | 0.27 | 47× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.1 | 0.33 | 40× faster | 34× faster |
| list 200 keys | 39.4 | 1.1 | 36× faster | 9.1× faster |
| get 4 KiB | 13.7 | 0.42 | 33× faster | 23× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.6 | 0.48 | 26× faster | 5.3× faster |
| move dir 200 × 64 KiB | 455 | 26.8 | 17× faster | 18× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.9 | 1.1 | 13× faster | 6.2× faster |
| get 1 MiB | 14.7 | 1.2 | 13× faster | 4.5× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.0 | 0.99 | 12× faster | 2.6× faster |
| delete 4 KiB, middle of 64 MiB | 298 | 38.0 | 7.8× faster | 11× faster |
| write at 4 KiB in 64 MiB | 288 | 37.2 | 7.8× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 291 | 38.2 | 7.6× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 276 | 37.4 | 7.4× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 271 | 37.0 | 7.3× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 290 | 39.7 | 7.3× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 286 | 40.9 | 7.0× faster | 11× faster |
| rename 64 MiB | 137 | 26.0 | 5.2× faster | 7.9× faster |
| delete 4 KiB, start of 32 MiB | 151 | 35.9 | 4.2× faster | 4.6× faster |
| write at 4 KiB in 32 MiB | 148 | 36.3 | 4.1× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 156 | 38.3 | 4.1× faster | 4.8× faster |
| append 4 KiB to 32 MiB | 140 | 36.5 | 3.8× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 138 | 37.7 | 3.7× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 155 | 42.8 | 3.6× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 133 | 37.8 | 3.5× faster | 5.9× faster |
| stream get 256 MiB | 589 | 187 | 3.1× faster | 16× faster |
| stream get 64 MiB | 136 | 49.5 | 2.7× faster | 15× faster |
| get 32 MiB | 67.6 | 25.6 | 2.6× faster | 12× faster |
| get 64 MiB | 126 | 54.1 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 286 | 132 | 2.2× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 143 | 93.5 | 1.5× faster | 1.4× faster |
| delete 4 KiB, middle of 1 MiB | 28.2 | 36.1 | 1.3× slower | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 27.3 | 35.8 | 1.3× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 26.8 | 35.1 | 1.3× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 26.7 | 35.7 | 1.3× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 26.6 | 35.6 | 1.3× slower | 1.6× slower |
| append 4 KiB to 1 MiB | 27.2 | 37.1 | 1.4× slower | 1.0× faster |
| insert 4 KiB, start of 1 MiB | 26.2 | 37.4 | 1.4× slower | 1.7× slower |
| multipart put 64 MiB × 8 MiB | 285 | 449 | 1.6× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 27.3 | 45.2 | 1.7× slower | 1.5× slower |
| multipart put 256 MiB × 16 MiB | 657 | 1,255 | 1.9× slower | 1.8× slower |
| put 32 MiB | 89.1 | 193 | 2.2× slower | 1.2× slower |
| put 64 MiB | 164 | 372 | 2.3× slower | 1.1× slower |
| overwrite 1 MiB | 14.0 | 32.0 | 2.3× slower | 1.9× slower |
| put 1 MiB | 13.6 | 32.0 | 2.3× slower | 1.7× slower |
| put 4 KiB | 12.0 | 30.5 | 2.5× slower | 2.0× slower |
| overwrite 4 KiB | 11.7 | 30.8 | 2.6× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.8 | 36.0 | 2.8× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 11.9 | 36.2 | 3.0× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.4 | 37.8 | 3.0× slower | 2.4× slower |
