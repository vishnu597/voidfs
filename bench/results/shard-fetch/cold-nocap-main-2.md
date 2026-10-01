# voidfs against a bare bucket, cold

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T135044Z` |
| When | 2026-10-01T13:50:44Z |
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

voidfs is faster in **3 of 10** scenarios and slower in the other **7**. Geometric mean speed-up over the bare bucket: **2.7×**; over the 4 rows SpaceFS gives a figure for, 2.4× (SpaceFS's: 1.4×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result, cache cleared |
|---|--:|--:|---|---|
| get 4 KiB | 13.1 | 0.24 | 54× faster | 1.1× slower |
| fanout get 1000 × 4 KiB, 32 at once | 13.7 | 0.37 | 37× faster | – |
| fanout get 1000 × 4 KiB, 64 at once | 14.8 | 0.58 | 25× faster | – |
| fanout get 200 × 256 KiB, 32 at once | 14.0 | 14.2 | 1.0× slower | – |
| get 1 MiB | 14.0 | 14.5 | 1.0× slower | 1.2× slower |
| stream get 256 MiB | 545 | 571 | 1.0× slower | – |
| get 64 MiB | 134 | 159 | 1.2× slower | 2.6× faster |
| stream get 64 MiB | 127 | 154 | 1.2× slower | – |
| range 64 KiB of 64 MiB | 13.0 | 16.9 | 1.3× slower | – |
| get 32 MiB | 63.9 | 84.3 | 1.3× slower | 2.0× faster |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | get |
|---|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 1.01 | 1.01 |
| get 4 KiB | 400 | 0.00 |  |
| get 64 MiB | 64 | 20.97 | 20.97 |
| stream get 256 MiB | 32 | 99.22 | 99.22 |
| stream get 64 MiB | 64 | 23.83 | 23.83 |
| get 32 MiB | 64 | 13.92 | 13.92 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 1.00 | 1.00 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |
| get 1 MiB | 400 | 2.00 | 2.00 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |
