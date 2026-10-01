# voidfs against a bare bucket, cold

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T133144Z` |
| When | 2026-10-01T13:31:44Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bandwidth to the bucket | capped by that relay (--bandwidth s3): 95 MB/s down and 68 MB/s up per connection, 1000 MB/s in all each way |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 12b68e9 |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4 --bandwidth s3` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 10 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Cold | Each round ran in waves of one operation per worker, each wave once the one before had ended; voidfs-server's shard and page caches were dropped before each of its waves. The bare bucket's own caches were not |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/), with their cache cleared: given for four rows |

voidfs is faster in **7 of 10** scenarios and slower in the other **3**. Geometric mean speed-up over the bare bucket: **4.8×**; over the 4 rows SpaceFS gives a figure for, 5.5× (SpaceFS's: 1.4×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result, cache cleared |
|---|--:|--:|---|---|
| get 4 KiB | 12.7 | 0.28 | 45× faster | 1.1× slower |
| fanout get 1000 × 4 KiB, 32 at once | 13.4 | 0.39 | 35× faster | – |
| fanout get 1000 × 4 KiB, 64 at once | 14.6 | 0.56 | 26× faster | – |
| stream get 256 MiB | 2,840 | 424 | 6.7× faster | – |
| get 64 MiB | 727 | 145 | 5.0× faster | 2.6× faster |
| get 32 MiB | 371 | 86.7 | 4.3× faster | 2.0× faster |
| stream get 64 MiB | 720 | 190 | 3.8× faster | – |
| get 1 MiB | 24.1 | 24.8 | 1.0× slower | 1.2× slower |
| fanout get 200 × 256 KiB, 32 at once | 16.6 | 17.2 | 1.0× slower | – |
| range 64 KiB of 64 MiB | 13.7 | 41.6 | 3.0× slower | – |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | get |
|---|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.93 | 0.93 |
| get 4 KiB | 400 | 0.00 |  |
| get 64 MiB | 64 | 3.88 | 3.88 |
| stream get 256 MiB | 32 | 15.12 | 15.12 |
| stream get 64 MiB | 64 | 4.00 | 4.00 |
| get 32 MiB | 64 | 1.88 | 1.88 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 1.00 | 1.00 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |
| get 1 MiB | 400 | 0.13 | 0.13 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |
