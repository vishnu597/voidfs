# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T021025Z` |
| When | 2026-10-01T02:10:25Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **2.9×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.1 | 0.23 | 56× faster | 13× faster |
| range 64 KiB of 64 MiB | 13.5 | 0.30 | 45× faster | 34× faster |
| get 4 KiB | 13.7 | 0.33 | 41× faster | 23× faster |
| list 200 keys | 33.3 | 1.1 | 30× faster | 9.1× faster |
| move dir 200 × 64 KiB | 473 | 16.1 | 29× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 13.1 | 0.51 | 26× faster | 5.3× faster |
| get 1 MiB | 14.4 | 1.1 | 13× faster | 4.5× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.1 | 0.94 | 13× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 13.9 | 1.1 | 13× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 314 | 36.9 | 8.5× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 292 | 36.2 | 8.1× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 297 | 37.9 | 7.8× faster | 11× faster |
| truncate 4 KiB, end of 64 MiB | 299 | 38.3 | 7.8× faster | 13× faster |
| append 4 KiB to 64 MiB | 268 | 35.7 | 7.5× faster | 15× faster |
| rename 64 MiB | 123 | 17.4 | 7.1× faster | 7.9× faster |
| write at 4 KiB in 64 MiB | 254 | 36.3 | 7.0× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 290 | 44.0 | 6.6× faster | 11× faster |
| delete 4 KiB, middle of 32 MiB | 149 | 34.6 | 4.3× faster | 4.8× faster |
| append 4 KiB to 32 MiB | 145 | 36.6 | 4.0× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 142 | 36.6 | 3.9× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 149 | 38.5 | 3.9× faster | 5.1× faster |
| truncate 4 KiB, end of 32 MiB | 143 | 37.3 | 3.8× faster | 5.9× faster |
| delete 4 KiB, start of 32 MiB | 146 | 39.0 | 3.7× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 136 | 41.2 | 3.3× faster | 3.8× faster |
| stream get 256 MiB | 573 | 201 | 2.8× faster | 16× faster |
| stream get 64 MiB | 133 | 50.2 | 2.6× faster | 15× faster |
| get 32 MiB | 68.2 | 25.9 | 2.6× faster | 12× faster |
| get 64 MiB | 122 | 53.4 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 275 | 137 | 2.0× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 138 | 86.6 | 1.6× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 309 | 263 | 1.2× faster | 2.4× slower |
| overwrite 4 KiB | 12.8 | 13.9 | 1.1× slower | 3.1× slower |
| put 4 KiB | 12.6 | 13.9 | 1.1× slower | 2.0× slower |
| put 32 MiB | 92.5 | 116 | 1.3× slower | 1.2× slower |
| put 64 MiB | 163 | 205 | 1.3× slower | 1.1× slower |
| append 4 KiB to 1 MiB | 27.7 | 36.4 | 1.3× slower | 1.0× faster |
| fanout put 1000 × 4 KiB, 32 at once | 12.1 | 16.0 | 1.3× slower | 2.2× slower |
| truncate 4 KiB, end of 1 MiB | 26.1 | 34.7 | 1.3× slower | 2.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.2 | 16.4 | 1.3× slower | 2.4× slower |
| insert 4 KiB, middle of 1 MiB | 28.5 | 38.9 | 1.4× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 28.1 | 38.7 | 1.4× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 28.2 | 39.2 | 1.4× slower | 1.7× slower |
| multipart put 256 MiB × 16 MiB | 680 | 960 | 1.4× slower | 1.8× slower |
| delete 4 KiB, start of 1 MiB | 27.7 | 40.1 | 1.4× slower | 1.5× slower |
| patch 16 × 4 KiB in 1 MiB | 26.7 | 38.8 | 1.5× slower | 1.5× slower |
| write at 4 KiB in 1 MiB | 27.0 | 39.3 | 1.5× slower | 1.6× slower |
| overwrite 1 MiB | 13.8 | 32.7 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.5 | 32.7 | 2.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.3 | 36.2 | 2.7× slower | 2.8× slower |

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
| truncate 4 KiB, end of 64 MiB | 64 | 1.44 |  | 0.03 | 1.00 | 0.41 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.44 |  |  | 1.00 | 0.44 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| rename 64 MiB | 64 | 0.17 |  |  |  | 0.17 |
| delete 4 KiB, start of 64 MiB | 64 | 1.41 |  | 0.05 | 1.00 | 0.36 |
| insert 4 KiB, start of 64 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| write at 4 KiB in 64 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| delete 4 KiB, start of 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| insert 4 KiB, start of 32 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.89 |  | 5.50 | 15.69 | 0.70 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.67 |  | 0.03 | 10.95 | 0.69 |
| append 4 KiB to 1 MiB | 128 | 1.37 |  |  | 1.01 | 0.36 |
| put 64 MiB | 64 | 29.27 |  |  | 28.50 | 0.77 |
| put 32 MiB | 64 | 14.61 |  |  | 14.00 | 0.61 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.37 |  |  | 1.01 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.49 |  |  | 1.13 | 0.36 |
| delete 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.35 |  |  | 1.00 | 0.35 |
| put 1 MiB | 400 | 1.55 |  |  | 1.18 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 172.53 | 0.84 | 33.03 | 138.03 | 0.62 |
| overwrite 1 MiB | 400 | 1.55 |  |  | 1.18 | 0.37 |
| put 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 61.69 | 0.94 | 17.00 | 42.94 | 0.81 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.09 |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 0.13 |  |  |  | 0.13 |
