# voidfs against a bare bucket, cold

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T021837Z` |
| When | 2026-10-01T02:18:37Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bandwidth to the bucket | capped by that relay (--bandwidth s3): 95 MB/s down and 68 MB/s up per connection, 1000 MB/s in all each way |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 202c38a (with uncommitted changes) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4 --bandwidth s3` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 10 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Cold | Each round ran in waves of one operation per worker, each wave once the one before had ended; voidfs-server's shard and page caches were dropped before each of its waves. The bare bucket's own caches were not |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/), with their cache cleared: given for four rows |

voidfs is faster in **7 of 10** scenarios and slower in the other **3**. Geometric mean speed-up over the bare bucket: **3.0×**; over the 4 rows SpaceFS gives a figure for, 3.1× (SpaceFS's: 1.4×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result, cache cleared |
|---|--:|--:|---|---|
| get 4 KiB | 13.9 | 0.27 | 52× faster | 1.1× slower |
| fanout get 1000 × 4 KiB, 32 at once | 14.5 | 0.40 | 36× faster | – |
| fanout get 1000 × 4 KiB, 64 at once | 15.5 | 0.63 | 25× faster | – |
| stream get 64 MiB | 724 | 478 | 1.5× faster | – |
| stream get 256 MiB | 2,840 | 1,926 | 1.5× faster | – |
| get 64 MiB | 728 | 517 | 1.4× faster | 2.6× faster |
| get 32 MiB | 372 | 269 | 1.4× faster | 2.0× faster |
| get 1 MiB | 25.0 | 25.8 | 1.0× slower | 1.2× slower |
| fanout get 200 × 256 KiB, 32 at once | 17.4 | 18.2 | 1.0× slower | – |
| range 64 KiB of 64 MiB | 14.6 | 43.5 | 3.0× slower | – |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | get |
|---|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 1.02 | 1.02 |
| get 4 KiB | 400 | 0.00 |  |
| get 64 MiB | 64 | 26.00 | 26.00 |
| stream get 256 MiB | 32 | 95.62 | 95.62 |
| stream get 64 MiB | 64 | 21.56 | 21.56 |
| get 32 MiB | 64 | 11.23 | 11.23 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 1.00 | 1.00 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |
| get 1 MiB | 400 | 1.00 | 1.00 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |
