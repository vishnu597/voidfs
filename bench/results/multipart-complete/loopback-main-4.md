# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T234533Z` |
| When | 2026-09-30T23:45:33Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | a03a25d |
| distance to the bucket | none (loopback) |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.8×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 382 | 0.98 | 388× faster | 18× faster |
| rename 64 MiB | 112 | 0.75 | 149× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 246 | 5.1 | 48× faster | 13× faster |
| append 4 KiB to 64 MiB | 186 | 4.2 | 45× faster | 15× faster |
| list 200 keys | 35.9 | 1.0 | 35× faster | 9.1× faster |
| delete 4 KiB, middle of 64 MiB | 226 | 8.1 | 28× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 255 | 10.1 | 25× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 199 | 8.6 | 23× faster | 6.9× faster |
| truncate 4 KiB, end of 32 MiB | 101 | 4.6 | 22× faster | 5.9× faster |
| insert 4 KiB, middle of 64 MiB | 205 | 10.9 | 19× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 125 | 7.4 | 17× faster | 3.5× faster |
| append 4 KiB to 32 MiB | 102 | 6.5 | 16× faster | 8.0× faster |
| write at 4 KiB in 64 MiB | 180 | 11.5 | 16× faster | 6.2× faster |
| write at 4 KiB in 32 MiB | 105 | 8.0 | 13× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 119 | 9.8 | 12× faster | 4.6× faster |
| delete 4 KiB, middle of 32 MiB | 125 | 12.4 | 10× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 108 | 14.0 | 7.8× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.42 | 6.0× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.8 | 0.87 | 5.5× faster | 2.6× faster |
| multipart put 64 MiB × 8 MiB | 1,146 | 228 | 5.0× faster | 2.4× slower |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.9× faster | 34× faster |
| head | 0.77 | 0.17 | 4.6× faster | 13× faster |
| get 4 KiB | 0.83 | 0.21 | 4.0× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.74 | 3.3× faster | 6.2× faster |
| multipart put 256 MiB × 16 MiB | 2,125 | 890 | 2.4× faster | 1.8× slower |
| patch 16 × 4 KiB in 64 MiB | 174 | 106 | 1.6× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 108 | 66.8 | 1.6× faster | 1.4× faster |
| fanout put 1000 × 4 KiB, 64 at once | 4.8 | 3.3 | 1.4× faster | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 2.8 | 2.0 | 1.4× faster | 2.2× slower |
| get 1 MiB | 1.0 | 0.85 | 1.2× faster | 4.5× faster |
| overwrite 4 KiB | 1.0 | 0.88 | 1.1× faster | 3.1× slower |
| get 64 MiB | 49.1 | 52.5 | 1.1× slower | 17× faster |
| stream get 256 MiB | 187 | 203 | 1.1× slower | 16× faster |
| get 32 MiB | 22.9 | 26.2 | 1.1× slower | 12× faster |
| put 4 KiB | 0.83 | 0.95 | 1.1× slower | 2.0× slower |
| stream get 64 MiB | 44.7 | 51.5 | 1.2× slower | 15× faster |
| append 4 KiB to 1 MiB | 4.1 | 4.8 | 1.2× slower | 1.0× faster |
| put 64 MiB | 151 | 176 | 1.2× slower | 1.1× slower |
| write at 4 KiB in 1 MiB | 4.4 | 5.3 | 1.2× slower | 1.6× slower |
| put 32 MiB | 75.7 | 91.8 | 1.2× slower | 1.2× slower |
| truncate 4 KiB, end of 1 MiB | 4.2 | 5.1 | 1.2× slower | 2.1× slower |
| delete 4 KiB, middle of 1 MiB | 4.4 | 5.3 | 1.2× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 4.4 | 5.5 | 1.2× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 4.2 | 5.2 | 1.2× slower | 1.4× slower |
| delete 4 KiB, start of 1 MiB | 3.9 | 5.1 | 1.3× slower | 1.5× slower |
| patch 16 × 4 KiB in 1 MiB | 4.1 | 5.3 | 1.3× slower | 1.5× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.7 | 6.4 | 1.7× slower | 2.8× slower |
| put 1 MiB | 3.1 | 5.6 | 1.8× slower | 1.7× slower |
| overwrite 1 MiB | 3.1 | 5.6 | 1.8× slower | 1.9× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.75 |  | 0.05 |  | 1.00 | 0.70 |
| insert 4 KiB, start of 64 MiB | 64 | 1.70 |  |  |  | 1.00 | 0.70 |
| write at 4 KiB in 64 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.62 |  |  |  | 1.00 | 0.62 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| delete 4 KiB, start of 32 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| insert 4 KiB, start of 32 MiB | 64 | 1.67 |  |  |  | 1.00 | 0.67 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 21.20 |  | 5.05 |  | 15.36 | 0.80 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.88 |  |  |  | 12.08 | 0.80 |
| append 4 KiB to 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| put 64 MiB | 64 | 29.59 |  |  |  | 28.61 | 0.98 |
| put 32 MiB | 64 | 15.27 |  |  |  | 14.31 | 0.95 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.60 |  |  |  | 1.00 | 0.60 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.74 |  |  |  | 1.16 | 0.59 |
| delete 4 KiB, start of 1 MiB | 128 | 1.55 |  |  |  | 1.00 | 0.55 |
| write at 4 KiB in 1 MiB | 128 | 1.54 |  |  |  | 1.00 | 0.54 |
| insert 4 KiB, start of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| put 1 MiB | 400 | 1.82 |  |  |  | 1.14 | 0.68 |
| multipart put 256 MiB × 16 MiB | 32 | 174.41 | 1.00 | 33.06 | 1.00 | 138.47 | 0.88 |
| overwrite 1 MiB | 400 | 1.87 |  |  |  | 1.20 | 0.67 |
| put 4 KiB | 400 | 0.25 |  |  |  |  | 0.25 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.58 |  |  |  | 1.00 | 0.58 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 61.41 | 1.00 | 17.00 | 1.00 | 41.62 | 0.78 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
