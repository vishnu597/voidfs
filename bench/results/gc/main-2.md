# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T014502Z` |
| When | 2026-09-28T01:45:02Z |
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
| head | 13.1 | 0.27 | 48× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.2 | 0.32 | 42× faster | 34× faster |
| list 200 keys | 42.1 | 1.0 | 41× faster | 9.1× faster |
| get 4 KiB | 14.1 | 0.40 | 35× faster | 23× faster |
| get 1 MiB | 14.1 | 1.2 | 12× faster | 4.5× faster |
| move dir 200 × 64 KiB | 465 | 104 | 4.5× faster | 18× faster |
| delete 4 KiB, start of 64 MiB | 303 | 95.2 | 3.2× faster | 6.9× faster |
| insert 4 KiB, start of 64 MiB | 291 | 93.0 | 3.1× faster | 6.8× faster |
| insert 4 KiB, middle of 64 MiB | 295 | 96.7 | 3.0× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 286 | 98.8 | 2.9× faster | 11× faster |
| stream get 256 MiB | 566 | 197 | 2.9× faster | 16× faster |
| append 4 KiB to 64 MiB | 281 | 98.6 | 2.9× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 271 | 95.2 | 2.8× faster | 13× faster |
| write at 4 KiB in 64 MiB | 265 | 94.4 | 2.8× faster | 6.2× faster |
| stream get 64 MiB | 134 | 52.0 | 2.6× faster | 15× faster |
| get 64 MiB | 118 | 51.0 | 2.3× faster | 17× faster |
| insert 4 KiB, start of 32 MiB | 161 | 92.4 | 1.7× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 155 | 96.6 | 1.6× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 145 | 93.0 | 1.6× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 151 | 99.0 | 1.5× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 150 | 101 | 1.5× faster | 5.9× faster |
| patch 16 × 4 KiB in 64 MiB | 272 | 187 | 1.5× faster | 2.1× faster |
| append 4 KiB to 32 MiB | 136 | 96.5 | 1.4× faster | 8.0× faster |
| delete 4 KiB, middle of 32 MiB | 137 | 103 | 1.3× faster | 4.8× faster |
| rename 64 MiB | 129 | 104 | 1.2× faster | 7.9× faster |
| patch 16 × 4 KiB in 32 MiB | 134 | 142 | 1.1× slower | 1.4× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.5 | 13.3 | 1.1× slower | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 11.9 | 12.6 | 1.1× slower | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.2 | 15.2 | 1.1× slower | 6.2× faster |
| get 32 MiB | 62.9 | 80.3 | 1.3× slower | 12× faster |
| multipart put 256 MiB × 16 MiB | 889 | 1,233 | 1.4× slower | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 245 | 405 | 1.7× slower | 2.4× slower |
| put 32 MiB | 88.6 | 191 | 2.2× slower | 1.2× slower |
| put 64 MiB | 166 | 372 | 2.2× slower | 1.1× slower |
| append 4 KiB to 1 MiB | 27.1 | 91.5 | 3.4× slower | 1.0× faster |
| insert 4 KiB, start of 1 MiB | 26.7 | 90.3 | 3.4× slower | 1.7× slower |
| delete 4 KiB, start of 1 MiB | 26.9 | 91.2 | 3.4× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 27.6 | 95.4 | 3.5× slower | 2.1× slower |
| delete 4 KiB, middle of 1 MiB | 26.5 | 91.9 | 3.5× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 27.2 | 94.6 | 3.5× slower | 1.6× slower |
| patch 16 × 4 KiB in 1 MiB | 26.1 | 91.5 | 3.5× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 26.4 | 93.2 | 3.5× slower | 1.4× slower |
| overwrite 1 MiB | 13.9 | 85.4 | 6.1× slower | 1.9× slower |
| put 1 MiB | 13.5 | 86.0 | 6.4× slower | 1.7× slower |
| put 4 KiB | 12.7 | 104 | 8.2× slower | 2.0× slower |
| overwrite 4 KiB | 12.0 | 105 | 8.7× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.8 | 378 | 29× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 11.8 | 411 | 35× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.0 | 830 | 69× slower | 2.4× slower |
