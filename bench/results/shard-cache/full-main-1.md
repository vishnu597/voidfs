# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T203915Z` |
| When | 2026-09-28T20:39:15Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 634267e (main) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | release build of main 634267e (cargo build --release -p voidfs-server), s3: store in that bucket, 512 MiB shard cache, moka's default admission (TinyLFU) |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **25 of 49** scenarios and slower in the other **24**. Geometric mean speed-up over the bare bucket: **2.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.4 | 0.31 | 44× faster | 13× faster |
| get 4 KiB | 14.1 | 0.34 | 42× faster | 23× faster |
| list 200 keys | 42.6 | 1.1 | 40× faster | 9.1× faster |
| range 64 KiB of 64 MiB | 13.7 | 0.35 | 39× faster | 34× faster |
| move dir 200 × 64 KiB | 462 | 27.0 | 17× faster | 18× faster |
| get 1 MiB | 14.6 | 1.2 | 12× faster | 4.5× faster |
| append 4 KiB to 64 MiB | 287 | 36.6 | 7.8× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 301 | 41.7 | 7.2× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 280 | 40.0 | 7.0× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 293 | 43.4 | 6.7× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 292 | 47.3 | 6.2× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 293 | 47.7 | 6.1× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 238 | 48.9 | 4.9× faster | 11× faster |
| rename 64 MiB | 124 | 26.6 | 4.7× faster | 7.9× faster |
| append 4 KiB to 32 MiB | 155 | 33.9 | 4.6× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 133 | 37.9 | 3.5× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 151 | 43.0 | 3.5× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 154 | 44.3 | 3.5× faster | 4.6× faster |
| stream get 256 MiB | 563 | 187 | 3.0× faster | 16× faster |
| insert 4 KiB, middle of 32 MiB | 147 | 53.8 | 2.7× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 136 | 50.2 | 2.7× faster | 4.8× faster |
| stream get 64 MiB | 131 | 49.1 | 2.7× faster | 15× faster |
| truncate 4 KiB, end of 32 MiB | 129 | 49.8 | 2.6× faster | 5.9× faster |
| get 64 MiB | 118 | 53.0 | 2.2× faster | 17× faster |
| get 32 MiB | 66.3 | 56.4 | 1.2× faster | 12× faster |
| patch 16 × 4 KiB in 32 MiB | 137 | 138 | 1.0× slower | 1.4× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.4 | 13.3 | 1.1× slower | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 13.3 | 1.1× slower | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.9 | 15.1 | 1.1× slower | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 286 | 320 | 1.1× slower | 2.1× faster |
| truncate 4 KiB, end of 1 MiB | 28.3 | 35.9 | 1.3× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 28.3 | 37.2 | 1.3× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 26.8 | 35.6 | 1.3× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 28.6 | 38.2 | 1.3× slower | 1.0× faster |
| delete 4 KiB, start of 1 MiB | 28.5 | 38.0 | 1.3× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 27.2 | 36.8 | 1.4× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 27.8 | 37.8 | 1.4× slower | 1.4× slower |
| multipart put 64 MiB × 8 MiB | 314 | 445 | 1.4× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 29.4 | 45.5 | 1.5× slower | 1.5× slower |
| multipart put 256 MiB × 16 MiB | 687 | 1,197 | 1.7× slower | 1.8× slower |
| put 32 MiB | 92.5 | 191 | 2.1× slower | 1.2× slower |
| put 4 KiB | 13.4 | 29.5 | 2.2× slower | 2.0× slower |
| overwrite 4 KiB | 12.9 | 29.4 | 2.3× slower | 3.1× slower |
| put 64 MiB | 162 | 371 | 2.3× slower | 1.1× slower |
| overwrite 1 MiB | 14.2 | 32.4 | 2.3× slower | 1.9× slower |
| put 1 MiB | 13.8 | 32.7 | 2.4× slower | 1.7× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.3 | 35.9 | 2.9× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.5 | 36.9 | 3.0× slower | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.7 | 39.0 | 3.1× slower | 2.8× slower |
