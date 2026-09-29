# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T143202Z` |
| When | 2026-09-29T14:32:02Z |
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

voidfs is faster in **28 of 49** scenarios and slower in the other **21**. Geometric mean speed-up over the bare bucket: **3.1×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 397 | 0.95 | 416× faster | 18× faster |
| rename 64 MiB | 102 | 0.74 | 138× faster | 7.9× faster |
| list 200 keys | 35.3 | 1.0 | 35× faster | 9.1× faster |
| append 4 KiB to 64 MiB | 178 | 6.0 | 30× faster | 15× faster |
| truncate 4 KiB, end of 64 MiB | 173 | 7.7 | 23× faster | 13× faster |
| delete 4 KiB, middle of 64 MiB | 230 | 10.5 | 22× faster | 11× faster |
| insert 4 KiB, start of 64 MiB | 253 | 11.7 | 22× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 227 | 10.5 | 22× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 184 | 9.1 | 20× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 214 | 10.6 | 20× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 113 | 6.9 | 16× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 114 | 7.3 | 16× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 98.9 | 6.4 | 15× faster | 8.0× faster |
| delete 4 KiB, middle of 32 MiB | 110 | 7.5 | 15× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 104 | 7.8 | 13× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 97.2 | 7.4 | 13× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 109 | 9.5 | 11× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.45 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.8 | 0.90 | 5.4× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.24 | 4.5× faster | 34× faster |
| head | 0.75 | 0.18 | 4.2× faster | 13× faster |
| get 4 KiB | 0.79 | 0.21 | 3.8× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.78 | 3.2× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 195 | 97.8 | 2.0× faster | 2.1× faster |
| get 1 MiB | 1.0 | 0.67 | 1.5× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 101 | 78.1 | 1.3× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,149 | 941 | 1.2× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 292 | 283 | 1.0× faster | 2.4× slower |
| get 32 MiB | 26.4 | 26.4 | 1.0× slower | 12× faster |
| stream get 64 MiB | 46.2 | 50.6 | 1.1× slower | 15× faster |
| stream get 256 MiB | 185 | 204 | 1.1× slower | 16× faster |
| get 64 MiB | 48.1 | 52.9 | 1.1× slower | 17× faster |
| insert 4 KiB, start of 1 MiB | 4.5 | 5.1 | 1.1× slower | 1.7× slower |
| append 4 KiB to 1 MiB | 4.2 | 4.7 | 1.1× slower | 1.0× faster |
| write at 4 KiB in 1 MiB | 4.0 | 4.5 | 1.1× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 4.4 | 5.4 | 1.2× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 4.2 | 5.2 | 1.2× slower | 2.1× slower |
| delete 4 KiB, start of 1 MiB | 4.0 | 5.0 | 1.2× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 4.3 | 5.8 | 1.4× slower | 1.4× slower |
| overwrite 4 KiB | 1.2 | 1.9 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.0 | 5.1 | 1.7× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.1 | 8.8 | 1.7× slower | 2.4× slower |
| put 4 KiB | 0.89 | 1.7 | 1.9× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.4 | 8.3 | 1.9× slower | 2.8× slower |
| put 64 MiB | 148 | 298 | 2.0× slower | 1.1× slower |
| put 32 MiB | 71.9 | 153 | 2.1× slower | 1.2× slower |
| overwrite 1 MiB | 3.7 | 9.2 | 2.5× slower | 1.9× slower |
| put 1 MiB | 2.9 | 7.4 | 2.6× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.1 | 20.4 | 5.0× slower | 1.5× slower |
