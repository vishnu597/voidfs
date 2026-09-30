# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260930T191944Z` |
| When | 2026-09-30T19:19:44Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 10feda4 |
| distance to the bucket | none (loopback) |
| pool features | inline-data |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **31 of 49** scenarios and slower in the other **18**. Geometric mean speed-up over the bare bucket: **3.5×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 381 | 1.0 | 370× faster | 18× faster |
| rename 64 MiB | 108 | 0.73 | 148× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 228 | 6.1 | 38× faster | 15× faster |
| list 200 keys | 34.9 | 1.0 | 34× faster | 9.1× faster |
| insert 4 KiB, start of 64 MiB | 179 | 5.4 | 33× faster | 6.8× faster |
| truncate 4 KiB, end of 64 MiB | 200 | 7.3 | 27× faster | 13× faster |
| truncate 4 KiB, end of 32 MiB | 118 | 5.4 | 22× faster | 5.9× faster |
| write at 4 KiB in 64 MiB | 192 | 10.2 | 19× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 197 | 10.9 | 18× faster | 6.9× faster |
| delete 4 KiB, start of 32 MiB | 122 | 7.0 | 17× faster | 4.6× faster |
| delete 4 KiB, middle of 64 MiB | 200 | 11.5 | 17× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 105 | 7.8 | 13× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 116 | 9.0 | 13× faster | 5.1× faster |
| insert 4 KiB, middle of 64 MiB | 177 | 15.4 | 12× faster | 11× faster |
| append 4 KiB to 32 MiB | 112 | 10.1 | 11× faster | 8.0× faster |
| delete 4 KiB, middle of 32 MiB | 95.9 | 10.9 | 8.8× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 95.6 | 11.5 | 8.3× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.43 | 5.8× faster | 5.3× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.22 | 5.0× faster | 34× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.2 | 0.86 | 4.8× faster | 2.6× faster |
| get 4 KiB | 0.82 | 0.20 | 4.2× faster | 23× faster |
| head | 0.79 | 0.20 | 4.0× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.87 | 3.0× faster | 6.2× faster |
| multipart put 256 MiB × 16 MiB | 2,258 | 1,069 | 2.1× faster | 1.8× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.4 | 3.1 | 2.1× faster | 2.4× slower |
| patch 16 × 4 KiB in 64 MiB | 184 | 93.2 | 2.0× faster | 2.1× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.3 | 1.9 | 1.7× faster | 2.2× slower |
| patch 16 × 4 KiB in 32 MiB | 98.3 | 64.8 | 1.5× faster | 1.4× faster |
| overwrite 4 KiB | 0.96 | 0.82 | 1.2× faster | 3.1× slower |
| multipart put 64 MiB × 8 MiB | 243 | 208 | 1.2× faster | 2.4× slower |
| get 1 MiB | 1.0 | 0.90 | 1.2× faster | 4.5× faster |
| put 4 KiB | 0.82 | 0.88 | 1.1× slower | 2.0× slower |
| get 32 MiB | 23.4 | 25.0 | 1.1× slower | 12× faster |
| put 32 MiB | 91.3 | 101 | 1.1× slower | 1.2× slower |
| delete 4 KiB, middle of 1 MiB | 5.0 | 5.9 | 1.2× slower | 1.4× slower |
| get 64 MiB | 40.2 | 47.6 | 1.2× slower | 17× faster |
| stream get 256 MiB | 168 | 198 | 1.2× slower | 16× faster |
| stream get 64 MiB | 40.9 | 48.5 | 1.2× slower | 15× faster |
| append 4 KiB to 1 MiB | 4.6 | 5.5 | 1.2× slower | 1.0× faster |
| put 64 MiB | 162 | 197 | 1.2× slower | 1.1× slower |
| write at 4 KiB in 1 MiB | 4.1 | 5.0 | 1.2× slower | 1.6× slower |
| patch 16 × 4 KiB in 1 MiB | 4.3 | 5.3 | 1.2× slower | 1.5× slower |
| delete 4 KiB, start of 1 MiB | 4.2 | 5.2 | 1.2× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 4.7 | 5.9 | 1.2× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 4.1 | 5.2 | 1.3× slower | 2.1× slower |
| insert 4 KiB, start of 1 MiB | 4.0 | 5.8 | 1.4× slower | 1.7× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.6 | 6.3 | 1.7× slower | 2.8× slower |
| put 1 MiB | 3.2 | 5.7 | 1.8× slower | 1.7× slower |
| overwrite 1 MiB | 3.0 | 5.5 | 1.8× slower | 1.9× slower |

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
| append 4 KiB to 64 MiB | 64 | 1.64 |  |  |  | 1.00 | 0.64 |
| head | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 64 MiB | 64 | 1.66 |  |  |  | 1.00 | 0.66 |
| get 32 MiB | 64 | 0.00 |  |  |  |  |  |
| delete 4 KiB, middle of 64 MiB | 64 | 1.77 |  |  |  | 1.00 | 0.77 |
| insert 4 KiB, middle of 64 MiB | 64 | 1.75 |  |  |  | 1.00 | 0.75 |
| list 200 keys | 400 | 0.00 |  |  |  |  |  |
| append 4 KiB to 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| rename 64 MiB | 64 | 0.25 |  |  |  |  | 0.25 |
| delete 4 KiB, start of 64 MiB | 64 | 1.81 |  | 0.05 |  | 1.00 | 0.77 |
| insert 4 KiB, start of 64 MiB | 64 | 1.78 |  |  |  | 1.00 | 0.78 |
| write at 4 KiB in 64 MiB | 64 | 1.81 |  | 0.05 |  | 1.00 | 0.77 |
| fanout get 200 × 256 KiB, 32 at once | 400 | 0.00 |  |  |  |  |  |
| truncate 4 KiB, end of 32 MiB | 64 | 1.61 |  |  |  | 1.00 | 0.61 |
| fanout get 1000 × 4 KiB, 32 at once | 2000 | 0.00 |  |  |  |  |  |
| write at 4 KiB in 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| delete 4 KiB, middle of 32 MiB | 64 | 1.72 |  |  |  | 1.00 | 0.72 |
| delete 4 KiB, start of 32 MiB | 64 | 1.73 |  |  |  | 1.00 | 0.73 |
| get 1 MiB | 400 | 0.00 |  |  |  |  |  |
| insert 4 KiB, middle of 32 MiB | 64 | 1.77 |  |  |  | 1.02 | 0.75 |
| insert 4 KiB, start of 32 MiB | 64 | 1.69 |  |  |  | 1.00 | 0.69 |
| fanout get 1000 × 4 KiB, 64 at once | 2000 | 0.00 |  |  |  |  |  |
| patch 16 × 4 KiB in 64 MiB | 64 | 20.86 |  | 4.70 |  | 15.25 | 0.91 |
| patch 16 × 4 KiB in 32 MiB | 64 | 12.64 |  | 0.06 |  | 11.73 | 0.84 |
| append 4 KiB to 1 MiB | 128 | 1.63 |  |  |  | 1.00 | 0.63 |
| put 64 MiB | 64 | 29.73 |  |  |  | 28.81 | 0.92 |
| put 32 MiB | 64 | 15.58 |  |  |  | 14.61 | 0.97 |
| delete 4 KiB, middle of 1 MiB | 128 | 1.62 |  |  |  | 1.00 | 0.62 |
| insert 4 KiB, middle of 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| patch 16 × 4 KiB in 1 MiB | 128 | 1.95 |  |  |  | 1.30 | 0.64 |
| delete 4 KiB, start of 1 MiB | 128 | 1.56 |  |  |  | 1.00 | 0.56 |
| write at 4 KiB in 1 MiB | 128 | 1.57 |  |  |  | 1.00 | 0.57 |
| insert 4 KiB, start of 1 MiB | 128 | 1.59 |  |  |  | 1.00 | 0.59 |
| put 1 MiB | 400 | 1.86 |  |  |  | 1.17 | 0.69 |
| multipart put 256 MiB × 16 MiB | 32 | 172.59 | 1.00 | 33.12 | 1.00 | 136.62 | 0.84 |
| overwrite 1 MiB | 400 | 1.92 |  |  |  | 1.14 | 0.78 |
| put 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
| truncate 4 KiB, end of 1 MiB | 128 | 1.56 |  |  |  | 1.00 | 0.56 |
| fanout put 1000 × 4 KiB, 32 at once | 2000 | 0.06 |  |  |  |  | 0.06 |
| fanout put 1000 × 4 KiB, 64 at once | 2000 | 0.03 |  |  |  |  | 0.03 |
| multipart put 64 MiB × 8 MiB | 32 | 62.06 | 1.00 | 17.00 | 1.00 | 42.28 | 0.78 |
| fanout put 200 × 256 KiB, 32 at once | 400 | 1.12 |  |  |  | 1.00 | 0.12 |
| overwrite 4 KiB | 400 | 0.26 |  |  |  |  | 0.26 |
