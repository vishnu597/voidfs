# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T191145Z` |
| When | 2026-09-30T19:11:45Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 10feda4 |
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
| head | 12.3 | 0.29 | 43× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.0 | 0.28 | 42× faster | 34× faster |
| get 4 KiB | 12.3 | 0.32 | 38× faster | 23× faster |
| list 200 keys | 37.6 | 1.1 | 34× faster | 9.1× faster |
| move dir 200 × 64 KiB | 459 | 14.2 | 32× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.2 | 0.47 | 26× faster | 5.3× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.0 | 1.0 | 12× faster | 6.2× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.1 | 1.0 | 12× faster | 2.6× faster |
| get 1 MiB | 13.2 | 1.2 | 11× faster | 4.5× faster |
| rename 64 MiB | 124 | 12.5 | 9.9× faster | 7.9× faster |
| delete 4 KiB, start of 64 MiB | 301 | 36.6 | 8.2× faster | 6.9× faster |
| append 4 KiB to 64 MiB | 285 | 34.8 | 8.2× faster | 15× faster |
| insert 4 KiB, start of 64 MiB | 282 | 34.8 | 8.1× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 275 | 34.4 | 8.0× faster | 13× faster |
| write at 4 KiB in 64 MiB | 276 | 35.9 | 7.7× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 255 | 35.0 | 7.3× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 286 | 42.8 | 6.7× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 150 | 35.4 | 4.2× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 150 | 36.0 | 4.2× faster | 5.9× faster |
| insert 4 KiB, middle of 32 MiB | 149 | 36.2 | 4.1× faster | 3.8× faster |
| multipart put 256 MiB × 16 MiB | 4,617 | 1,133 | 4.1× faster | 1.8× slower |
| delete 4 KiB, start of 32 MiB | 145 | 36.0 | 4.0× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 142 | 35.3 | 4.0× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 150 | 37.8 | 4.0× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 146 | 44.9 | 3.3× faster | 4.8× faster |
| stream get 256 MiB | 569 | 200 | 2.8× faster | 16× faster |
| stream get 64 MiB | 134 | 50.6 | 2.6× faster | 15× faster |
| get 32 MiB | 66.4 | 25.2 | 2.6× faster | 12× faster |
| get 64 MiB | 122 | 52.1 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 289 | 141 | 2.1× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 141 | 97.0 | 1.4× faster | 1.4× faster |
| overwrite 4 KiB | 11.9 | 14.3 | 1.2× slower | 3.1× slower |
| put 4 KiB | 11.8 | 14.3 | 1.2× slower | 2.0× slower |
| insert 4 KiB, middle of 1 MiB | 27.4 | 35.2 | 1.3× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 26.8 | 34.7 | 1.3× slower | 1.0× faster |
| multipart put 64 MiB × 8 MiB | 290 | 376 | 1.3× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 11.8 | 15.5 | 1.3× slower | 2.2× slower |
| write at 4 KiB in 1 MiB | 27.4 | 36.5 | 1.3× slower | 1.6× slower |
| put 64 MiB | 161 | 218 | 1.4× slower | 1.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 11.9 | 16.1 | 1.4× slower | 2.4× slower |
| delete 4 KiB, start of 1 MiB | 27.0 | 36.9 | 1.4× slower | 1.5× slower |
| put 32 MiB | 87.6 | 120 | 1.4× slower | 1.2× slower |
| delete 4 KiB, middle of 1 MiB | 26.2 | 36.4 | 1.4× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 26.5 | 37.0 | 1.4× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 26.7 | 37.2 | 1.4× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 26.3 | 45.0 | 1.7× slower | 1.5× slower |
| overwrite 1 MiB | 13.7 | 32.3 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.5 | 32.5 | 2.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.0 | 36.9 | 2.8× slower | 2.8× slower |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | delete_prefix | get | list | put | put_new |
|---|--:|--:|--:|--:|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.00 |  |  |  |  |  |
| get 4 KiB | 400 | 0.00 |  |  |  |  |  |
| move dir 200 × 64 KiB | 32 | 0.19 |  |  |  |  | 0.19 |
| get 64 MiB | 64 | 0.00 |  |  |  |  |  |
| stream get 256 MiB | 32 | 0.00 |  |  |  |  |  |
| stream get 64 MiB | 64 | 0.00 |  |  |  |  |  |
| append 4 KiB to 64 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.39 |  | 0.03 |  | 1.00 | 0.36 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| rename 64 MiB | 64 | 0.16 |  |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.44 |  | 0.03 |  | 1.00 | 0.41 |
| insert 4 KiB, start of 64 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| write at 4 KiB in 64 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.38 |  |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| delete 4 KiB, start of 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.39 |  |  |  | 1.00 | 0.39 |
| insert 4 KiB, start of 32 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.39 |  | 5.73 |  | 15.05 | 0.61 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.05 |  | 0.03 |  | 11.33 | 0.69 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 29.25 |  |  |  | 28.77 | 0.48 |
| put 32 MiB | 64 | 15.20 |  |  |  | 14.66 | 0.55 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.61 |  |  |  | 1.16 | 0.45 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.53 |  |  |  | 1.16 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 174.00 | 1.00 | 33.03 | 1.00 | 138.12 | 0.84 |
| overwrite 1 MiB | 400 | 1.57 |  |  |  | 1.20 | 0.37 |
| put 4 KiB | 400 | 0.13 |  |  |  |  | 0.13 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.34 |  |  |  | 1.00 | 0.34 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 62.78 | 1.00 | 17.00 | 1.00 | 43.06 | 0.72 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  |  | 0.14 |
