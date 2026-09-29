# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T141737Z` |
| When | 2026-09-29T14:17:37Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | acfb616 (with uncommitted changes) |
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
| move dir 200 × 64 KiB | 373 | 0.94 | 396× faster | 18× faster |
| rename 64 MiB | 114 | 0.75 | 152× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 190 | 5.1 | 37× faster | 13× faster |
| write at 4 KiB in 64 MiB | 224 | 6.3 | 36× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 210 | 6.0 | 35× faster | 15× faster |
| list 200 keys | 36.0 | 1.2 | 30× faster | 9.1× faster |
| append 4 KiB to 32 MiB | 109 | 4.1 | 27× faster | 8.0× faster |
| insert 4 KiB, start of 64 MiB | 202 | 8.8 | 23× faster | 6.8× faster |
| insert 4 KiB, middle of 64 MiB | 226 | 11.1 | 20× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 100 | 5.1 | 20× faster | 5.9× faster |
| delete 4 KiB, middle of 64 MiB | 240 | 13.1 | 18× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 187 | 10.8 | 17× faster | 6.9× faster |
| write at 4 KiB in 32 MiB | 117 | 6.7 | 17× faster | 5.1× faster |
| delete 4 KiB, start of 32 MiB | 110 | 7.0 | 16× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 115 | 9.3 | 12× faster | 3.5× faster |
| delete 4 KiB, middle of 32 MiB | 106 | 13.2 | 8.0× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 97.6 | 13.6 | 7.2× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.45 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.7 | 0.91 | 5.2× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.2 | 0.27 | 4.4× faster | 34× faster |
| head | 0.80 | 0.19 | 4.1× faster | 13× faster |
| get 4 KiB | 0.90 | 0.22 | 4.1× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.74 | 3.4× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 210 | 102 | 2.1× faster | 2.1× faster |
| multipart put 256 MiB × 16 MiB | 1,176 | 890 | 1.3× faster | 1.8× slower |
| patch 16 × 4 KiB in 32 MiB | 93.4 | 75.5 | 1.2× faster | 1.4× faster |
| get 1 MiB | 1.0 | 0.91 | 1.1× faster | 4.5× faster |
| delete 4 KiB, start of 1 MiB | 4.9 | 4.8 | 1.0× faster | 1.5× slower |
| get 32 MiB | 26.7 | 29.3 | 1.1× slower | 12× faster |
| append 4 KiB to 1 MiB | 4.2 | 4.7 | 1.1× slower | 1.0× faster |
| stream get 256 MiB | 186 | 212 | 1.1× slower | 16× faster |
| stream get 64 MiB | 44.1 | 51.3 | 1.2× slower | 15× faster |
| truncate 4 KiB, end of 1 MiB | 4.2 | 4.9 | 1.2× slower | 2.1× slower |
| get 64 MiB | 45.8 | 53.7 | 1.2× slower | 17× faster |
| write at 4 KiB in 1 MiB | 4.5 | 5.3 | 1.2× slower | 1.6× slower |
| insert 4 KiB, middle of 1 MiB | 4.1 | 4.9 | 1.2× slower | 1.4× slower |
| delete 4 KiB, middle of 1 MiB | 4.4 | 5.4 | 1.2× slower | 1.4× slower |
| multipart put 64 MiB × 8 MiB | 226 | 289 | 1.3× slower | 2.4× slower |
| insert 4 KiB, start of 1 MiB | 4.5 | 6.0 | 1.3× slower | 1.7× slower |
| overwrite 4 KiB | 0.94 | 1.5 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.2 | 9.7 | 1.6× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.0 | 4.9 | 1.6× slower | 2.2× slower |
| put 4 KiB | 0.77 | 1.5 | 2.0× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.6 | 7.4 | 2.1× slower | 2.8× slower |
| put 64 MiB | 150 | 310 | 2.1× slower | 1.1× slower |
| put 32 MiB | 73.6 | 156 | 2.1× slower | 1.2× slower |
| overwrite 1 MiB | 3.1 | 7.7 | 2.5× slower | 1.9× slower |
| put 1 MiB | 3.3 | 8.2 | 2.5× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.1 | 16.4 | 4.1× slower | 1.5× slower |
