# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T155454Z` |
| When | 2026-09-28T15:54:54Z |
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

voidfs is faster in **26 of 49** scenarios and slower in the other **23**. Geometric mean speed-up over the bare bucket: **1.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.2 | 0.25 | 48× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.0 | 0.27 | 45× faster | 34× faster |
| get 4 KiB | 12.2 | 0.29 | 42× faster | 23× faster |
| list 200 keys | 42.8 | 1.1 | 39× faster | 9.1× faster |
| get 1 MiB | 13.2 | 1.4 | 9.8× faster | 4.5× faster |
| move dir 200 × 64 KiB | 448 | 94.2 | 4.8× faster | 18× faster |
| stream get 256 MiB | 585 | 169 | 3.5× faster | 16× faster |
| delete 4 KiB, start of 64 MiB | 298 | 90.4 | 3.3× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 305 | 93.1 | 3.3× faster | 15× faster |
| delete 4 KiB, middle of 64 MiB | 292 | 91.8 | 3.2× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 279 | 94.8 | 2.9× faster | 11× faster |
| write at 4 KiB in 64 MiB | 272 | 93.1 | 2.9× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 261 | 90.6 | 2.9× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 268 | 94.6 | 2.8× faster | 6.8× faster |
| stream get 64 MiB | 140 | 50.0 | 2.8× faster | 15× faster |
| get 64 MiB | 119 | 49.2 | 2.4× faster | 17× faster |
| delete 4 KiB, start of 32 MiB | 151 | 90.6 | 1.7× faster | 4.6× faster |
| truncate 4 KiB, end of 32 MiB | 143 | 91.5 | 1.6× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 141 | 91.2 | 1.5× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 137 | 89.5 | 1.5× faster | 3.5× faster |
| append 4 KiB to 32 MiB | 147 | 96.7 | 1.5× faster | 8.0× faster |
| delete 4 KiB, middle of 32 MiB | 149 | 98.7 | 1.5× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 132 | 99.5 | 1.3× faster | 3.8× faster |
| rename 64 MiB | 121 | 96.7 | 1.3× faster | 7.9× faster |
| patch 16 × 4 KiB in 64 MiB | 270 | 238 | 1.1× faster | 2.1× faster |
| get 32 MiB | 62.2 | 56.5 | 1.1× faster | 12× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.4 | 12.7 | 1.0× slower | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 11.7 | 12.6 | 1.1× slower | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.5 | 14.5 | 1.1× slower | 6.2× faster |
| multipart put 256 MiB × 16 MiB | 1,042 | 1,130 | 1.1× slower | 1.8× slower |
| patch 16 × 4 KiB in 32 MiB | 125 | 144 | 1.1× slower | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 218 | 398 | 1.8× slower | 2.4× slower |
| put 32 MiB | 89.2 | 190 | 2.1× slower | 1.2× slower |
| put 64 MiB | 160 | 372 | 2.3× slower | 1.1× slower |
| delete 4 KiB, start of 1 MiB | 28.5 | 91.0 | 3.2× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 28.4 | 91.9 | 3.2× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 26.9 | 89.0 | 3.3× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 28.3 | 95.3 | 3.4× slower | 2.1× slower |
| delete 4 KiB, middle of 1 MiB | 26.8 | 92.9 | 3.5× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 27.1 | 95.6 | 3.5× slower | 1.0× faster |
| write at 4 KiB in 1 MiB | 26.9 | 95.2 | 3.5× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 26.3 | 93.4 | 3.6× slower | 1.4× slower |
| overwrite 1 MiB | 13.6 | 84.9 | 6.2× slower | 1.9× slower |
| put 1 MiB | 13.3 | 85.0 | 6.4× slower | 1.7× slower |
| put 4 KiB | 12.9 | 98.9 | 7.7× slower | 2.0× slower |
| overwrite 4 KiB | 12.6 | 105 | 8.3× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.7 | 376 | 30× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.2 | 399 | 33× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.1 | 795 | 66× slower | 2.4× slower |
