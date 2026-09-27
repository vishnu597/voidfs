# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260927T202315Z` |
| When | 2026-09-27T20:23:15Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | c434fcb (with uncommitted changes) |
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
| head | 12.1 | 0.25 | 48× faster | 13× faster |
| range 64 KiB of 64 MiB | 11.9 | 0.28 | 42× faster | 34× faster |
| get 4 KiB | 12.2 | 0.36 | 34× faster | 23× faster |
| list 200 keys | 38.8 | 1.1 | 34× faster | 9.1× faster |
| get 1 MiB | 13.4 | 1.5 | 9.1× faster | 4.5× faster |
| move dir 200 × 64 KiB | 460 | 95.3 | 4.8× faster | 18× faster |
| insert 4 KiB, start of 64 MiB | 298 | 90.7 | 3.3× faster | 6.8× faster |
| append 4 KiB to 64 MiB | 296 | 92.4 | 3.2× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 287 | 91.7 | 3.1× faster | 13× faster |
| delete 4 KiB, middle of 64 MiB | 291 | 94.9 | 3.1× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 277 | 91.7 | 3.0× faster | 11× faster |
| stream get 256 MiB | 583 | 193 | 3.0× faster | 16× faster |
| delete 4 KiB, start of 64 MiB | 266 | 89.9 | 3.0× faster | 6.9× faster |
| write at 4 KiB in 64 MiB | 288 | 97.9 | 2.9× faster | 6.2× faster |
| stream get 64 MiB | 131 | 49.7 | 2.6× faster | 15× faster |
| get 64 MiB | 122 | 53.5 | 2.3× faster | 17× faster |
| delete 4 KiB, start of 32 MiB | 147 | 89.3 | 1.7× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 146 | 91.2 | 1.6× faster | 8.0× faster |
| delete 4 KiB, middle of 32 MiB | 149 | 94.9 | 1.6× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 141 | 91.5 | 1.5× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 138 | 91.5 | 1.5× faster | 5.9× faster |
| patch 16 × 4 KiB in 64 MiB | 287 | 192 | 1.5× faster | 2.1× faster |
| write at 4 KiB in 32 MiB | 142 | 95.3 | 1.5× faster | 5.1× faster |
| rename 64 MiB | 141 | 95.9 | 1.5× faster | 7.9× faster |
| insert 4 KiB, start of 32 MiB | 138 | 96.4 | 1.4× faster | 3.5× faster |
| get 32 MiB | 66.6 | 58.7 | 1.1× faster | 12× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.5 | 12.4 | 1.0× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.0 | 12.4 | 1.0× slower | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 12.5 | 13.4 | 1.1× slower | 6.2× faster |
| patch 16 × 4 KiB in 32 MiB | 135 | 145 | 1.1× slower | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 254 | 408 | 1.6× slower | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 625 | 1,212 | 1.9× slower | 1.8× slower |
| put 32 MiB | 90.3 | 188 | 2.1× slower | 1.2× slower |
| put 64 MiB | 168 | 378 | 2.2× slower | 1.1× slower |
| insert 4 KiB, middle of 1 MiB | 27.5 | 92.5 | 3.4× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 27.3 | 92.6 | 3.4× slower | 1.0× faster |
| truncate 4 KiB, end of 1 MiB | 27.2 | 93.6 | 3.4× slower | 2.1× slower |
| patch 16 × 4 KiB in 1 MiB | 26.4 | 91.2 | 3.5× slower | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 26.8 | 93.4 | 3.5× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 26.4 | 93.4 | 3.5× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 26.3 | 93.5 | 3.6× slower | 1.6× slower |
| insert 4 KiB, start of 1 MiB | 27.0 | 96.8 | 3.6× slower | 1.7× slower |
| overwrite 1 MiB | 13.5 | 85.9 | 6.3× slower | 1.9× slower |
| put 1 MiB | 13.4 | 85.4 | 6.4× slower | 1.7× slower |
| put 4 KiB | 12.7 | 104 | 8.2× slower | 2.0× slower |
| overwrite 4 KiB | 11.8 | 101 | 8.6× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.0 | 368 | 28× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 412 | 34× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.3 | 859 | 70× slower | 2.4× slower |
