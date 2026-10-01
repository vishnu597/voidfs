# voidfs against a bare bucket, cold

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T135143Z` |
| When | 2026-10-01T13:51:43Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 12b68e9 |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 10 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Cold | Each round ran in waves of one operation per worker, each wave once the one before had ended; voidfs-server's shard and page caches were dropped before each of its waves. The bare bucket's own caches were not |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/), with their cache cleared: given for four rows |

voidfs is faster in **3 of 10** scenarios and slower in the other **7**. Geometric mean speed-up over the bare bucket: **2.5×**; over the 4 rows SpaceFS gives a figure for, 2.3× (SpaceFS's: 1.4×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result, cache cleared |
|---|--:|--:|---|---|
| get 4 KiB | 12.9 | 0.27 | 47× faster | 1.1× slower |
| fanout get 1000 × 4 KiB, 32 at once | 13.5 | 0.39 | 34× faster | – |
| fanout get 1000 × 4 KiB, 64 at once | 14.6 | 0.57 | 25× faster | – |
| fanout get 200 × 256 KiB, 32 at once | 14.2 | 14.4 | 1.0× slower | – |
| get 1 MiB | 13.4 | 14.6 | 1.1× slower | 1.2× slower |
| stream get 256 MiB | 537 | 623 | 1.2× slower | – |
| get 64 MiB | 136 | 159 | 1.2× slower | 2.6× faster |
| get 32 MiB | 64.5 | 79.1 | 1.2× slower | 2.0× faster |
| range 64 KiB of 64 MiB | 13.0 | 17.4 | 1.3× slower | – |
| stream get 64 MiB | 125 | 220 | 1.8× slower | – |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | get |
|---|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 1.02 | 1.02 |
| get 4 KiB | 400 | 0.00 |  |
| get 64 MiB | 64 | 27.58 | 27.58 |
| stream get 256 MiB | 32 | 100.31 | 100.31 |
| stream get 64 MiB | 64 | 18.42 | 18.42 |
| get 32 MiB | 64 | 12.95 | 12.95 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 1.00 | 1.00 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |
| get 1 MiB | 400 | 2.00 | 2.00 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |
