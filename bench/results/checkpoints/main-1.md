# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T140524Z` |
| When | 2026-09-28T14:05:24Z |
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

voidfs is faster in **25 of 49** scenarios and slower in the other **24**. Geometric mean speed-up over the bare bucket: **1.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.8 | 0.26 | 49× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.0 | 0.31 | 39× faster | 34× faster |
| get 4 KiB | 12.4 | 0.34 | 37× faster | 23× faster |
| list 200 keys | 35.6 | 1.1 | 33× faster | 9.1× faster |
| get 1 MiB | 14.0 | 1.4 | 9.7× faster | 4.5× faster |
| move dir 200 × 64 KiB | 451 | 95.7 | 4.7× faster | 18× faster |
| delete 4 KiB, middle of 64 MiB | 297 | 91.6 | 3.2× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 295 | 91.9 | 3.2× faster | 6.8× faster |
| append 4 KiB to 64 MiB | 279 | 88.5 | 3.2× faster | 15× faster |
| delete 4 KiB, start of 64 MiB | 295 | 96.0 | 3.1× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 283 | 93.1 | 3.0× faster | 13× faster |
| insert 4 KiB, middle of 64 MiB | 270 | 97.2 | 2.8× faster | 11× faster |
| stream get 256 MiB | 493 | 179 | 2.8× faster | 16× faster |
| stream get 64 MiB | 118 | 46.6 | 2.5× faster | 15× faster |
| write at 4 KiB in 64 MiB | 240 | 96.0 | 2.5× faster | 6.2× faster |
| get 64 MiB | 104 | 47.8 | 2.2× faster | 17× faster |
| write at 4 KiB in 32 MiB | 153 | 92.2 | 1.7× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 145 | 89.3 | 1.6× faster | 3.5× faster |
| append 4 KiB to 32 MiB | 144 | 89.4 | 1.6× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 146 | 94.2 | 1.6× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 143 | 92.3 | 1.6× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 143 | 94.8 | 1.5× faster | 5.9× faster |
| delete 4 KiB, middle of 32 MiB | 139 | 96.3 | 1.4× faster | 4.8× faster |
| patch 16 × 4 KiB in 64 MiB | 272 | 209 | 1.3× faster | 2.1× faster |
| rename 64 MiB | 120 | 96.1 | 1.2× faster | 7.9× faster |
| patch 16 × 4 KiB in 32 MiB | 147 | 148 | 1.0× slower | 1.4× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.1 | 12.4 | 1.0× slower | 5.3× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.3 | 13.8 | 1.0× slower | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.0 | 12.7 | 1.1× slower | 2.6× faster |
| get 32 MiB | 60.6 | 70.3 | 1.2× slower | 12× faster |
| multipart put 256 MiB × 16 MiB | 889 | 1,217 | 1.4× slower | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 259 | 437 | 1.7× slower | 2.4× slower |
| put 32 MiB | 88.0 | 191 | 2.2× slower | 1.2× slower |
| put 64 MiB | 170 | 379 | 2.2× slower | 1.1× slower |
| patch 16 × 4 KiB in 1 MiB | 29.9 | 90.6 | 3.0× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 29.1 | 100 | 3.5× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 27.7 | 98.9 | 3.6× slower | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 27.9 | 102 | 3.6× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 27.8 | 102 | 3.6× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 27.2 | 101 | 3.7× slower | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 27.4 | 102 | 3.7× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 27.7 | 104 | 3.7× slower | 1.5× slower |
| put 1 MiB | 14.0 | 86.7 | 6.2× slower | 1.7× slower |
| overwrite 1 MiB | 13.7 | 86.9 | 6.3× slower | 1.9× slower |
| overwrite 4 KiB | 14.3 | 112 | 7.8× slower | 3.1× slower |
| put 4 KiB | 13.7 | 111 | 8.1× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.5 | 392 | 31× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 446 | 37× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.8 | 901 | 70× slower | 2.4× slower |
