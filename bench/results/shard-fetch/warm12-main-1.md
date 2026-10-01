# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T135311Z` |
| When | 2026-10-01T13:53:11Z |
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
| head | 11.8 | 0.20 | 59× faster | 13× faster |
| list 200 keys | 46.5 | 0.99 | 47× faster | 9.1× faster |
| range 64 KiB of 64 MiB | 12.1 | 0.32 | 38× faster | 34× faster |
| get 4 KiB | 12.3 | 0.34 | 36× faster | 23× faster |
| move dir 200 × 64 KiB | 456 | 14.8 | 31× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 11.9 | 0.45 | 27× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.0 | 0.86 | 14× faster | 2.6× faster |
| get 1 MiB | 13.0 | 0.98 | 13× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 12.9 | 1.0 | 13× faster | 6.2× faster |
| rename 64 MiB | 138 | 12.6 | 11× faster | 7.9× faster |
| delete 4 KiB, start of 64 MiB | 327 | 37.3 | 8.8× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 311 | 36.8 | 8.5× faster | 11× faster |
| append 4 KiB to 64 MiB | 301 | 36.7 | 8.2× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 296 | 37.6 | 7.9× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 273 | 35.4 | 7.7× faster | 13× faster |
| write at 4 KiB in 64 MiB | 295 | 39.3 | 7.5× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 287 | 42.8 | 6.7× faster | 11× faster |
| delete 4 KiB, start of 32 MiB | 146 | 34.8 | 4.2× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 149 | 37.3 | 4.0× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 149 | 37.5 | 4.0× faster | 5.1× faster |
| truncate 4 KiB, end of 32 MiB | 149 | 37.7 | 3.9× faster | 5.9× faster |
| insert 4 KiB, middle of 32 MiB | 153 | 40.0 | 3.8× faster | 3.8× faster |
| append 4 KiB to 32 MiB | 132 | 36.0 | 3.7× faster | 8.0× faster |
| delete 4 KiB, middle of 32 MiB | 143 | 41.1 | 3.5× faster | 4.8× faster |
| stream get 256 MiB | 569 | 195 | 2.9× faster | 16× faster |
| stream get 64 MiB | 134 | 51.1 | 2.6× faster | 15× faster |
| get 32 MiB | 67.3 | 25.9 | 2.6× faster | 12× faster |
| get 64 MiB | 129 | 51.0 | 2.5× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 277 | 153 | 1.8× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 137 | 98.0 | 1.4× faster | 1.4× faster |
| put 4 KiB | 12.8 | 14.3 | 1.1× slower | 2.0× slower |
| overwrite 4 KiB | 12.8 | 14.6 | 1.1× slower | 3.1× slower |
| truncate 4 KiB, end of 1 MiB | 27.5 | 33.7 | 1.2× slower | 2.1× slower |
| put 64 MiB | 177 | 228 | 1.3× slower | 1.1× slower |
| append 4 KiB to 1 MiB | 27.6 | 35.6 | 1.3× slower | 1.0× faster |
| put 32 MiB | 86.8 | 113 | 1.3× slower | 1.2× slower |
| patch 16 × 4 KiB in 1 MiB | 27.1 | 35.2 | 1.3× slower | 1.5× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.8 | 16.7 | 1.3× slower | 2.4× slower |
| delete 4 KiB, start of 1 MiB | 27.9 | 36.8 | 1.3× slower | 1.5× slower |
| delete 4 KiB, middle of 1 MiB | 26.9 | 35.8 | 1.3× slower | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 26.3 | 35.3 | 1.3× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 26.6 | 36.4 | 1.4× slower | 1.6× slower |
| insert 4 KiB, start of 1 MiB | 27.0 | 37.1 | 1.4× slower | 1.7× slower |
| multipart put 256 MiB × 16 MiB | 649 | 953 | 1.5× slower | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 287 | 453 | 1.6× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.0 | 22.0 | 1.8× slower | 2.2× slower |
| overwrite 1 MiB | 14.3 | 32.6 | 2.3× slower | 1.9× slower |
| put 1 MiB | 13.4 | 32.8 | 2.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.1 | 35.8 | 2.7× slower | 2.8× slower |

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
| truncate 4 KiB, end of 64 MiB | 64 | 1.36 |  | 0.02 | 1.00 | 0.34 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.45 |  |  | 1.00 | 0.45 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.45 |  | 0.03 | 1.00 | 0.42 |
| insert 4 KiB, start of 64 MiB | 64 | 1.47 |  | 0.03 | 1.00 | 0.44 |
| write at 4 KiB in 64 MiB | 64 | 1.42 |  | 0.02 | 1.00 | 0.41 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| delete 4 KiB, start of 32 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.44 |  |  | 1.00 | 0.44 |
| insert 4 KiB, start of 32 MiB | 64 | 1.44 |  |  | 1.00 | 0.44 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.98 |  | 4.97 | 15.30 | 0.72 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.58 |  | 0.03 | 10.94 | 0.61 |
| append 4 KiB to 1 MiB | 128 | 1.35 |  |  | 1.00 | 0.35 |
| put 64 MiB | 64 | 28.94 |  |  | 28.33 | 0.61 |
| put 32 MiB | 64 | 15.02 |  |  | 14.38 | 0.64 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.34 |  |  | 1.00 | 0.34 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.50 |  |  | 1.13 | 0.37 |
| delete 4 KiB, start of 1 MiB | 128 | 1.37 |  |  | 1.00 | 0.37 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.52 |  |  | 1.14 | 0.38 |
| multipart put 256 MiB × 16 MiB | 32 | 173.28 | 0.97 | 33.09 | 138.38 | 0.84 |
| overwrite 1 MiB | 400 | 1.55 |  |  | 1.18 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.34 |  |  | 1.00 | 0.34 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.05 |  |  |  | 0.05 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 60.69 | 0.81 | 17.00 | 42.38 | 0.50 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
