# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T141020Z` |
| When | 2026-10-01T14:10:20Z |
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

voidfs is faster in **41 of 49** scenarios and slower in the other **8**. Geometric mean speed-up over the bare bucket: **5.8×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.5 | 0.26 | 52× faster | 13× faster |
| list 200 keys | 45.5 | 1.2 | 40× faster | 9.1× faster |
| insert 4 KiB, start of 64 MiB | 1,757 | 44.6 | 39× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 1,752 | 49.6 | 35× faster | 13× faster |
| write at 4 KiB in 64 MiB | 1,753 | 49.7 | 35× faster | 6.2× faster |
| get 4 KiB | 13.8 | 0.41 | 34× faster | 23× faster |
| range 64 KiB of 64 MiB | 13.2 | 0.41 | 33× faster | 34× faster |
| move dir 200 × 64 KiB | 463 | 16.1 | 29× faster | 18× faster |
| delete 4 KiB, middle of 64 MiB | 1,757 | 61.4 | 29× faster | 11× faster |
| append 4 KiB to 64 MiB | 1,752 | 62.8 | 28× faster | 15× faster |
| delete 4 KiB, start of 64 MiB | 1,756 | 63.0 | 28× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 1,755 | 65.8 | 27× faster | 11× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.5 | 0.49 | 26× faster | 5.3× faster |
| delete 4 KiB, start of 32 MiB | 894 | 40.6 | 22× faster | 4.6× faster |
| truncate 4 KiB, end of 32 MiB | 892 | 42.0 | 21× faster | 5.9× faster |
| get 1 MiB | 22.8 | 1.4 | 16× faster | 4.5× faster |
| append 4 KiB to 32 MiB | 895 | 56.2 | 16× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 893 | 61.3 | 15× faster | 5.1× faster |
| stream get 64 MiB | 722 | 49.8 | 14× faster | 15× faster |
| stream get 256 MiB | 2,841 | 219 | 13× faster | 16× faster |
| get 64 MiB | 733 | 57.0 | 13× faster | 17× faster |
| insert 4 KiB, middle of 32 MiB | 895 | 70.3 | 13× faster | 3.8× faster |
| insert 4 KiB, start of 32 MiB | 895 | 71.0 | 13× faster | 3.5× faster |
| delete 4 KiB, middle of 32 MiB | 894 | 72.2 | 12× faster | 4.8× faster |
| fanout get 200 × 256 KiB, 32 at once | 16.0 | 1.3 | 12× faster | 6.2× faster |
| get 32 MiB | 373 | 31.4 | 12× faster | 12× faster |
| fanout get 1000 × 4 KiB, 64 at once | 11.9 | 1.0 | 12× faster | 2.6× faster |
| rename 64 MiB | 124 | 13.9 | 8.9× faster | 7.9× faster |
| patch 16 × 4 KiB in 64 MiB | 1,754 | 416 | 4.2× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 894 | 263 | 3.4× faster | 1.4× faster |
| put 64 MiB | 1,024 | 582 | 1.8× faster | 1.1× slower |
| put 32 MiB | 519 | 298 | 1.7× faster | 1.2× slower |
| truncate 4 KiB, end of 1 MiB | 50.1 | 39.3 | 1.3× faster | 2.1× slower |
| write at 4 KiB in 1 MiB | 50.7 | 44.0 | 1.2× faster | 1.6× slower |
| delete 4 KiB, start of 1 MiB | 50.8 | 44.4 | 1.1× faster | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 51.4 | 45.4 | 1.1× faster | 1.4× slower |
| append 4 KiB to 1 MiB | 52.2 | 46.4 | 1.1× faster | 1.0× faster |
| patch 16 × 4 KiB in 1 MiB | 51.3 | 45.7 | 1.1× faster | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 50.5 | 45.7 | 1.1× faster | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 52.4 | 47.6 | 1.1× faster | 1.7× slower |
| multipart put 64 MiB × 8 MiB | 641 | 601 | 1.1× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 2,080 | 2,229 | 1.1× slower | 1.8× slower |
| put 4 KiB | 13.2 | 15.6 | 1.2× slower | 2.0× slower |
| overwrite 4 KiB | 12.7 | 15.3 | 1.2× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.2 | 18.5 | 1.5× slower | 2.2× slower |
| overwrite 1 MiB | 27.7 | 44.6 | 1.6× slower | 1.9× slower |
| put 1 MiB | 27.5 | 44.3 | 1.6× slower | 1.7× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.6 | 21.1 | 1.7× slower | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 16.5 | 36.9 | 2.2× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.67 |  | 0.02 | 1.00 | 0.66 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.58 |  | 0.03 | 1.00 | 0.55 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.59 |  |  | 1.00 | 0.59 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.66 |  | 0.02 | 1.00 | 0.64 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.61 |  |  | 1.00 | 0.61 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.64 |  | 0.02 | 1.00 | 0.62 |
| insert 4 KiB, start of 64 MiB | 64 | 1.56 |  | 0.03 | 1.00 | 0.53 |
| write at 4 KiB in 64 MiB | 64 | 1.56 |  |  | 1.00 | 0.56 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.56 |  |  | 1.00 | 0.56 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.55 |  |  | 1.00 | 0.55 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.66 |  |  | 1.00 | 0.66 |
| delete 4 KiB, start of 32 MiB | 64 | 1.52 |  |  | 1.00 | 0.52 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.69 |  |  | 1.02 | 0.67 |
| insert 4 KiB, start of 32 MiB | 64 | 1.73 |  |  | 1.02 | 0.72 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.44 |  | 5.20 | 15.44 | 0.80 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.06 |  | 0.02 | 11.20 | 0.84 |
| append 4 KiB to 1 MiB | 128 | 1.44 |  |  | 1.00 | 0.44 |
| put 64 MiB | 64 | 29.36 |  |  | 28.58 | 0.78 |
| put 32 MiB | 64 | 15.19 |  |  | 14.42 | 0.77 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.43 |  |  | 1.00 | 0.43 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.46 |  |  | 1.00 | 0.46 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.59 |  |  | 1.15 | 0.44 |
| delete 4 KiB, start of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| write at 4 KiB in 1 MiB | 128 | 1.44 |  |  | 1.00 | 0.44 |
| insert 4 KiB, start of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| put 1 MiB | 400 | 1.63 |  |  | 1.14 | 0.48 |
| multipart put 256 MiB × 16 MiB | 32 | 173.16 | 0.94 | 33.00 | 138.31 | 0.91 |
| overwrite 1 MiB | 400 | 1.65 |  |  | 1.17 | 0.48 |
| put 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.43 |  |  | 1.00 | 0.43 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 60.62 | 0.91 | 17.00 | 41.81 | 0.91 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
