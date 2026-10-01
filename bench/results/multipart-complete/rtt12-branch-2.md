# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T233224Z` |
| When | 2026-09-30T23:32:24Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | a03a25d |
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
| range 64 KiB of 64 MiB | 13.6 | 0.31 | 45× faster | 34× faster |
| head | 12.6 | 0.28 | 44× faster | 13× faster |
| list 200 keys | 41.8 | 0.97 | 43× faster | 9.1× faster |
| get 4 KiB | 14.0 | 0.34 | 41× faster | 23× faster |
| move dir 200 × 64 KiB | 453 | 15.5 | 29× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.8 | 0.54 | 24× faster | 5.3× faster |
| get 1 MiB | 13.7 | 1.0 | 13× faster | 4.5× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.9 | 1.1 | 12× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.2 | 1.1 | 11× faster | 2.6× faster |
| rename 64 MiB | 133 | 13.8 | 9.6× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 296 | 36.8 | 8.0× faster | 13× faster |
| append 4 KiB to 64 MiB | 278 | 36.7 | 7.6× faster | 15× faster |
| delete 4 KiB, start of 64 MiB | 326 | 43.1 | 7.6× faster | 6.9× faster |
| write at 4 KiB in 64 MiB | 283 | 38.2 | 7.4× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 293 | 40.6 | 7.2× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 277 | 39.8 | 7.0× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 275 | 40.0 | 6.9× faster | 6.8× faster |
| write at 4 KiB in 32 MiB | 162 | 36.7 | 4.4× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 157 | 36.7 | 4.3× faster | 3.5× faster |
| delete 4 KiB, middle of 32 MiB | 158 | 39.5 | 4.0× faster | 4.8× faster |
| append 4 KiB to 32 MiB | 147 | 38.4 | 3.8× faster | 8.0× faster |
| insert 4 KiB, middle of 32 MiB | 138 | 36.4 | 3.8× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 143 | 37.8 | 3.8× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 148 | 42.4 | 3.5× faster | 4.6× faster |
| stream get 256 MiB | 572 | 194 | 3.0× faster | 16× faster |
| stream get 64 MiB | 137 | 48.7 | 2.8× faster | 15× faster |
| get 32 MiB | 67.3 | 25.9 | 2.6× faster | 12× faster |
| get 64 MiB | 126 | 51.5 | 2.4× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 283 | 136 | 2.1× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 150 | 87.1 | 1.7× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,127 | 903 | 1.2× faster | 1.8× slower |
| overwrite 4 KiB | 13.4 | 15.6 | 1.2× slower | 3.1× slower |
| put 4 KiB | 13.5 | 15.9 | 1.2× slower | 2.0× slower |
| put 32 MiB | 86.4 | 103 | 1.2× slower | 1.2× slower |
| put 64 MiB | 158 | 208 | 1.3× slower | 1.1× slower |
| insert 4 KiB, middle of 1 MiB | 28.7 | 38.0 | 1.3× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 28.9 | 38.5 | 1.3× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 28.2 | 38.2 | 1.4× slower | 1.7× slower |
| delete 4 KiB, start of 1 MiB | 28.0 | 38.0 | 1.4× slower | 1.5× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.2 | 16.7 | 1.4× slower | 2.2× slower |
| patch 16 × 4 KiB in 1 MiB | 27.6 | 38.6 | 1.4× slower | 1.5× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.7 | 17.8 | 1.4× slower | 2.4× slower |
| delete 4 KiB, middle of 1 MiB | 28.9 | 40.5 | 1.4× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 27.8 | 40.3 | 1.5× slower | 1.0× faster |
| write at 4 KiB in 1 MiB | 26.7 | 39.4 | 1.5× slower | 1.6× slower |
| multipart put 64 MiB × 8 MiB | 228 | 343 | 1.5× slower | 2.4× slower |
| put 1 MiB | 14.3 | 33.6 | 2.3× slower | 1.7× slower |
| overwrite 1 MiB | 14.2 | 33.5 | 2.4× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.9 | 37.9 | 2.9× slower | 2.8× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| head | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.42 |  | 0.03 | 1.00 | 0.39 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.44 |  |  | 1.00 | 0.44 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.48 |  | 0.05 | 1.00 | 0.44 |
| insert 4 KiB, start of 64 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| write at 4 KiB in 64 MiB | 64 | 1.39 |  | 0.02 | 1.00 | 0.38 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| delete 4 KiB, start of 32 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| insert 4 KiB, start of 32 MiB | 64 | 1.41 |  |  | 1.02 | 0.39 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.33 |  | 4.38 | 15.28 | 0.67 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.75 |  | 0.22 | 11.89 | 0.64 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.48 |  |  | 27.89 | 0.59 |
| put 32 MiB | 64 | 14.98 |  |  | 14.30 | 0.69 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.74 |  |  | 1.38 | 0.36 |
| delete 4 KiB, start of 1 MiB | 128 | 1.34 |  |  | 1.00 | 0.34 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.56 |  |  | 1.19 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 173.69 | 0.94 | 33.19 | 138.66 | 0.91 |
| overwrite 1 MiB | 400 | 1.56 |  |  | 1.19 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.34 |  |  | 1.00 | 0.34 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.03 |  |  |  | 0.03 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 61.34 | 0.97 | 17.00 | 42.53 | 0.84 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  | 1.00 | 0.09 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
