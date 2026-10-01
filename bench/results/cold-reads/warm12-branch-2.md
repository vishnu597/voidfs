# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261001T021259Z` |
| When | 2026-10-01T02:12:59Z |
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

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 12.7 | 0.27 | 47× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.0 | 0.26 | 46× faster | 34× faster |
| get 4 KiB | 13.0 | 0.38 | 34× faster | 23× faster |
| list 200 keys | 33.9 | 0.99 | 34× faster | 9.1× faster |
| move dir 200 × 64 KiB | 432 | 13.6 | 32× faster | 18× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.2 | 0.54 | 23× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.1 | 0.91 | 13× faster | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 12.6 | 1.1 | 11× faster | 6.2× faster |
| get 1 MiB | 13.1 | 1.2 | 11× faster | 4.5× faster |
| rename 64 MiB | 119 | 12.5 | 9.5× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 289 | 35.5 | 8.2× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 274 | 33.7 | 8.1× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 300 | 36.9 | 8.1× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 289 | 36.9 | 7.8× faster | 15× faster |
| delete 4 KiB, middle of 64 MiB | 284 | 37.4 | 7.6× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 286 | 38.7 | 7.4× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 268 | 42.1 | 6.4× faster | 11× faster |
| append 4 KiB to 32 MiB | 153 | 36.3 | 4.2× faster | 8.0× faster |
| truncate 4 KiB, end of 32 MiB | 141 | 33.9 | 4.1× faster | 5.9× faster |
| insert 4 KiB, start of 32 MiB | 144 | 35.7 | 4.1× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 138 | 35.2 | 3.9× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 152 | 39.1 | 3.9× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 139 | 38.3 | 3.6× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 151 | 41.8 | 3.6× faster | 3.8× faster |
| stream get 256 MiB | 589 | 188 | 3.1× faster | 16× faster |
| get 32 MiB | 67.6 | 26.3 | 2.6× faster | 12× faster |
| stream get 64 MiB | 135 | 52.8 | 2.6× faster | 15× faster |
| get 64 MiB | 122 | 53.9 | 2.3× faster | 17× faster |
| patch 16 × 4 KiB in 64 MiB | 285 | 130 | 2.2× faster | 2.1× faster |
| multipart put 64 MiB × 8 MiB | 384 | 273 | 1.4× faster | 2.4× slower |
| patch 16 × 4 KiB in 32 MiB | 143 | 106 | 1.4× faster | 1.4× faster |
| put 4 KiB | 12.9 | 14.0 | 1.1× slower | 2.0× slower |
| overwrite 4 KiB | 13.0 | 14.2 | 1.1× slower | 3.1× slower |
| append 4 KiB to 1 MiB | 27.8 | 34.0 | 1.2× slower | 1.0× faster |
| multipart put 256 MiB × 16 MiB | 787 | 979 | 1.2× slower | 1.8× slower |
| delete 4 KiB, middle of 1 MiB | 27.5 | 35.2 | 1.3× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 28.0 | 36.3 | 1.3× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 27.1 | 35.4 | 1.3× slower | 1.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 11.9 | 15.7 | 1.3× slower | 2.2× slower |
| patch 16 × 4 KiB in 1 MiB | 26.5 | 35.1 | 1.3× slower | 1.5× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.3 | 16.3 | 1.3× slower | 2.4× slower |
| truncate 4 KiB, end of 1 MiB | 26.7 | 35.7 | 1.3× slower | 2.1× slower |
| put 32 MiB | 89.1 | 121 | 1.4× slower | 1.2× slower |
| write at 4 KiB in 1 MiB | 26.2 | 36.2 | 1.4× slower | 1.6× slower |
| put 64 MiB | 161 | 226 | 1.4× slower | 1.1× slower |
| insert 4 KiB, start of 1 MiB | 26.7 | 37.9 | 1.4× slower | 1.7× slower |
| overwrite 1 MiB | 13.8 | 32.4 | 2.4× slower | 1.9× slower |
| put 1 MiB | 13.4 | 32.9 | 2.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 13.0 | 36.8 | 2.8× slower | 2.8× slower |

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
| truncate 4 KiB, end of 64 MiB | 64 | 1.34 |  |  | 1.00 | 0.34 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.42 |  |  | 1.00 | 0.42 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| list 200 keys | 400 | 0.00 |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.36 |  |  | 1.00 | 0.36 |
| rename 64 MiB | 64 | 0.16 |  |  |  | 0.16 |
| delete 4 KiB, start of 64 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| insert 4 KiB, start of 64 MiB | 64 | 1.44 |  | 0.03 | 1.00 | 0.41 |
| write at 4 KiB in 64 MiB | 64 | 1.47 |  | 0.05 | 1.00 | 0.42 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.38 |  |  | 1.00 | 0.38 |
| delete 4 KiB, start of 32 MiB | 64 | 1.39 |  |  | 1.00 | 0.39 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.45 |  |  | 1.00 | 0.45 |
| insert 4 KiB, start of 32 MiB | 64 | 1.41 |  |  | 1.00 | 0.41 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.73 |  | 4.86 | 15.14 | 0.73 |
| patch 16 × 4 KiB in 32 MiB | 64 | 11.14 |  | 0.05 | 10.50 | 0.59 |
| append 4 KiB to 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 64 MiB | 64 | 28.61 |  |  | 28.11 | 0.50 |
| put 32 MiB | 64 | 15.34 |  |  | 14.80 | 0.55 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.49 |  |  | 1.14 | 0.35 |
| delete 4 KiB, start of 1 MiB | 128 | 1.34 |  |  | 1.00 | 0.34 |
| write at 4 KiB in 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| insert 4 KiB, start of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| put 1 MiB | 400 | 1.54 |  |  | 1.17 | 0.37 |
| multipart put 256 MiB × 16 MiB | 32 | 172.62 | 0.97 | 33.03 | 137.84 | 0.78 |
| overwrite 1 MiB | 400 | 1.55 |  |  | 1.18 | 0.37 |
| put 4 KiB | 400 | 0.13 |  |  |  | 0.13 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.36 |  |  | 1.00 | 0.36 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.04 |  |  |  | 0.04 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.02 |  |  |  | 0.02 |
| multipart put 64 MiB × 8 MiB | 32 | 59.69 | 0.75 | 17.00 | 41.44 | 0.50 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.10 |  |  | 1.00 | 0.10 |
| overwrite 4 KiB | 400 | 0.14 |  |  |  | 0.14 |
