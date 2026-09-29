# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T140910Z` |
| When | 2026-09-29T14:09:10Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | acfb616 (with uncommitted changes) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.6×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.6 | 0.25 | 50× faster | 13× faster |
| get 4 KiB | 12.8 | 0.30 | 42× faster | 23× faster |
| range 64 KiB of 64 MiB | 12.3 | 0.31 | 40× faster | 34× faster |
| list 200 keys | 35.3 | 1.1 | 32× faster | 9.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.3 | 0.53 | 23× faster | 5.3× faster |
| move dir 200 × 64 KiB | 456 | 24.2 | 19× faster | 18× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.1 | 1.0 | 13× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 11.8 | 0.98 | 12× faster | 2.6× faster |
| get 1 MiB | 12.4 | 1.2 | 11× faster | 4.5× faster |
| append 4 KiB to 64 MiB | 308 | 33.5 | 9.2× faster | 15× faster |
| write at 4 KiB in 64 MiB | 297 | 35.0 | 8.5× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 291 | 35.1 | 8.3× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 287 | 35.4 | 8.1× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 293 | 38.9 | 7.5× faster | 13× faster |
| insert 4 KiB, middle of 64 MiB | 304 | 40.5 | 7.5× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 294 | 41.5 | 7.1× faster | 11× faster |
| rename 64 MiB | 127 | 24.6 | 5.2× faster | 7.9× faster |
| write at 4 KiB in 32 MiB | 152 | 35.6 | 4.3× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 152 | 36.7 | 4.2× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 142 | 35.1 | 4.1× faster | 5.9× faster |
| insert 4 KiB, middle of 32 MiB | 153 | 38.2 | 4.0× faster | 3.8× faster |
| append 4 KiB to 32 MiB | 145 | 36.4 | 4.0× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 145 | 36.9 | 3.9× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 152 | 39.8 | 3.8× faster | 4.8× faster |
| stream get 256 MiB | 597 | 191 | 3.1× faster | 16× faster |
| stream get 64 MiB | 133 | 52.8 | 2.5× faster | 15× faster |
| get 64 MiB | 118 | 48.3 | 2.4× faster | 17× faster |
| get 32 MiB | 60.7 | 27.1 | 2.2× faster | 12× faster |
| patch 16 × 4 KiB in 64 MiB | 283 | 135 | 2.1× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 131 | 101 | 1.3× faster | 1.4× faster |
| insert 4 KiB, middle of 1 MiB | 26.6 | 34.1 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 26.6 | 34.3 | 1.3× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 27.5 | 36.6 | 1.3× slower | 1.0× faster |
| truncate 4 KiB, end of 1 MiB | 26.9 | 36.2 | 1.3× slower | 2.1× slower |
| delete 4 KiB, start of 1 MiB | 27.0 | 36.3 | 1.3× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 27.4 | 37.1 | 1.4× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 27.2 | 36.9 | 1.4× slower | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | 27.1 | 45.8 | 1.7× slower | 1.5× slower |
| multipart put 256 MiB × 16 MiB | 591 | 1,259 | 2.1× slower | 1.8× slower |
| put 32 MiB | 91.0 | 195 | 2.1× slower | 1.2× slower |
| overwrite 1 MiB | 14.0 | 32.0 | 2.3× slower | 1.9× slower |
| put 64 MiB | 164 | 380 | 2.3× slower | 1.1× slower |
| put 1 MiB | 13.6 | 32.2 | 2.4× slower | 1.7× slower |
| put 4 KiB | 13.0 | 31.8 | 2.4× slower | 2.0× slower |
| multipart put 64 MiB × 8 MiB | 181 | 449 | 2.5× slower | 2.4× slower |
| overwrite 4 KiB | 12.3 | 33.1 | 2.7× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.2 | 36.0 | 2.7× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.1 | 36.5 | 3.0× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.0 | 36.5 | 3.1× slower | 2.2× slower |
