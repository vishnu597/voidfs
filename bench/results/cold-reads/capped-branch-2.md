# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T023736Z` |
| When | 2026-10-01T02:37:36Z |
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
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **42 of 49** scenarios and slower in the other **7**. Geometric mean speed-up over the bare bucket: **5.9×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.7 | 0.27 | 50× faster | 13× faster |
| list 200 keys | 43.7 | 0.95 | 46× faster | 9.1× faster |
| range 64 KiB of 64 MiB | 13.0 | 0.32 | 41× faster | 34× faster |
| append 4 KiB to 64 MiB | 1,751 | 47.5 | 37× faster | 15× faster |
| get 4 KiB | 13.9 | 0.40 | 35× faster | 23× faster |
| write at 4 KiB in 64 MiB | 1,751 | 55.6 | 32× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 1,752 | 55.6 | 31× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 1,751 | 56.5 | 31× faster | 13× faster |
| move dir 200 × 64 KiB | 462 | 15.2 | 30× faster | 18× faster |
| delete 4 KiB, middle of 64 MiB | 1,751 | 61.8 | 28× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 1,755 | 66.8 | 26× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 1,754 | 71.2 | 25× faster | 6.9× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.6 | 0.54 | 23× faster | 5.3× faster |
| append 4 KiB to 32 MiB | 891 | 39.6 | 23× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 891 | 47.8 | 19× faster | 4.6× faster |
| get 1 MiB | 23.3 | 1.4 | 17× faster | 4.5× faster |
| truncate 4 KiB, end of 32 MiB | 890 | 52.4 | 17× faster | 5.9× faster |
| insert 4 KiB, middle of 32 MiB | 891 | 61.4 | 15× faster | 3.8× faster |
| insert 4 KiB, start of 32 MiB | 892 | 62.7 | 14× faster | 3.5× faster |
| stream get 64 MiB | 720 | 52.9 | 14× faster | 15× faster |
| fanout get 200 × 256 KiB, 32 at once | 15.5 | 1.1 | 14× faster | 6.2× faster |
| stream get 256 MiB | 2,840 | 212 | 13× faster | 16× faster |
| get 64 MiB | 727 | 55.5 | 13× faster | 17× faster |
| write at 4 KiB in 32 MiB | 890 | 68.5 | 13× faster | 5.1× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 0.99 | 12× faster | 2.6× faster |
| get 32 MiB | 370 | 30.1 | 12× faster | 12× faster |
| delete 4 KiB, middle of 32 MiB | 892 | 72.8 | 12× faster | 4.8× faster |
| rename 64 MiB | 126 | 14.0 | 9.0× faster | 7.9× faster |
| patch 16 × 4 KiB in 64 MiB | 1,752 | 392 | 4.5× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 892 | 271 | 3.3× faster | 1.4× faster |
| put 64 MiB | 1,026 | 574 | 1.8× faster | 1.1× slower |
| put 32 MiB | 520 | 304 | 1.7× faster | 1.2× slower |
| write at 4 KiB in 1 MiB | 53.3 | 44.6 | 1.2× faster | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 51.3 | 43.9 | 1.2× faster | 2.1× slower |
| insert 4 KiB, middle of 1 MiB | 53.8 | 46.9 | 1.1× faster | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 51.6 | 45.3 | 1.1× faster | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 53.1 | 47.7 | 1.1× faster | 1.7× slower |
| delete 4 KiB, start of 1 MiB | 51.7 | 46.7 | 1.1× faster | 1.5× slower |
| multipart put 256 MiB × 16 MiB | 2,260 | 2,051 | 1.1× faster | 1.8× slower |
| patch 16 × 4 KiB in 1 MiB | 51.8 | 47.4 | 1.1× faster | 1.5× slower |
| append 4 KiB to 1 MiB | 52.6 | 48.2 | 1.1× faster | 1.0× faster |
| multipart put 64 MiB × 8 MiB | 634 | 594 | 1.1× faster | 2.4× slower |
| put 4 KiB | 14.1 | 16.3 | 1.2× slower | 2.0× slower |
| overwrite 4 KiB | 13.4 | 15.7 | 1.2× slower | 3.1× slower |
| overwrite 1 MiB | 28.0 | 43.4 | 1.6× slower | 1.9× slower |
| put 1 MiB | 28.0 | 44.0 | 1.6× slower | 1.7× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.0 | 19.8 | 1.6× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.5 | 21.2 | 1.7× slower | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 16.2 | 35.8 | 2.2× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.55 |  | 0.02 | 1.00 | 0.53 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.59 |  | 0.02 | 1.00 | 0.58 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.73 |  |  | 1.00 | 0.73 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.66 |  | 0.02 | 1.00 | 0.64 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.53 |  |  | 1.02 | 0.52 |
| rename 64 MiB | 64 | 0.17 |  |  |  | 0.17 |
| delete 4 KiB, start of 64 MiB | 64 | 1.64 |  |  | 1.00 | 0.64 |
| insert 4 KiB, start of 64 MiB | 64 | 1.61 |  | 0.02 | 1.00 | 0.59 |
| write at 4 KiB in 64 MiB | 64 | 1.56 |  |  | 1.00 | 0.56 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.62 |  |  | 1.00 | 0.62 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.72 |  |  | 1.00 | 0.72 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.75 |  |  | 1.02 | 0.73 |
| delete 4 KiB, start of 32 MiB | 64 | 1.53 |  |  | 1.00 | 0.53 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.70 |  |  | 1.00 | 0.70 |
| insert 4 KiB, start of 32 MiB | 64 | 1.66 |  |  | 1.00 | 0.66 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.92 |  | 4.72 | 15.33 | 0.88 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.30 |  | 0.03 | 11.41 | 0.86 |
| append 4 KiB to 1 MiB | 128 | 1.43 |  |  | 1.00 | 0.43 |
| put 64 MiB | 64 | 29.53 |  |  | 28.70 | 0.83 |
| put 32 MiB | 64 | 15.16 |  |  | 14.45 | 0.70 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.61 |  |  | 1.16 | 0.45 |
| delete 4 KiB, start of 1 MiB | 128 | 1.43 |  |  | 1.00 | 0.43 |
| write at 4 KiB in 1 MiB | 128 | 1.46 |  |  | 1.00 | 0.46 |
| insert 4 KiB, start of 1 MiB | 128 | 1.45 |  |  | 1.01 | 0.44 |
| put 1 MiB | 400 | 1.67 |  | 0.00 | 1.19 | 0.48 |
| multipart put 256 MiB × 16 MiB | 32 | 173.94 | 0.94 | 33.00 | 139.06 | 0.94 |
| overwrite 1 MiB | 400 | 1.64 |  |  | 1.15 | 0.48 |
| put 4 KiB | 400 | 0.13 |  |  |  | 0.13 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 59.88 | 0.84 | 17.00 | 41.34 | 0.69 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 0.13 |  |  |  | 0.13 |
