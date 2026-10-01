# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T020238Z` |
| When | 2026-10-01T02:02:38Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| bandwidth to the bucket | capped by that relay (--bandwidth s3): 95 MB/s down and 68 MB/s up per connection, 1000 MB/s in all each way |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 202c38a (with uncommitted changes) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4 --bandwidth s3` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 31 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| range 64 KiB of 64 MiB | 13.0 | – | – | 34× faster |
| get 4 KiB | 12.8 | – | – | 23× faster |
| get 64 MiB | 730 | – | – | 17× faster |
| stream get 256 MiB | 2,840 | – | – | 16× faster |
| stream get 64 MiB | 722 | – | – | 15× faster |
| append 4 KiB to 64 MiB | 1,754 | – | – | 15× faster |
| head | 12.4 | – | – | 13× faster |
| truncate 4 KiB, end of 64 MiB | 1,754 | – | – | 13× faster |
| get 32 MiB | 371 | – | – | 12× faster |
| delete 4 KiB, middle of 64 MiB | 1,751 | – | – | 11× faster |
| insert 4 KiB, middle of 64 MiB | 1,753 | – | – | 11× faster |
| append 4 KiB to 32 MiB | 893 | – | – | 8.0× faster |
| rename 64 MiB | 118 | – | – | 7.9× faster |
| delete 4 KiB, start of 64 MiB | 1,754 | – | – | 6.9× faster |
| insert 4 KiB, start of 64 MiB | 1,755 | – | – | 6.8× faster |
| write at 4 KiB in 64 MiB | 1,755 | – | – | 6.2× faster |
| truncate 4 KiB, end of 32 MiB | 892 | – | – | 5.9× faster |
| write at 4 KiB in 32 MiB | 894 | – | – | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 895 | – | – | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 893 | – | – | 4.6× faster |
| get 1 MiB | 22.9 | – | – | 4.5× faster |
| insert 4 KiB, middle of 32 MiB | 894 | – | – | 3.8× faster |
| insert 4 KiB, start of 32 MiB | 896 | – | – | 3.5× faster |
| patch 16 × 4 KiB in 64 MiB | 1,754 | – | – | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 892 | – | – | 1.4× faster |
| put 64 MiB | 1,024 | – | – | 1.1× slower |
| put 32 MiB | 519 | – | – | 1.2× slower |
| put 1 MiB | 28.3 | – | – | 1.7× slower |
| multipart put 256 MiB × 16 MiB | 2,022 | – | – | 1.8× slower |
| put 4 KiB | 14.7 | – | – | 2.0× slower |
| multipart put 64 MiB × 8 MiB | 639 | – | – | 2.4× slower |
