# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T140333Z` |
| When | 2026-10-01T14:03:33Z |
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
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **40 of 49** scenarios and slower in the other **9**. Geometric mean speed-up over the bare bucket: **5.7×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.1 | 0.27 | 45× faster | 13× faster |
| list 200 keys | 42.1 | 0.99 | 43× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 1,751 | 43.1 | 41× faster | 13× faster |
| get 4 KiB | 12.6 | 0.33 | 38× faster | 23× faster |
| append 4 KiB to 64 MiB | 1,754 | 46.5 | 38× faster | 15× faster |
| range 64 KiB of 64 MiB | 12.1 | 0.34 | 35× faster | 34× faster |
| move dir 200 × 64 KiB | 462 | 13.8 | 33× faster | 18× faster |
| write at 4 KiB in 64 MiB | 1,753 | 65.0 | 27× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 1,757 | 67.9 | 26× faster | 6.9× faster |
| append 4 KiB to 32 MiB | 895 | 36.4 | 25× faster | 8.0× faster |
| insert 4 KiB, start of 64 MiB | 1,756 | 74.2 | 24× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 1,754 | 76.8 | 23× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 1,755 | 77.7 | 23× faster | 11× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.3 | 0.56 | 22× faster | 5.3× faster |
| truncate 4 KiB, end of 32 MiB | 893 | 52.9 | 17× faster | 5.9× faster |
| get 1 MiB | 23.3 | 1.4 | 17× faster | 4.5× faster |
| delete 4 KiB, start of 32 MiB | 895 | 59.0 | 15× faster | 4.6× faster |
| write at 4 KiB in 32 MiB | 894 | 60.9 | 15× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 897 | 62.7 | 14× faster | 3.8× faster |
| insert 4 KiB, start of 32 MiB | 895 | 64.8 | 14× faster | 3.5× faster |
| stream get 64 MiB | 721 | 52.8 | 14× faster | 15× faster |
| fanout get 200 × 256 KiB, 32 at once | 16.1 | 1.2 | 14× faster | 6.2× faster |
| get 64 MiB | 728 | 55.1 | 13× faster | 17× faster |
| stream get 256 MiB | 2,840 | 222 | 13× faster | 16× faster |
| delete 4 KiB, middle of 32 MiB | 892 | 70.1 | 13× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 64 at once | 13.0 | 1.0 | 13× faster | 2.6× faster |
| get 32 MiB | 372 | 31.7 | 12× faster | 12× faster |
| rename 64 MiB | 123 | 12.4 | 9.9× faster | 7.9× faster |
| patch 16 × 4 KiB in 64 MiB | 1,753 | 403 | 4.4× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 894 | 273 | 3.3× faster | 1.4× faster |
| put 64 MiB | 1,024 | 585 | 1.7× faster | 1.1× slower |
| put 32 MiB | 519 | 300 | 1.7× faster | 1.2× slower |
| insert 4 KiB, start of 1 MiB | 53.0 | 44.4 | 1.2× faster | 1.7× slower |
| write at 4 KiB in 1 MiB | 52.8 | 45.0 | 1.2× faster | 1.6× slower |
| append 4 KiB to 1 MiB | 53.5 | 46.4 | 1.2× faster | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 52.1 | 45.8 | 1.1× faster | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 52.0 | 46.2 | 1.1× faster | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 51.0 | 45.4 | 1.1× faster | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 52.2 | 47.6 | 1.1× faster | 2.1× slower |
| patch 16 × 4 KiB in 1 MiB | 51.5 | 47.7 | 1.1× faster | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 600 | 623 | 1.0× slower | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 2,098 | 2,183 | 1.0× slower | 1.8× slower |
| put 4 KiB | 14.4 | 16.8 | 1.2× slower | 2.0× slower |
| overwrite 4 KiB | 13.3 | 16.3 | 1.2× slower | 3.1× slower |
| overwrite 1 MiB | 27.8 | 44.0 | 1.6× slower | 1.9× slower |
| put 1 MiB | 28.5 | 45.0 | 1.6× slower | 1.7× slower |
| fanout put 1000 × 4 KiB, 64 at once | 13.1 | 22.6 | 1.7× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.5 | 26.9 | 2.1× slower | 2.2× slower |
| fanout put 200 × 256 KiB, 32 at once | 16.6 | 35.9 | 2.2× slower | 2.8× slower |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | delete_prefix | get | put | put_new |
|---|--:|--:|--:|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.00 |  |  |  |  |
| get 4 KiB | 400 | 0.00 |  |  |  |  |
| move dir 200 × 64 KiB | 32 | 0.19 |  |  |  | 0.19 |
| get 64 MiB | 64 | 0.00 |  |  |  |  |
| stream get 256 MiB | 32 | 0.00 |  |  |  |  |
| stream get 64 MiB | 64 | 0.00 |  |  |  |  |
| append 4 KiB to 64 MiB | 64 | 1.61 |  | 0.02 | 1.00 | 0.59 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.47 |  |  | 1.00 | 0.47 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.78 |  |  | 1.00 | 0.78 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.80 |  | 0.02 | 1.00 | 0.78 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.47 |  |  | 1.00 | 0.47 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.67 |  | 0.03 | 1.00 | 0.64 |
| insert 4 KiB, start of 64 MiB | 64 | 1.73 |  | 0.02 | 1.00 | 0.72 |
| write at 4 KiB in 64 MiB | 64 | 1.66 |  |  | 1.00 | 0.66 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.56 |  |  | 1.00 | 0.56 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.64 |  |  | 1.00 | 0.64 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.70 |  | 0.02 | 1.02 | 0.67 |
| delete 4 KiB, start of 32 MiB | 64 | 1.62 |  |  | 1.00 | 0.62 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.69 |  |  | 1.00 | 0.69 |
| insert 4 KiB, start of 32 MiB | 64 | 1.70 |  |  | 1.00 | 0.70 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.62 |  | 5.53 | 15.31 | 0.78 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.39 |  | 0.11 | 11.50 | 0.78 |
| append 4 KiB to 1 MiB | 128 | 1.46 |  |  | 1.00 | 0.46 |
| put 64 MiB | 64 | 29.31 |  |  | 28.56 | 0.75 |
| put 32 MiB | 64 | 15.14 |  |  | 14.42 | 0.72 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.50 |  |  | 1.05 | 0.45 |
| delete 4 KiB, start of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| write at 4 KiB in 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| insert 4 KiB, start of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| put 1 MiB | 400 | 1.64 |  |  | 1.16 | 0.48 |
| multipart put 256 MiB × 16 MiB | 32 | 172.47 | 0.94 | 33.00 | 137.56 | 0.97 |
| overwrite 1 MiB | 400 | 1.69 |  |  | 1.21 | 0.48 |
| put 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.44 |  |  | 1.00 | 0.44 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.05 |  |  |  | 0.05 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 60.38 | 0.94 | 17.00 | 41.69 | 0.75 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 0.13 |  |  |  | 0.13 |
