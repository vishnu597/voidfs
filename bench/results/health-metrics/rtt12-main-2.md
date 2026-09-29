# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T141441Z` |
| When | 2026-09-29T14:14:41Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **2.6×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.5 | 0.24 | 52× faster | 13× faster |
| get 4 KiB | 12.8 | 0.31 | 41× faster | 23× faster |
| range 64 KiB of 64 MiB | 12.0 | 0.32 | 37× faster | 34× faster |
| list 200 keys | 34.2 | 1.1 | 30× faster | 9.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.1 | 0.50 | 24× faster | 5.3× faster |
| move dir 200 × 64 KiB | 435 | 23.5 | 19× faster | 18× faster |
| fanout get 200 × 256 KiB, 32 at once | 12.9 | 0.99 | 13× faster | 6.2× faster |
| get 1 MiB | 13.7 | 1.2 | 11× faster | 4.5× faster |
| fanout get 1000 × 4 KiB, 64 at once | 11.9 | 1.2 | 10× faster | 2.6× faster |
| delete 4 KiB, start of 64 MiB | 285 | 33.4 | 8.5× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 289 | 34.5 | 8.4× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 304 | 36.4 | 8.4× faster | 6.8× faster |
| append 4 KiB to 64 MiB | 272 | 33.6 | 8.1× faster | 15× faster |
| delete 4 KiB, middle of 64 MiB | 298 | 38.6 | 7.7× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 291 | 38.8 | 7.5× faster | 11× faster |
| write at 4 KiB in 64 MiB | 254 | 36.9 | 6.9× faster | 6.2× faster |
| rename 64 MiB | 121 | 24.1 | 5.0× faster | 7.9× faster |
| append 4 KiB to 32 MiB | 158 | 33.7 | 4.7× faster | 8.0× faster |
| truncate 4 KiB, end of 32 MiB | 150 | 35.5 | 4.2× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 142 | 34.5 | 4.1× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 141 | 37.3 | 3.8× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 150 | 41.6 | 3.6× faster | 4.8× faster |
| insert 4 KiB, start of 32 MiB | 142 | 40.7 | 3.5× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 144 | 42.3 | 3.4× faster | 3.8× faster |
| stream get 256 MiB | 541 | 176 | 3.1× faster | 16× faster |
| stream get 64 MiB | 125 | 48.9 | 2.6× faster | 15× faster |
| get 32 MiB | 67.6 | 28.2 | 2.4× faster | 12× faster |
| get 64 MiB | 120 | 50.3 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 262 | 132 | 2.0× faster | 2.1× faster |
| multipart put 256 MiB × 16 MiB | 2,185 | 1,245 | 1.8× faster | 1.8× slower |
| patch 16 × 4 KiB in 32 MiB | 144 | 103 | 1.4× faster | 1.4× faster |
| truncate 4 KiB, end of 1 MiB | 28.1 | 35.6 | 1.3× slower | 2.1× slower |
| insert 4 KiB, middle of 1 MiB | 26.8 | 34.8 | 1.3× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 26.8 | 35.8 | 1.3× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 26.9 | 36.0 | 1.3× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 26.5 | 36.1 | 1.4× slower | 1.0× faster |
| write at 4 KiB in 1 MiB | 26.3 | 36.4 | 1.4× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 26.3 | 37.4 | 1.4× slower | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | 26.0 | 47.0 | 1.8× slower | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 208 | 458 | 2.2× slower | 2.4× slower |
| put 32 MiB | 84.3 | 189 | 2.2× slower | 1.2× slower |
| put 64 MiB | 163 | 367 | 2.3× slower | 1.1× slower |
| put 1 MiB | 14.2 | 32.5 | 2.3× slower | 1.7× slower |
| overwrite 1 MiB | 13.9 | 31.9 | 2.3× slower | 1.9× slower |
| overwrite 4 KiB | 12.2 | 30.3 | 2.5× slower | 3.1× slower |
| put 4 KiB | 12.6 | 32.8 | 2.6× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.6 | 37.0 | 2.9× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 36.5 | 3.0× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.1 | 36.5 | 3.0× slower | 2.4× slower |
