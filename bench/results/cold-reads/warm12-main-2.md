# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T021526Z` |
| When | 2026-10-01T02:15:26Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 202c38a |
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
| list 200 keys | 40.8 | 1.0 | 40× faster | 9.1× faster |
| move dir 200 × 64 KiB | 464 | 12.5 | 37× faster | 18× faster |
| range 64 KiB of 64 MiB | 11.7 | 0.32 | 37× faster | 34× faster |
| get 4 KiB | 12.6 | 0.36 | 35× faster | 23× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.1 | 0.52 | 23× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.1 | 0.89 | 14× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.4 | 1.1 | 12× faster | 6.2× faster |
| get 1 MiB | 12.7 | 1.2 | 11× faster | 4.5× faster |
| rename 64 MiB | 117 | 12.4 | 9.4× faster | 7.9× faster |
| write at 4 KiB in 64 MiB | 298 | 33.7 | 8.8× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 301 | 35.1 | 8.6× faster | 15× faster |
| delete 4 KiB, start of 64 MiB | 284 | 35.7 | 8.0× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 284 | 36.6 | 7.8× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 272 | 38.7 | 7.0× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 288 | 41.3 | 7.0× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 284 | 43.4 | 6.5× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 150 | 35.0 | 4.3× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 153 | 36.0 | 4.2× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 156 | 37.4 | 4.2× faster | 4.8× faster |
| append 4 KiB to 32 MiB | 139 | 35.0 | 4.0× faster | 8.0× faster |
| insert 4 KiB, middle of 32 MiB | 145 | 39.5 | 3.7× faster | 3.8× faster |
| write at 4 KiB in 32 MiB | 134 | 36.9 | 3.6× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 137 | 39.6 | 3.5× faster | 3.5× faster |
| stream get 256 MiB | 595 | 198 | 3.0× faster | 16× faster |
| get 32 MiB | 66.6 | 25.8 | 2.6× faster | 12× faster |
| stream get 64 MiB | 136 | 54.0 | 2.5× faster | 15× faster |
| get 64 MiB | 123 | 50.5 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 271 | 127 | 2.1× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 132 | 96.4 | 1.4× faster | 1.4× faster |
| overwrite 4 KiB | 14.1 | 15.3 | 1.1× slower | 3.1× slower |
| multipart put 256 MiB × 16 MiB | 799 | 877 | 1.1× slower | 1.8× slower |
| put 4 KiB | 13.8 | 15.7 | 1.1× slower | 2.0× slower |
| put 32 MiB | 89.1 | 114 | 1.3× slower | 1.2× slower |
| write at 4 KiB in 1 MiB | 26.7 | 34.6 | 1.3× slower | 1.6× slower |
| put 64 MiB | 162 | 211 | 1.3× slower | 1.1× slower |
| delete 4 KiB, start of 1 MiB | 27.5 | 35.8 | 1.3× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 27.0 | 35.5 | 1.3× slower | 1.7× slower |
| truncate 4 KiB, end of 1 MiB | 28.1 | 37.1 | 1.3× slower | 2.1× slower |
| multipart put 64 MiB × 8 MiB | 253 | 339 | 1.3× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 26.7 | 36.2 | 1.4× slower | 1.5× slower |
| append 4 KiB to 1 MiB | 26.0 | 35.3 | 1.4× slower | 1.0× faster |
| fanout put 1000 × 4 KiB, 32 at once | 12.2 | 16.8 | 1.4× slower | 2.2× slower |
| delete 4 KiB, middle of 1 MiB | 27.3 | 37.9 | 1.4× slower | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 26.9 | 38.0 | 1.4× slower | 1.4× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.2 | 17.9 | 1.5× slower | 2.4× slower |
| overwrite 1 MiB | 13.9 | 33.6 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.4 | 33.1 | 2.5× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.3 | 40.3 | 3.0× slower | 2.8× slower |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | delete_prefix | get | put | put_new |
|---|--:|--:|--:|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.00 |  |  |  |  |
| get 4 KiB | 400 | 0.00 |  |  |  |  |
| move dir 200 × 64 KiB | 32 | 0.16 |  |  |  | 0.16 |
| get 64 MiB | 64 | 0.00 |  |  |  |  |
| stream get 256 MiB | 32 | 0.00 |  |  |  |  |
| stream get 64 MiB | 64 | 0.00 |  |  |  |  |
| append 4 KiB to 64 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.45 |  | 0.03 | 1.00 | 0.42 |
| insert 4 KiB, start of 64 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| write at 4 KiB in 64 MiB | 64 | 1.33 |  |  | 1.00 | 0.33 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  | 0.00 |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.44 |  |  | 1.00 | 0.44 |
| delete 4 KiB, start of 32 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| insert 4 KiB, start of 32 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.34 |  | 4.95 | 14.66 | 0.73 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.06 |  | 0.08 | 11.42 | 0.56 |
| append 4 KiB to 1 MiB | 128 | 1.35 |  |  | 1.00 | 0.35 |
| put 64 MiB | 64 | 29.25 |  |  | 28.58 | 0.67 |
| put 32 MiB | 64 | 15.09 |  |  | 14.47 | 0.62 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.34 |  |  | 1.00 | 0.34 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.38 |  |  | 1.02 | 0.36 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.55 |  |  | 1.18 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 171.94 | 0.97 | 33.03 | 137.06 | 0.88 |
| overwrite 1 MiB | 400 | 1.56 |  |  | 1.19 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.35 |  |  | 1.00 | 0.35 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 60.16 | 0.97 | 17.00 | 41.50 | 0.69 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.08 |  |  | 1.00 | 0.08 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
