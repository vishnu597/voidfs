# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T192520Z` |
| When | 2026-09-28T19:25:20Z |
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

voidfs is faster in **29 of 49** scenarios and slower in the other **20**. Geometric mean speed-up over the bare bucket: **1.1×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.7 | 0.26 | 50× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.4 | 0.30 | 42× faster | 34× faster |
| get 4 KiB | 12.7 | 0.34 | 37× faster | 23× faster |
| list 200 keys | 42.2 | 1.2 | 34× faster | 9.1× faster |
| get 1 MiB | 13.1 | 1.2 | 11× faster | 4.5× faster |
| move dir 200 × 64 KiB | 452 | 96.3 | 4.7× faster | 18× faster |
| insert 4 KiB, middle of 64 MiB | 308 | 96.6 | 3.2× faster | 11× faster |
| append 4 KiB to 64 MiB | 301 | 95.8 | 3.1× faster | 15× faster |
| write at 4 KiB in 64 MiB | 291 | 93.9 | 3.1× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 288 | 93.1 | 3.1× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 297 | 98.1 | 3.0× faster | 13× faster |
| delete 4 KiB, middle of 64 MiB | 283 | 94.1 | 3.0× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 279 | 93.1 | 3.0× faster | 6.9× faster |
| stream get 256 MiB | 618 | 214 | 2.9× faster | 16× faster |
| stream get 64 MiB | 135 | 53.0 | 2.5× faster | 15× faster |
| get 64 MiB | 131 | 53.3 | 2.5× faster | 17× faster |
| multipart put 256 MiB × 16 MiB | 2,262 | 1,201 | 1.9× faster | 1.8× slower |
| truncate 4 KiB, end of 32 MiB | 150 | 91.4 | 1.6× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 148 | 90.6 | 1.6× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 152 | 94.0 | 1.6× faster | 4.8× faster |
| insert 4 KiB, start of 32 MiB | 142 | 90.4 | 1.6× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 139 | 90.0 | 1.5× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 139 | 92.7 | 1.5× faster | 8.0× faster |
| get 32 MiB | 74.0 | 52.2 | 1.4× faster | 12× faster |
| insert 4 KiB, middle of 32 MiB | 142 | 101 | 1.4× faster | 3.8× faster |
| patch 16 × 4 KiB in 64 MiB | 281 | 222 | 1.3× faster | 2.1× faster |
| rename 64 MiB | 119 | 97.7 | 1.2× faster | 7.9× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.3 | 12.9 | 1.0× faster | 6.2× faster |
| patch 16 × 4 KiB in 32 MiB | 146 | 146 | 1.0× faster | 1.4× faster |
| fanout get 1000 × 4 KiB, 64 at once | 11.9 | 12.3 | 1.0× slower | 2.6× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.1 | 12.6 | 1.0× slower | 5.3× faster |
| multipart put 64 MiB × 8 MiB | 289 | 405 | 1.4× slower | 2.4× slower |
| put 32 MiB | 96.3 | 190 | 2.0× slower | 1.2× slower |
| put 64 MiB | 163 | 374 | 2.3× slower | 1.1× slower |
| write at 4 KiB in 1 MiB | 27.3 | 91.3 | 3.3× slower | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 28.0 | 94.1 | 3.4× slower | 2.1× slower |
| insert 4 KiB, middle of 1 MiB | 27.3 | 93.7 | 3.4× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 27.2 | 93.5 | 3.4× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 26.6 | 91.4 | 3.4× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 27.1 | 93.9 | 3.5× slower | 1.0× faster |
| patch 16 × 4 KiB in 1 MiB | 26.3 | 91.1 | 3.5× slower | 1.5× slower |
| delete 4 KiB, start of 1 MiB | 27.2 | 94.3 | 3.5× slower | 1.5× slower |
| overwrite 1 MiB | 13.6 | 87.7 | 6.5× slower | 1.9× slower |
| put 1 MiB | 13.5 | 87.3 | 6.5× slower | 1.7× slower |
| overwrite 4 KiB | 13.3 | 103 | 7.7× slower | 3.1× slower |
| put 4 KiB | 12.3 | 102 | 8.3× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.9 | 378 | 27× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 11.9 | 410 | 34× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.1 | 818 | 68× slower | 2.4× slower |
