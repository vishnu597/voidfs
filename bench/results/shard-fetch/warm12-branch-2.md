# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T135822Z` |
| When | 2026-10-01T13:58:22Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.4 | 0.28 | 48× faster | 13× faster |
| list 200 keys | 42.8 | 1.0 | 41× faster | 9.1× faster |
| range 64 KiB of 64 MiB | 12.0 | 0.31 | 39× faster | 34× faster |
| move dir 200 × 64 KiB | 447 | 13.7 | 33× faster | 18× faster |
| get 4 KiB | 12.6 | 0.44 | 29× faster | 23× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.2 | 0.55 | 22× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 1.0 | 12× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.5 | 1.1 | 12× faster | 6.2× faster |
| get 1 MiB | 13.0 | 1.3 | 10× faster | 4.5× faster |
| rename 64 MiB | 127 | 13.8 | 9.2× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 300 | 38.1 | 7.9× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 270 | 34.9 | 7.7× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 270 | 36.3 | 7.4× faster | 13× faster |
| delete 4 KiB, start of 64 MiB | 300 | 40.7 | 7.4× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 292 | 40.0 | 7.3× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 291 | 41.7 | 7.0× faster | 11× faster |
| write at 4 KiB in 64 MiB | 253 | 39.2 | 6.5× faster | 6.2× faster |
| write at 4 KiB in 32 MiB | 154 | 35.6 | 4.3× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 150 | 35.0 | 4.3× faster | 4.6× faster |
| truncate 4 KiB, end of 32 MiB | 151 | 36.7 | 4.1× faster | 5.9× faster |
| delete 4 KiB, middle of 32 MiB | 163 | 42.8 | 3.8× faster | 4.8× faster |
| multipart put 256 MiB × 16 MiB | 3,408 | 923 | 3.7× faster | 1.8× slower |
| append 4 KiB to 32 MiB | 147 | 40.0 | 3.7× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 141 | 40.4 | 3.5× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 146 | 43.3 | 3.4× faster | 3.8× faster |
| stream get 256 MiB | 584 | 186 | 3.1× faster | 16× faster |
| get 32 MiB | 68.7 | 25.6 | 2.7× faster | 12× faster |
| stream get 64 MiB | 135 | 50.3 | 2.7× faster | 15× faster |
| get 64 MiB | 121 | 50.6 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 293 | 125 | 2.3× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 154 | 87.0 | 1.8× faster | 1.4× faster |
| put 64 MiB | 197 | 214 | 1.1× slower | 1.1× slower |
| put 4 KiB | 12.1 | 13.9 | 1.2× slower | 2.0× slower |
| put 32 MiB | 94.1 | 110 | 1.2× slower | 1.2× slower |
| overwrite 4 KiB | 12.0 | 14.3 | 1.2× slower | 3.1× slower |
| multipart put 64 MiB × 8 MiB | 283 | 339 | 1.2× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 15.1 | 1.3× slower | 2.2× slower |
| insert 4 KiB, start of 1 MiB | 28.0 | 36.0 | 1.3× slower | 1.7× slower |
| write at 4 KiB in 1 MiB | 27.6 | 35.6 | 1.3× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 28.7 | 37.2 | 1.3× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 27.0 | 35.2 | 1.3× slower | 1.0× faster |
| truncate 4 KiB, end of 1 MiB | 27.4 | 35.8 | 1.3× slower | 2.1× slower |
| delete 4 KiB, middle of 1 MiB | 26.8 | 35.1 | 1.3× slower | 1.4× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.3 | 16.3 | 1.3× slower | 2.4× slower |
| patch 16 × 4 KiB in 1 MiB | 26.8 | 35.9 | 1.3× slower | 1.5× slower |
| delete 4 KiB, start of 1 MiB | 25.7 | 37.0 | 1.4× slower | 1.5× slower |
| overwrite 1 MiB | 13.9 | 33.0 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.4 | 33.6 | 2.5× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 14.1 | 37.4 | 2.6× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| insert 4 KiB, start of 64 MiB | 64 | 1.44 |  | 0.03 | 1.00 | 0.41 |
| write at 4 KiB in 64 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.44 |  |  | 1.00 | 0.44 |
| delete 4 KiB, start of 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| insert 4 KiB, start of 32 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 22.11 |  | 5.80 | 15.69 | 0.62 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.69 |  | 0.16 | 11.86 | 0.67 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.92 |  |  | 28.28 | 0.64 |
| put 32 MiB | 64 | 15.31 |  |  | 14.70 | 0.61 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.50 |  |  | 1.14 | 0.36 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.37 |  |  | 1.00 | 0.37 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.53 |  |  | 1.16 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 172.03 | 0.94 | 33.03 | 137.28 | 0.78 |
| overwrite 1 MiB | 400 | 1.56 |  |  | 1.19 | 0.37 |
| put 4 KiB | 400 | 0.13 |  |  |  | 0.13 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 60.19 | 0.84 | 17.00 | 41.78 | 0.56 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 0.13 |  |  |  | 0.13 |
