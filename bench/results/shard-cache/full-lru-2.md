# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T204501Z` |
| When | 2026-09-28T20:45:01Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 634267e with this branch's changes (shard-cache) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | release build of the shard-cache branch (cargo build --release -p voidfs-server), s3: store in that bucket, 512 MiB shard cache, least recently used first |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.6×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.1 | 0.24 | 50× faster | 13× faster |
| range 64 KiB of 64 MiB | 11.9 | 0.26 | 45× faster | 34× faster |
| list 200 keys | 39.7 | 1.1 | 35× faster | 9.1× faster |
| get 4 KiB | 12.7 | 0.38 | 34× faster | 23× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.0 | 0.49 | 24× faster | 5.3× faster |
| move dir 200 × 64 KiB | 452 | 24.1 | 19× faster | 18× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.1 | 0.93 | 13× faster | 2.6× faster |
| get 1 MiB | 13.2 | 1.1 | 12× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.2 | 1.1 | 12× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 307 | 33.9 | 9.1× faster | 15× faster |
| delete 4 KiB, start of 64 MiB | 278 | 36.3 | 7.6× faster | 6.9× faster |
| write at 4 KiB in 64 MiB | 289 | 38.1 | 7.6× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 267 | 35.3 | 7.6× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 283 | 37.4 | 7.5× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 286 | 38.5 | 7.4× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 288 | 43.9 | 6.6× faster | 11× faster |
| rename 64 MiB | 122 | 23.9 | 5.1× faster | 7.9× faster |
| delete 4 KiB, start of 32 MiB | 153 | 39.4 | 3.9× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 143 | 37.1 | 3.9× faster | 8.0× faster |
| insert 4 KiB, middle of 32 MiB | 139 | 37.3 | 3.7× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 131 | 35.4 | 3.7× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 142 | 39.9 | 3.6× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 140 | 40.7 | 3.4× faster | 3.5× faster |
| delete 4 KiB, middle of 32 MiB | 134 | 43.1 | 3.1× faster | 4.8× faster |
| stream get 256 MiB | 606 | 205 | 3.0× faster | 16× faster |
| stream get 64 MiB | 138 | 50.1 | 2.8× faster | 15× faster |
| get 32 MiB | 66.3 | 25.2 | 2.6× faster | 12× faster |
| get 64 MiB | 126 | 55.9 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 287 | 143 | 2.0× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 143 | 98.2 | 1.5× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,021 | 1,266 | 1.2× slower | 1.8× slower |
| truncate 4 KiB, end of 1 MiB | 27.3 | 34.4 | 1.3× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 27.2 | 35.1 | 1.3× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 26.9 | 35.5 | 1.3× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 27.1 | 35.8 | 1.3× slower | 1.0× faster |
| insert 4 KiB, start of 1 MiB | 26.5 | 36.0 | 1.4× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 26.3 | 36.4 | 1.4× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 26.0 | 36.9 | 1.4× slower | 1.5× slower |
| patch 16 × 4 KiB in 1 MiB | 25.9 | 45.3 | 1.7× slower | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 220 | 443 | 2.0× slower | 2.4× slower |
| put 32 MiB | 85.8 | 191 | 2.2× slower | 1.2× slower |
| put 4 KiB | 12.3 | 27.9 | 2.3× slower | 2.0× slower |
| overwrite 4 KiB | 11.9 | 27.2 | 2.3× slower | 3.1× slower |
| put 64 MiB | 161 | 373 | 2.3× slower | 1.1× slower |
| put 1 MiB | 13.7 | 32.1 | 2.3× slower | 1.7× slower |
| overwrite 1 MiB | 13.6 | 32.4 | 2.4× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.8 | 37.0 | 2.9× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.3 | 36.7 | 3.0× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.0 | 36.6 | 3.1× slower | 2.2× slower |
