# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T020752Z` |
| When | 2026-09-28T02:07:52Z |
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

voidfs is faster in **25 of 49** scenarios and slower in the other **24**. Geometric mean speed-up over the bare bucket: **1.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.2 | 0.23 | 58× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.6 | 0.33 | 42× faster | 34× faster |
| list 200 keys | 39.9 | 1.1 | 37× faster | 9.1× faster |
| get 4 KiB | 13.6 | 0.38 | 36× faster | 23× faster |
| get 1 MiB | 14.0 | 1.4 | 9.9× faster | 4.5× faster |
| move dir 200 × 64 KiB | 459 | 105 | 4.4× faster | 18× faster |
| delete 4 KiB, start of 64 MiB | 288 | 95.0 | 3.0× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 290 | 98.1 | 3.0× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 282 | 95.3 | 3.0× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 277 | 95.3 | 2.9× faster | 6.8× faster |
| stream get 256 MiB | 566 | 196 | 2.9× faster | 16× faster |
| stream get 64 MiB | 136 | 48.4 | 2.8× faster | 15× faster |
| write at 4 KiB in 64 MiB | 275 | 97.6 | 2.8× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 266 | 96.1 | 2.8× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 281 | 105 | 2.7× faster | 11× faster |
| get 64 MiB | 124 | 52.3 | 2.4× faster | 17× faster |
| write at 4 KiB in 32 MiB | 160 | 94.0 | 1.7× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 156 | 97.3 | 1.6× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 153 | 96.6 | 1.6× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 141 | 96.7 | 1.5× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 135 | 94.8 | 1.4× faster | 4.8× faster |
| patch 16 × 4 KiB in 64 MiB | 274 | 193 | 1.4× faster | 2.1× faster |
| append 4 KiB to 32 MiB | 136 | 98.5 | 1.4× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 127 | 94.1 | 1.4× faster | 3.5× faster |
| rename 64 MiB | 131 | 108 | 1.2× faster | 7.9× faster |
| patch 16 × 4 KiB in 32 MiB | 146 | 147 | 1.0× slower | 1.4× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.2 | 12.7 | 1.0× slower | 5.3× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.5 | 14.2 | 1.1× slower | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 13.2 | 1.1× slower | 2.6× faster |
| get 32 MiB | 66.8 | 72.8 | 1.1× slower | 12× faster |
| multipart put 64 MiB × 8 MiB | 291 | 407 | 1.4× slower | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 789 | 1,190 | 1.5× slower | 1.8× slower |
| put 32 MiB | 90.0 | 190 | 2.1× slower | 1.2× slower |
| put 64 MiB | 163 | 377 | 2.3× slower | 1.1× slower |
| append 4 KiB to 1 MiB | 28.6 | 97.2 | 3.4× slower | 1.0× faster |
| patch 16 × 4 KiB in 1 MiB | 26.9 | 91.9 | 3.4× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 27.0 | 93.1 | 3.5× slower | 2.1× slower |
| delete 4 KiB, start of 1 MiB | 28.2 | 99.2 | 3.5× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 27.7 | 98.3 | 3.5× slower | 1.7× slower |
| write at 4 KiB in 1 MiB | 27.4 | 97.8 | 3.6× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 27.3 | 98.1 | 3.6× slower | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 27.2 | 98.2 | 3.6× slower | 1.4× slower |
| put 1 MiB | 14.1 | 87.0 | 6.2× slower | 1.7× slower |
| overwrite 1 MiB | 13.8 | 85.2 | 6.2× slower | 1.9× slower |
| put 4 KiB | 12.9 | 104 | 8.1× slower | 2.0× slower |
| overwrite 4 KiB | 12.7 | 105 | 8.3× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.9 | 368 | 29× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 413 | 34× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.1 | 837 | 69× slower | 2.4× slower |
