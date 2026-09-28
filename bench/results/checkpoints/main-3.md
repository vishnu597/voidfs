# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T143559Z` |
| When | 2026-09-28T14:35:59Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | f6154da (with uncommitted changes) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **26 of 49** scenarios and slower in the other **23**. Geometric mean speed-up over the bare bucket: **1.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| get 4 KiB | 14.1 | 0.33 | 43× faster | 23× faster |
| head | 13.0 | 0.30 | 43× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.8 | 0.33 | 39× faster | 34× faster |
| list 200 keys | 41.4 | 1.1 | 38× faster | 9.1× faster |
| get 1 MiB | 14.0 | 1.4 | 9.8× faster | 4.5× faster |
| move dir 200 × 64 KiB | 463 | 104 | 4.4× faster | 18× faster |
| insert 4 KiB, start of 64 MiB | 295 | 96.3 | 3.1× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 289 | 94.9 | 3.0× faster | 13× faster |
| delete 4 KiB, start of 64 MiB | 300 | 99.0 | 3.0× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 282 | 95.3 | 3.0× faster | 11× faster |
| stream get 256 MiB | 573 | 195 | 2.9× faster | 16× faster |
| insert 4 KiB, middle of 64 MiB | 276 | 95.6 | 2.9× faster | 11× faster |
| append 4 KiB to 64 MiB | 286 | 100 | 2.9× faster | 15× faster |
| write at 4 KiB in 64 MiB | 269 | 98.9 | 2.7× faster | 6.2× faster |
| stream get 64 MiB | 133 | 49.5 | 2.7× faster | 15× faster |
| get 64 MiB | 118 | 51.1 | 2.3× faster | 17× faster |
| delete 4 KiB, middle of 32 MiB | 152 | 90.8 | 1.7× faster | 4.8× faster |
| insert 4 KiB, start of 32 MiB | 148 | 91.5 | 1.6× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 153 | 95.0 | 1.6× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 138 | 89.0 | 1.6× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 143 | 95.3 | 1.5× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 135 | 91.2 | 1.5× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 141 | 98.7 | 1.4× faster | 8.0× faster |
| get 32 MiB | 67.7 | 55.3 | 1.2× faster | 12× faster |
| rename 64 MiB | 134 | 110 | 1.2× faster | 7.9× faster |
| patch 16 × 4 KiB in 32 MiB | 136 | 124 | 1.1× faster | 1.4× faster |
| fanout get 1000 × 4 KiB, 32 at once | 11.8 | 12.4 | 1.1× slower | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 11.9 | 12.6 | 1.1× slower | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 14.6 | 15.8 | 1.1× slower | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 286 | 348 | 1.2× slower | 2.1× faster |
| multipart put 64 MiB × 8 MiB | 294 | 402 | 1.4× slower | 2.4× slower |
| put 32 MiB | 90.8 | 186 | 2.1× slower | 1.2× slower |
| multipart put 256 MiB × 16 MiB | 587 | 1,206 | 2.1× slower | 1.8× slower |
| put 64 MiB | 165 | 371 | 2.3× slower | 1.1× slower |
| delete 4 KiB, start of 1 MiB | 27.5 | 91.3 | 3.3× slower | 1.5× slower |
| patch 16 × 4 KiB in 1 MiB | 27.5 | 92.0 | 3.4× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 27.3 | 93.4 | 3.4× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 27.3 | 94.1 | 3.4× slower | 1.7× slower |
| write at 4 KiB in 1 MiB | 27.2 | 94.0 | 3.5× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 26.8 | 93.2 | 3.5× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 27.6 | 96.7 | 3.5× slower | 2.1× slower |
| append 4 KiB to 1 MiB | 26.4 | 92.7 | 3.5× slower | 1.0× faster |
| overwrite 1 MiB | 13.6 | 85.2 | 6.3× slower | 1.9× slower |
| put 1 MiB | 13.5 | 85.4 | 6.3× slower | 1.7× slower |
| put 4 KiB | 12.8 | 102 | 8.0× slower | 2.0× slower |
| overwrite 4 KiB | 12.5 | 105 | 8.4× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.3 | 369 | 28× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 413 | 34× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 11.9 | 831 | 70× slower | 2.4× slower |
