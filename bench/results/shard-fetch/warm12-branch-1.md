# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T135542Z` |
| When | 2026-10-01T13:55:42Z |
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

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **3.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.4 | 0.24 | 53× faster | 13× faster |
| range 64 KiB of 64 MiB | 11.9 | 0.31 | 39× faster | 34× faster |
| list 200 keys | 40.0 | 1.1 | 38× faster | 9.1× faster |
| get 4 KiB | 12.9 | 0.37 | 35× faster | 23× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.1 | 0.50 | 24× faster | 5.3× faster |
| move dir 200 × 64 KiB | 458 | 19.4 | 24× faster | 18× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.2 | 1.1 | 12× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.1 | 1.1 | 11× faster | 2.6× faster |
| get 1 MiB | 13.1 | 1.2 | 11× faster | 4.5× faster |
| rename 64 MiB | 123 | 12.5 | 9.8× faster | 7.9× faster |
| delete 4 KiB, start of 64 MiB | 322 | 38.1 | 8.5× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 296 | 35.0 | 8.5× faster | 13× faster |
| delete 4 KiB, middle of 64 MiB | 300 | 37.1 | 8.1× faster | 11× faster |
| append 4 KiB to 64 MiB | 292 | 36.2 | 8.1× faster | 15× faster |
| insert 4 KiB, middle of 64 MiB | 302 | 40.5 | 7.5× faster | 11× faster |
| write at 4 KiB in 64 MiB | 296 | 40.9 | 7.2× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 289 | 40.9 | 7.1× faster | 6.8× faster |
| insert 4 KiB, start of 32 MiB | 156 | 34.9 | 4.5× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 148 | 35.2 | 4.2× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 150 | 35.6 | 4.2× faster | 8.0× faster |
| insert 4 KiB, middle of 32 MiB | 146 | 35.3 | 4.1× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 145 | 37.1 | 3.9× faster | 4.6× faster |
| truncate 4 KiB, end of 32 MiB | 143 | 37.0 | 3.9× faster | 5.9× faster |
| delete 4 KiB, middle of 32 MiB | 146 | 41.7 | 3.5× faster | 4.8× faster |
| stream get 256 MiB | 586 | 197 | 3.0× faster | 16× faster |
| stream get 64 MiB | 133 | 49.0 | 2.7× faster | 15× faster |
| get 32 MiB | 68.1 | 28.0 | 2.4× faster | 12× faster |
| get 64 MiB | 121 | 52.6 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 273 | 130 | 2.1× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 135 | 93.5 | 1.4× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,101 | 1,193 | 1.1× slower | 1.8× slower |
| put 4 KiB | 12.5 | 14.9 | 1.2× slower | 2.0× slower |
| overwrite 4 KiB | 12.1 | 14.6 | 1.2× slower | 3.1× slower |
| delete 4 KiB, middle of 1 MiB | 28.2 | 34.1 | 1.2× slower | 1.4× slower |
| put 64 MiB | 171 | 207 | 1.2× slower | 1.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.2 | 15.9 | 1.3× slower | 2.2× slower |
| delete 4 KiB, start of 1 MiB | 28.7 | 37.5 | 1.3× slower | 1.5× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.4 | 16.3 | 1.3× slower | 2.4× slower |
| truncate 4 KiB, end of 1 MiB | 27.2 | 36.1 | 1.3× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 27.2 | 36.7 | 1.3× slower | 1.6× slower |
| put 32 MiB | 89.3 | 120 | 1.3× slower | 1.2× slower |
| patch 16 × 4 KiB in 1 MiB | 26.0 | 35.3 | 1.4× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 27.1 | 37.0 | 1.4× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 26.5 | 37.2 | 1.4× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 27.1 | 38.3 | 1.4× slower | 1.4× slower |
| multipart put 64 MiB × 8 MiB | 233 | 349 | 1.5× slower | 2.4× slower |
| overwrite 1 MiB | 13.8 | 32.4 | 2.3× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 15.4 | 36.3 | 2.4× slower | 2.8× slower |
| put 1 MiB | 13.5 | 33.3 | 2.5× slower | 1.7× slower |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | delete_prefix | get | put | put_new |
|---|--:|--:|--:|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.00 |  |  |  |  |
| get 4 KiB | 400 | 0.00 |  |  |  |  |
| move dir 200 × 64 KiB | 32 | 0.22 |  |  |  | 0.22 |
| get 64 MiB | 64 | 0.00 |  |  |  |  |
| stream get 256 MiB | 32 | 0.00 |  |  |  |  |
| stream get 64 MiB | 64 | 0.00 |  |  |  |  |
| append 4 KiB to 64 MiB | 64 | 1.38 |  |  | 1.02 | 0.36 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.38 |  | 0.03 | 1.00 | 0.34 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 64 MiB | 64 | 1.45 |  |  | 1.00 | 0.45 |
| write at 4 KiB in 64 MiB | 64 | 1.44 |  | 0.02 | 1.00 | 0.42 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.44 |  |  | 1.00 | 0.44 |
| delete 4 KiB, start of 32 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.45 |  |  | 1.02 | 0.44 |
| insert 4 KiB, start of 32 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.55 |  | 4.25 | 15.59 | 0.70 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.34 |  | 0.03 | 11.77 | 0.55 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 29.31 |  |  | 28.59 | 0.72 |
| put 32 MiB | 64 | 15.12 |  |  | 14.66 | 0.47 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.34 |  |  | 1.00 | 0.34 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.50 |  |  | 1.13 | 0.37 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.55 |  |  | 1.18 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 171.59 | 0.97 | 33.03 | 136.75 | 0.84 |
| overwrite 1 MiB | 400 | 1.53 |  |  | 1.17 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.34 |  |  | 1.00 | 0.34 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 60.47 | 0.97 | 17.00 | 41.72 | 0.78 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
