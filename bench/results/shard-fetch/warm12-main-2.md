# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T140101Z` |
| When | 2026-10-01T14:01:01Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 12b68e9 |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.9×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.1 | 0.25 | 49× faster | 13× faster |
| list 200 keys | 45.5 | 1.1 | 41× faster | 9.1× faster |
| range 64 KiB of 64 MiB | 11.6 | 0.30 | 38× faster | 34× faster |
| move dir 200 × 64 KiB | 458 | 14.6 | 31× faster | 18× faster |
| get 4 KiB | 12.8 | 0.43 | 29× faster | 23× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.2 | 0.61 | 20× faster | 5.3× faster |
| rename 64 MiB | 161 | 12.4 | 13× faster | 7.9× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.0 | 0.98 | 12× faster | 2.6× faster |
| get 1 MiB | 12.9 | 1.2 | 11× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.1 | 1.4 | 9.4× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 288 | 34.6 | 8.3× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 279 | 33.8 | 8.3× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 291 | 35.6 | 8.2× faster | 13× faster |
| append 4 KiB to 64 MiB | 285 | 36.5 | 7.8× faster | 15× faster |
| delete 4 KiB, middle of 64 MiB | 301 | 39.0 | 7.7× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 304 | 40.3 | 7.5× faster | 11× faster |
| write at 4 KiB in 64 MiB | 286 | 41.8 | 6.8× faster | 6.2× faster |
| insert 4 KiB, middle of 32 MiB | 154 | 36.6 | 4.2× faster | 3.8× faster |
| write at 4 KiB in 32 MiB | 150 | 35.7 | 4.2× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 153 | 37.0 | 4.1× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 145 | 36.1 | 4.0× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 138 | 34.8 | 4.0× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 131 | 35.2 | 3.7× faster | 4.6× faster |
| stream get 256 MiB | 574 | 199 | 2.9× faster | 16× faster |
| delete 4 KiB, middle of 32 MiB | 131 | 45.9 | 2.9× faster | 4.8× faster |
| stream get 64 MiB | 131 | 49.7 | 2.6× faster | 15× faster |
| get 32 MiB | 66.2 | 26.7 | 2.5× faster | 12× faster |
| get 64 MiB | 122 | 54.0 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 256 | 126 | 2.0× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 136 | 103 | 1.3× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 282 | 299 | 1.1× slower | 2.4× slower |
| put 4 KiB | 12.9 | 14.8 | 1.1× slower | 2.0× slower |
| overwrite 4 KiB | 12.3 | 14.3 | 1.2× slower | 3.1× slower |
| insert 4 KiB, middle of 1 MiB | 27.1 | 34.4 | 1.3× slower | 1.4× slower |
| put 64 MiB | 183 | 233 | 1.3× slower | 1.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.8 | 16.7 | 1.3× slower | 2.4× slower |
| append 4 KiB to 1 MiB | 26.3 | 34.7 | 1.3× slower | 1.0× faster |
| patch 16 × 4 KiB in 1 MiB | 28.2 | 37.2 | 1.3× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 26.6 | 35.5 | 1.3× slower | 1.6× slower |
| put 32 MiB | 89.6 | 120 | 1.3× slower | 1.2× slower |
| delete 4 KiB, start of 1 MiB | 27.5 | 36.8 | 1.3× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 26.3 | 35.2 | 1.3× slower | 2.1× slower |
| delete 4 KiB, middle of 1 MiB | 27.5 | 37.1 | 1.3× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 27.8 | 37.7 | 1.4× slower | 1.7× slower |
| multipart put 256 MiB × 16 MiB | 673 | 1,047 | 1.6× slower | 1.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 21.9 | 1.8× slower | 2.2× slower |
| overwrite 1 MiB | 14.2 | 32.7 | 2.3× slower | 1.9× slower |
| put 1 MiB | 13.5 | 32.8 | 2.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.2 | 35.9 | 2.7× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.38 |  | 0.03 | 1.00 | 0.34 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.31 |  |  | 1.00 | 0.31 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.42 |  | 0.05 | 1.00 | 0.38 |
| insert 4 KiB, start of 64 MiB | 64 | 1.39 |  |  | 1.02 | 0.38 |
| write at 4 KiB in 64 MiB | 64 | 1.44 |  |  | 1.00 | 0.44 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.47 |  |  | 1.00 | 0.47 |
| delete 4 KiB, start of 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| insert 4 KiB, start of 32 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 19.84 |  | 4.11 | 15.03 | 0.70 |
| patch 16 × 4 KiB in 32 MiB | 64 | 13.27 |  | 0.02 | 12.62 | 0.62 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 29.53 |  |  | 29.00 | 0.53 |
| put 32 MiB | 64 | 14.69 |  |  | 14.14 | 0.55 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.34 |  |  | 1.00 | 0.34 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.37 |  |  | 1.00 | 0.37 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.62 |  |  | 1.26 | 0.36 |
| delete 4 KiB, start of 1 MiB | 128 | 1.35 |  |  | 1.00 | 0.35 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.54 |  |  | 1.17 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 173.78 | 0.97 | 33.03 | 139.09 | 0.69 |
| overwrite 1 MiB | 400 | 1.53 |  |  | 1.16 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 61.12 | 0.97 | 17.00 | 42.25 | 0.91 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
