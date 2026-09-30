# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T030215Z` |
| When | 2026-09-30T03:02:15Z |
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

voidfs is faster in **28 of 49** scenarios and slower in the other **21**. Geometric mean speed-up over the bare bucket: **3.2×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 388 | 0.95 | 410× faster | 18× faster |
| rename 64 MiB | 109 | 1.1 | 102× faster | 7.9× faster |
| write at 4 KiB in 64 MiB | 226 | 6.1 | 37× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 189 | 5.7 | 33× faster | 15× faster |
| list 200 keys | 38.1 | 1.2 | 33× faster | 9.1× faster |
| delete 4 KiB, middle of 64 MiB | 217 | 9.4 | 23× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 233 | 10.3 | 23× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 209 | 9.5 | 22× faster | 13× faster |
| insert 4 KiB, middle of 64 MiB | 237 | 11.3 | 21× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 217 | 10.5 | 21× faster | 6.8× faster |
| append 4 KiB to 32 MiB | 108 | 5.8 | 19× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 114 | 6.5 | 17× faster | 3.5× faster |
| truncate 4 KiB, end of 32 MiB | 92.6 | 5.7 | 16× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 107 | 9.6 | 11× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 117 | 11.3 | 10× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 106 | 11.5 | 9.2× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 103 | 11.5 | 8.9× faster | 4.6× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.7 | 0.48 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 5.0 | 0.93 | 5.4× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.8× faster | 34× faster |
| get 4 KiB | 0.83 | 0.21 | 3.9× faster | 23× faster |
| head | 0.84 | 0.22 | 3.9× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.80 | 3.3× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 229 | 107 | 2.1× faster | 2.1× faster |
| multipart put 256 MiB × 16 MiB | 1,412 | 935 | 1.5× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 345 | 245 | 1.4× faster | 2.4× slower |
| get 1 MiB | 1.1 | 0.81 | 1.4× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 106 | 82.2 | 1.3× faster | 1.4× faster |
| stream get 256 MiB | 196 | 216 | 1.1× slower | 16× faster |
| append 4 KiB to 1 MiB | 4.9 | 5.4 | 1.1× slower | 1.0× faster |
| get 32 MiB | 25.3 | 29.0 | 1.1× slower | 12× faster |
| stream get 64 MiB | 48.5 | 55.7 | 1.1× slower | 15× faster |
| put 32 MiB | 97.5 | 113 | 1.2× slower | 1.2× slower |
| get 64 MiB | 50.8 | 59.1 | 1.2× slower | 17× faster |
| put 64 MiB | 193 | 225 | 1.2× slower | 1.1× slower |
| insert 4 KiB, start of 1 MiB | 4.3 | 5.1 | 1.2× slower | 1.7× slower |
| delete 4 KiB, middle of 1 MiB | 5.4 | 6.4 | 1.2× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 4.3 | 5.3 | 1.2× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 5.2 | 6.9 | 1.3× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 4.2 | 5.8 | 1.4× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 5.1 | 7.0 | 1.4× slower | 1.4× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.9 | 9.8 | 1.4× slower | 2.4× slower |
| overwrite 4 KiB | 1.0 | 1.5 | 1.5× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.5 | 6.2 | 1.8× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.4 | 6.0 | 1.8× slower | 2.2× slower |
| put 1 MiB | 3.1 | 5.8 | 1.9× slower | 1.7× slower |
| overwrite 1 MiB | 4.4 | 8.2 | 1.9× slower | 1.9× slower |
| put 4 KiB | 1.1 | 2.2 | 2.0× slower | 2.0× slower |
| patch 16 × 4 KiB in 1 MiB | 4.5 | 26.4 | 5.9× slower | 1.5× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.72 |  | 0.03 |  | 1.00 | 0.69 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.78 |  | 0.03 |  | 1.00 | 0.75 |
| insert 4 KiB, start of 64 MiB | 64 | 1.80 |  | 0.03 |  | 1.00 | 0.77 |
| write at 4 KiB in 64 MiB | 64 | 1.73 |  | 0.03 |  | 1.00 | 0.70 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| delete 4 KiB, start of 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| insert 4 KiB, start of 32 MiB | 64 | 1.83 |  |  |  | 1.00 | 0.83 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.28 |  | 5.05 |  | 15.44 | 0.80 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.86 |  |  |  | 11.94 | 0.92 |
| append 4 KiB to 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| put 64 MiB | 64 | 28.89 |  | 0.02 |  | 27.94 | 0.94 |
| put 32 MiB | 64 | 15.78 |  |  |  | 14.78 | 1.00 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.69 |  |  |  | 1.04 | 0.65 |
| delete 4 KiB, start of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| write at 4 KiB in 1 MiB | 128 | 1.52 |  |  |  | 1.00 | 0.52 |
| insert 4 KiB, start of 1 MiB | 128 | 1.61 |  |  |  | 1.00 | 0.61 |
| put 1 MiB | 400 | 1.96 |  |  |  | 1.17 | 0.79 |
| multipart put 256 MiB × 16 MiB | 32 | 173.97 | 1.00 | 33.00 | 1.00 | 138.06 | 0.91 |
| overwrite 1 MiB | 400 | 1.82 |  |  |  | 1.14 | 0.69 |
| put 4 KiB | 400 | 1.40 |  |  |  | 1.00 | 0.40 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.53 |  |  |  | 1.00 | 0.53 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 1.09 |  |  |  | 1.00 | 0.09 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 1.05 |  |  |  | 1.00 | 0.05 |
| multipart put 64 MiB × 8 MiB | 32 | 61.47 | 1.00 | 17.00 | 1.00 | 41.75 | 0.72 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.13 |  |  |  | 1.00 | 0.13 |
| overwrite 4 KiB | 400 | 1.39 |  |  |  | 1.00 | 0.39 |
