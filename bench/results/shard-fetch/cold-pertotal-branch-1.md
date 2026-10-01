# voidfs against a bare bucket, cold

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T134118Z` |
| When | 2026-10-01T13:41:18Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bandwidth to the bucket | capped by that relay (--bandwidth 95/68/-): 95 MB/s down and 68 MB/s up per connection, unlimited in all each way |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 12b68e9 |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4 --bandwidth 95/68/-` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 10 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Cold | Each round ran in waves of one operation per worker, each wave once the one before had ended; voidfs-server's shard and page caches were dropped before each of its waves. The bare bucket's own caches were not |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/), with their cache cleared: given for four rows |

voidfs is faster in **7 of 10** scenarios and slower in the other **3**. Geometric mean speed-up over the bare bucket: **5.4×**; over the 4 rows SpaceFS gives a figure for, 6.1× (SpaceFS's: 1.4×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result, cache cleared |
|---|--:|--:|---|---|
| get 4 KiB | 13.7 | 0.24 | 57× faster | 1.1× slower |
| fanout get 1000 × 4 KiB, 32 at once | 14.6 | 0.38 | 38× faster | – |
| fanout get 1000 × 4 KiB, 64 at once | 15.5 | 0.57 | 27× faster | – |
| stream get 256 MiB | 2,841 | 384 | 7.4× faster | – |
| get 64 MiB | 732 | 104 | 7.1× faster | 2.6× faster |
| stream get 64 MiB | 722 | 103 | 7.0× faster | – |
| get 32 MiB | 374 | 105 | 3.5× faster | 2.0× faster |
| fanout get 200 × 256 KiB, 32 at once | 18.0 | 18.5 | 1.0× slower | – |
| get 1 MiB | 24.9 | 26.1 | 1.0× slower | 1.2× slower |
| range 64 KiB of 64 MiB | 15.0 | 48.6 | 3.2× slower | – |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | get |
|---|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.82 | 0.82 |
| get 4 KiB | 400 | 0.00 |  |
| get 64 MiB | 64 | 3.75 | 3.75 |
| stream get 256 MiB | 32 | 13.38 | 13.38 |
| stream get 64 MiB | 64 | 2.88 | 2.88 |
| get 32 MiB | 64 | 2.00 | 2.00 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 1.00 | 1.00 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |
| get 1 MiB | 400 | 0.13 | 0.13 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |
