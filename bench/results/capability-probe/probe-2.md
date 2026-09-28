# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T155008Z` |
| When | 2026-09-28T15:50:08Z |
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
| head | 12.1 | 0.23 | 52× faster | 13× faster |
| get 4 KiB | 12.2 | 0.27 | 45× faster | 23× faster |
| range 64 KiB of 64 MiB | 11.9 | 0.27 | 44× faster | 34× faster |
| list 200 keys | 36.5 | 1.0 | 35× faster | 9.1× faster |
| get 1 MiB | 13.6 | 1.3 | 11× faster | 4.5× faster |
| move dir 200 × 64 KiB | 438 | 95.2 | 4.6× faster | 18× faster |
| insert 4 KiB, start of 64 MiB | 287 | 90.0 | 3.2× faster | 6.8× faster |
| append 4 KiB to 64 MiB | 282 | 91.4 | 3.1× faster | 15× faster |
| write at 4 KiB in 64 MiB | 272 | 88.3 | 3.1× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 280 | 92.3 | 3.0× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 285 | 95.1 | 3.0× faster | 11× faster |
| truncate 4 KiB, end of 64 MiB | 263 | 90.3 | 2.9× faster | 13× faster |
| stream get 256 MiB | 535 | 184 | 2.9× faster | 16× faster |
| delete 4 KiB, start of 64 MiB | 258 | 91.9 | 2.8× faster | 6.9× faster |
| stream get 64 MiB | 128 | 47.0 | 2.7× faster | 15× faster |
| get 64 MiB | 120 | 50.5 | 2.4× faster | 17× faster |
| insert 4 KiB, start of 32 MiB | 150 | 93.9 | 1.6× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 146 | 93.8 | 1.6× faster | 5.9× faster |
| delete 4 KiB, middle of 32 MiB | 147 | 94.7 | 1.6× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 141 | 92.8 | 1.5× faster | 3.8× faster |
| append 4 KiB to 32 MiB | 135 | 92.5 | 1.5× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 139 | 95.6 | 1.5× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 137 | 95.0 | 1.4× faster | 4.6× faster |
| rename 64 MiB | 125 | 97.9 | 1.3× faster | 7.9× faster |
| patch 16 × 4 KiB in 64 MiB | 277 | 252 | 1.1× faster | 2.1× faster |
| get 32 MiB | 62.8 | 59.3 | 1.1× faster | 12× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 12.3 | 1.0× slower | 2.6× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.1 | 12.4 | 1.0× slower | 5.3× faster |
| patch 16 × 4 KiB in 32 MiB | 137 | 144 | 1.0× slower | 1.4× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.1 | 14.2 | 1.1× slower | 6.2× faster |
| multipart put 256 MiB × 16 MiB | 884 | 1,185 | 1.3× slower | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 238 | 412 | 1.7× slower | 2.4× slower |
| put 32 MiB | 86.6 | 189 | 2.2× slower | 1.2× slower |
| put 64 MiB | 165 | 372 | 2.2× slower | 1.1× slower |
| patch 16 × 4 KiB in 1 MiB | 27.5 | 88.4 | 3.2× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 26.9 | 90.8 | 3.4× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 27.7 | 93.7 | 3.4× slower | 1.6× slower |
| insert 4 KiB, start of 1 MiB | 26.7 | 90.9 | 3.4× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 27.1 | 93.6 | 3.5× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 26.7 | 92.5 | 3.5× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 26.5 | 92.8 | 3.5× slower | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 26.5 | 93.0 | 3.5× slower | 1.4× slower |
| put 1 MiB | 13.5 | 84.9 | 6.3× slower | 1.7× slower |
| overwrite 1 MiB | 13.4 | 85.1 | 6.3× slower | 1.9× slower |
| overwrite 4 KiB | 13.0 | 99.2 | 7.6× slower | 3.1× slower |
| put 4 KiB | 12.2 | 98.9 | 8.1× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.7 | 387 | 31× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 11.9 | 399 | 33× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.3 | 803 | 65× slower | 2.4× slower |
