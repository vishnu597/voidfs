# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T025937Z` |
| When | 2026-09-30T02:59:37Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 8ce1a94 |
| distance to the bucket | none (loopback) |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.5×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 391 | 0.99 | 395× faster | 18× faster |
| rename 64 MiB | 111 | 0.83 | 134× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 216 | 5.3 | 41× faster | 13× faster |
| append 4 KiB to 64 MiB | 210 | 5.2 | 41× faster | 15× faster |
| list 200 keys | 36.1 | 0.93 | 39× faster | 9.1× faster |
| insert 4 KiB, start of 64 MiB | 193 | 7.7 | 25× faster | 6.8× faster |
| truncate 4 KiB, end of 32 MiB | 104 | 4.6 | 23× faster | 5.9× faster |
| insert 4 KiB, start of 32 MiB | 118 | 5.4 | 22× faster | 3.5× faster |
| delete 4 KiB, middle of 64 MiB | 244 | 12.8 | 19× faster | 11× faster |
| append 4 KiB to 32 MiB | 105 | 5.6 | 19× faster | 8.0× faster |
| write at 4 KiB in 64 MiB | 175 | 9.9 | 18× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 179 | 10.2 | 18× faster | 6.9× faster |
| write at 4 KiB in 32 MiB | 105 | 6.0 | 17× faster | 5.1× faster |
| insert 4 KiB, middle of 64 MiB | 200 | 11.8 | 17× faster | 11× faster |
| insert 4 KiB, middle of 32 MiB | 104 | 10.0 | 10× faster | 3.8× faster |
| delete 4 KiB, start of 32 MiB | 117 | 12.9 | 9.1× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 104 | 11.7 | 8.9× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.46 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.6 | 0.89 | 5.2× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.6× faster | 34× faster |
| get 4 KiB | 0.81 | 0.22 | 3.7× faster | 23× faster |
| head | 0.73 | 0.20 | 3.6× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.74 | 3.5× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 194 | 97.5 | 2.0× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 64 at once | 6.3 | 3.4 | 1.9× faster | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.4 | 2.2 | 1.5× faster | 2.2× slower |
| get 1 MiB | 1.0 | 0.69 | 1.5× faster | 4.5× faster |
| multipart put 256 MiB × 16 MiB | 1,079 | 792 | 1.4× faster | 1.8× slower |
| patch 16 × 4 KiB in 32 MiB | 98.7 | 76.1 | 1.3× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 268 | 225 | 1.2× faster | 2.4× slower |
| overwrite 4 KiB | 1.0 | 0.88 | 1.2× faster | 3.1× slower |
| delete 4 KiB, start of 1 MiB | 4.4 | 4.5 | 1.0× slower | 1.5× slower |
| put 4 KiB | 0.82 | 0.86 | 1.0× slower | 2.0× slower |
| stream get 64 MiB | 48.2 | 51.8 | 1.1× slower | 15× faster |
| stream get 256 MiB | 190 | 207 | 1.1× slower | 16× faster |
| put 32 MiB | 82.6 | 90.5 | 1.1× slower | 1.2× slower |
| get 64 MiB | 47.2 | 52.8 | 1.1× slower | 17× faster |
| truncate 4 KiB, end of 1 MiB | 4.2 | 4.9 | 1.2× slower | 2.1× slower |
| insert 4 KiB, middle of 1 MiB | 4.3 | 5.1 | 1.2× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 4.2 | 5.1 | 1.2× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 4.3 | 5.1 | 1.2× slower | 1.0× faster |
| put 64 MiB | 148 | 182 | 1.2× slower | 1.1× slower |
| insert 4 KiB, start of 1 MiB | 4.3 | 5.4 | 1.3× slower | 1.7× slower |
| write at 4 KiB in 1 MiB | 4.2 | 5.4 | 1.3× slower | 1.6× slower |
| get 32 MiB | 22.3 | 29.7 | 1.3× slower | 12× faster |
| overwrite 1 MiB | 3.5 | 6.0 | 1.7× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.6 | 6.3 | 1.7× slower | 2.8× slower |
| put 1 MiB | 3.0 | 5.9 | 2.0× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.1 | 19.0 | 4.6× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.75 |  | 0.03 |  | 1.00 | 0.72 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.61 |  |  |  | 1.00 | 0.61 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.77 |  | 0.05 |  | 1.00 | 0.72 |
| insert 4 KiB, start of 64 MiB | 64 | 1.70 |  | 0.03 |  | 1.00 | 0.67 |
| write at 4 KiB in 64 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.59 |  |  |  | 1.00 | 0.59 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.83 |  |  |  | 1.00 | 0.83 |
| delete 4 KiB, start of 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| insert 4 KiB, start of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.23 |  | 4.59 |  | 15.73 | 0.91 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.42 |  | 0.08 |  | 11.41 | 0.94 |
| append 4 KiB to 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| put 64 MiB | 64 | 29.45 |  |  |  | 28.47 | 0.98 |
| put 32 MiB | 64 | 15.59 |  |  |  | 14.62 | 0.97 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.64 |  |  |  | 1.00 | 0.64 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.80 |  |  |  | 1.05 | 0.75 |
| delete 4 KiB, start of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| write at 4 KiB in 1 MiB | 128 | 1.54 |  |  |  | 1.00 | 0.54 |
| insert 4 KiB, start of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| put 1 MiB | 400 | 1.76 |  |  |  | 1.17 | 0.59 |
| multipart put 256 MiB × 16 MiB | 32 | 172.81 | 1.00 | 33.00 | 1.00 | 136.84 | 0.97 |
| overwrite 1 MiB | 400 | 1.87 |  |  |  | 1.19 | 0.69 |
| put 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 62.34 | 1.00 | 17.00 | 1.00 | 42.62 | 0.72 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
