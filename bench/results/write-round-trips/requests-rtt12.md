# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T214454Z` |
| When | 2026-09-29T21:44:54Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 41d489e |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **2.7×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.5 | 0.23 | 53× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.1 | 0.27 | 45× faster | 34× faster |
| get 4 KiB | 12.9 | 0.30 | 43× faster | 23× faster |
| list 200 keys | 34.4 | 1.1 | 31× faster | 9.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.3 | 0.47 | 26× faster | 5.3× faster |
| move dir 200 × 64 KiB | 457 | 23.7 | 19× faster | 18× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.0 | 0.99 | 13× faster | 6.2× faster |
| get 1 MiB | 13.2 | 1.1 | 12× faster | 4.5× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.0 | 1.1 | 11× faster | 2.6× faster |
| insert 4 KiB, start of 64 MiB | 316 | 35.9 | 8.8× faster | 6.8× faster |
| append 4 KiB to 64 MiB | 300 | 34.5 | 8.7× faster | 15× faster |
| write at 4 KiB in 64 MiB | 301 | 40.3 | 7.5× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 259 | 35.3 | 7.3× faster | 13× faster |
| delete 4 KiB, start of 64 MiB | 315 | 43.9 | 7.2× faster | 6.9× faster |
| delete 4 KiB, middle of 64 MiB | 296 | 42.5 | 7.0× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 294 | 43.5 | 6.8× faster | 11× faster |
| rename 64 MiB | 122 | 24.4 | 5.0× faster | 7.9× faster |
| insert 4 KiB, middle of 32 MiB | 160 | 36.3 | 4.4× faster | 3.8× faster |
| truncate 4 KiB, end of 32 MiB | 142 | 33.9 | 4.2× faster | 5.9× faster |
| delete 4 KiB, middle of 32 MiB | 154 | 37.1 | 4.1× faster | 4.8× faster |
| insert 4 KiB, start of 32 MiB | 151 | 36.9 | 4.1× faster | 3.5× faster |
| append 4 KiB to 32 MiB | 136 | 33.9 | 4.0× faster | 8.0× faster |
| delete 4 KiB, start of 32 MiB | 139 | 35.7 | 3.9× faster | 4.6× faster |
| write at 4 KiB in 32 MiB | 152 | 41.3 | 3.7× faster | 5.1× faster |
| stream get 256 MiB | 561 | 192 | 2.9× faster | 16× faster |
| stream get 64 MiB | 132 | 49.6 | 2.7× faster | 15× faster |
| get 64 MiB | 126 | 50.5 | 2.5× faster | 17× faster |
| get 32 MiB | 67.7 | 27.9 | 2.4× faster | 12× faster |
| patch 16 × 4 KiB in 64 MiB | 294 | 162 | 1.8× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 136 | 94.4 | 1.4× faster | 1.4× faster |
| put 32 MiB | 104 | 129 | 1.2× slower | 1.2× slower |
| insert 4 KiB, start of 1 MiB | 27.9 | 34.9 | 1.3× slower | 1.7× slower |
| delete 4 KiB, start of 1 MiB | 27.1 | 35.6 | 1.3× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 27.1 | 36.5 | 1.3× slower | 1.6× slower |
| append 4 KiB to 1 MiB | 26.6 | 35.8 | 1.3× slower | 1.0× faster |
| multipart put 64 MiB × 8 MiB | 318 | 432 | 1.4× slower | 2.4× slower |
| truncate 4 KiB, end of 1 MiB | 26.8 | 36.6 | 1.4× slower | 2.1× slower |
| delete 4 KiB, middle of 1 MiB | 26.9 | 37.3 | 1.4× slower | 1.4× slower |
| put 64 MiB | 160 | 222 | 1.4× slower | 1.1× slower |
| insert 4 KiB, middle of 1 MiB | 26.7 | 37.4 | 1.4× slower | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | 26.9 | 44.3 | 1.6× slower | 1.5× slower |
| multipart put 256 MiB × 16 MiB | 527 | 1,107 | 2.1× slower | 1.8× slower |
| overwrite 1 MiB | 13.7 | 32.3 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.5 | 32.7 | 2.4× slower | 1.7× slower |
| put 4 KiB | 12.8 | 33.1 | 2.6× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.4 | 37.9 | 2.8× slower | 2.8× slower |
| overwrite 4 KiB | 12.9 | 37.5 | 2.9× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.1 | 37.3 | 3.1× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.0 | 37.7 | 3.1× slower | 2.2× slower |

## voidfs's requests to the bucket

Per operation, over the measured rounds, from voidfs-server's metrics (including its read of `gc/pending.json` once a minute).

| Scenario | Operations | Requests per operation | delete_prefix | get | list | put | put_new |
|---|--:|--:|--:|--:|--:|--:|--:|
| range 64 KiB of 64 MiB | 400 | 0.00 |  |  |  |  |  |
| get 4 KiB | 400 | 0.00 |  |  |  |  |  |
| move dir 200 × 64 KiB | 32 | 0.25 |  |  |  |  | 0.25 |
| get 64 MiB | 64 | 0.00 |  |  |  |  |  |
| stream get 256 MiB | 32 | 0.00 |  |  |  |  |  |
| stream get 64 MiB | 64 | 0.00 |  |  |  |  |  |
| append 4 KiB to 64 MiB | 64 | 1.34 |  |  |  | 1.00 | 0.34 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.42 |  | 0.03 |  | 1.00 | 0.39 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.45 |  |  |  | 1.00 | 0.45 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  |  | 1.00 | 0.41 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.36 |  |  |  | 1.02 | 0.34 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.47 |  | 0.05 |  | 1.00 | 0.42 |
| insert 4 KiB, start of 64 MiB | 64 | 1.44 |  | 0.02 |  | 1.00 | 0.42 |
| write at 4 KiB in 64 MiB | 64 | 1.45 |  | 0.02 |  | 1.00 | 0.44 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.44 |  |  |  | 1.00 | 0.44 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| delete 4 KiB, start of 32 MiB | 64 | 1.42 |  |  |  | 1.00 | 0.42 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 32 MiB | 64 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.84 |  | 4.88 |  | 15.33 | 0.64 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.08 |  |  |  | 11.39 | 0.69 |
| append 4 KiB to 1 MiB | 128 | 1.37 |  |  |  | 1.01 | 0.36 |
| put 64 MiB | 64 | 28.86 |  |  |  | 28.31 | 0.55 |
| put 32 MiB | 64 | 15.44 |  |  |  | 14.77 | 0.67 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.64 |  |  |  | 1.16 | 0.48 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.54 |  |  |  | 1.17 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 173.12 | 1.00 | 33.03 | 1.00 | 137.31 | 0.78 |
| overwrite 1 MiB | 400 | 1.55 |  |  |  | 1.18 | 0.37 |
| put 4 KiB | 400 | 1.32 |  |  |  | 1.00 | 0.32 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 1.08 |  |  |  | 1.00 | 0.08 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 1.04 |  |  |  | 1.00 | 0.04 |
| multipart put 64 MiB × 8 MiB | 32 | 61.47 | 1.00 | 17.00 | 1.00 | 41.69 | 0.78 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 1.32 |  |  |  | 1.00 | 0.33 |
