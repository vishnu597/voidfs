# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T023051Z` |
| When | 2026-10-01T02:30:51Z |
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

voidfs is faster in **41 of 49** scenarios and slower in the other **8**. Geometric mean speed-up over the bare bucket: **5.9×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.5 | 0.24 | 57× faster | 13× faster |
| get 4 KiB | 14.4 | 0.37 | 39× faster | 23× faster |
| truncate 4 KiB, end of 64 MiB | 1,752 | 46.1 | 38× faster | 13× faster |
| list 200 keys | 37.4 | 1.0 | 37× faster | 9.1× faster |
| append 4 KiB to 64 MiB | 1,750 | 47.1 | 37× faster | 15× faster |
| range 64 KiB of 64 MiB | 14.7 | 0.40 | 36× faster | 34× faster |
| write at 4 KiB in 64 MiB | 1,749 | 62.3 | 28× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 1,751 | 63.3 | 28× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 1,752 | 64.6 | 27× faster | 6.9× faster |
| move dir 200 × 64 KiB | 446 | 16.9 | 26× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.1 | 0.46 | 26× faster | 5.3× faster |
| insert 4 KiB, middle of 64 MiB | 1,748 | 77.0 | 23× faster | 11× faster |
| append 4 KiB to 32 MiB | 889 | 40.9 | 22× faster | 8.0× faster |
| delete 4 KiB, middle of 64 MiB | 1,746 | 81.3 | 21× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 890 | 42.7 | 21× faster | 5.9× faster |
| get 1 MiB | 23.2 | 1.3 | 18× faster | 4.5× faster |
| insert 4 KiB, start of 32 MiB | 889 | 50.1 | 18× faster | 3.5× faster |
| delete 4 KiB, middle of 32 MiB | 888 | 57.8 | 15× faster | 4.8× faster |
| stream get 64 MiB | 721 | 49.5 | 15× faster | 15× faster |
| stream get 256 MiB | 2,840 | 203 | 14× faster | 16× faster |
| write at 4 KiB in 32 MiB | 889 | 64.4 | 14× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 890 | 65.6 | 14× faster | 3.8× faster |
| fanout get 200 × 256 KiB, 32 at once | 15.0 | 1.1 | 14× faster | 6.2× faster |
| delete 4 KiB, start of 32 MiB | 890 | 66.6 | 13× faster | 4.6× faster |
| get 64 MiB | 728 | 56.5 | 13× faster | 17× faster |
| fanout get 1000 × 4 KiB, 64 at once | 11.9 | 0.98 | 12× faster | 2.6× faster |
| get 32 MiB | 369 | 30.9 | 12× faster | 12× faster |
| rename 64 MiB | 123 | 12.5 | 9.9× faster | 7.9× faster |
| patch 16 × 4 KiB in 64 MiB | 1,748 | 359 | 4.9× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 889 | 259 | 3.4× faster | 1.4× faster |
| put 64 MiB | 1,023 | 577 | 1.8× faster | 1.1× slower |
| put 32 MiB | 519 | 304 | 1.7× faster | 1.2× slower |
| append 4 KiB to 1 MiB | 51.9 | 45.0 | 1.2× faster | 1.0× faster |
| patch 16 × 4 KiB in 1 MiB | 51.5 | 44.7 | 1.2× faster | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 51.4 | 44.7 | 1.1× faster | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 51.1 | 44.5 | 1.1× faster | 1.4× slower |
| write at 4 KiB in 1 MiB | 50.8 | 44.7 | 1.1× faster | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 50.2 | 44.5 | 1.1× faster | 2.1× slower |
| insert 4 KiB, middle of 1 MiB | 52.4 | 47.2 | 1.1× faster | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 49.6 | 48.2 | 1.0× faster | 1.5× slower |
| multipart put 64 MiB × 8 MiB | 616 | 606 | 1.0× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,966 | 2,075 | 1.1× slower | 1.8× slower |
| overwrite 4 KiB | 13.1 | 16.2 | 1.2× slower | 3.1× slower |
| put 4 KiB | 12.5 | 15.5 | 1.2× slower | 2.0× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 17.7 | 1.5× slower | 2.2× slower |
| overwrite 1 MiB | 27.8 | 44.1 | 1.6× slower | 1.9× slower |
| put 1 MiB | 27.6 | 43.8 | 1.6× slower | 1.7× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.4 | 20.8 | 1.7× slower | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 16.8 | 34.4 | 2.0× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.56 |  | 0.02 | 1.00 | 0.55 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.52 |  | 0.02 | 1.00 | 0.50 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.77 |  |  | 1.00 | 0.77 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.83 |  | 0.02 | 1.02 | 0.80 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.50 |  |  | 1.00 | 0.50 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.69 |  | 0.05 | 1.00 | 0.64 |
| insert 4 KiB, start of 64 MiB | 64 | 1.70 |  | 0.02 | 1.00 | 0.69 |
| write at 4 KiB in 64 MiB | 64 | 1.64 |  | 0.02 | 1.00 | 0.62 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.55 |  |  | 1.00 | 0.55 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.66 |  |  | 1.00 | 0.66 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.67 |  | 0.02 | 1.00 | 0.66 |
| delete 4 KiB, start of 32 MiB | 64 | 1.69 |  |  | 1.00 | 0.69 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.72 |  |  | 1.02 | 0.70 |
| insert 4 KiB, start of 32 MiB | 64 | 1.62 |  |  | 1.00 | 0.62 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.80 |  | 5.56 | 15.44 | 0.80 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.23 |  | 0.02 | 11.39 | 0.83 |
| append 4 KiB to 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| put 64 MiB | 64 | 29.48 |  |  | 28.72 | 0.77 |
| put 32 MiB | 64 | 14.95 |  |  | 14.22 | 0.73 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.44 |  |  | 1.00 | 0.44 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.60 |  |  | 1.15 | 0.45 |
| delete 4 KiB, start of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| write at 4 KiB in 1 MiB | 128 | 1.44 |  |  | 1.00 | 0.44 |
| insert 4 KiB, start of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| put 1 MiB | 400 | 1.67 |  |  | 1.18 | 0.48 |
| multipart put 256 MiB × 16 MiB | 32 | 172.28 | 0.97 | 33.00 | 137.41 | 0.91 |
| overwrite 1 MiB | 400 | 1.69 |  |  | 1.20 | 0.48 |
| put 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.45 |  |  | 1.00 | 0.45 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 60.25 | 0.88 | 17.00 | 41.72 | 0.66 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
