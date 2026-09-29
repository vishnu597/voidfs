# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T144210Z` |
| When | 2026-09-29T14:42:10Z |
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

voidfs is faster in **29 of 49** scenarios and slower in the other **20**. Geometric mean speed-up over the bare bucket: **3.1×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 388 | 0.95 | 407× faster | 18× faster |
| rename 64 MiB | 102 | 0.76 | 134× faster | 7.9× faster |
| list 200 keys | 35.9 | 1.1 | 32× faster | 9.1× faster |
| append 4 KiB to 64 MiB | 187 | 5.9 | 32× faster | 15× faster |
| delete 4 KiB, start of 64 MiB | 242 | 9.4 | 26× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 234 | 10.2 | 23× faster | 13× faster |
| truncate 4 KiB, end of 32 MiB | 93.7 | 4.1 | 23× faster | 5.9× faster |
| write at 4 KiB in 64 MiB | 210 | 9.7 | 22× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 193 | 9.7 | 20× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 218 | 13.3 | 16× faster | 11× faster |
| insert 4 KiB, middle of 64 MiB | 187 | 12.6 | 15× faster | 11× faster |
| append 4 KiB to 32 MiB | 100.0 | 6.8 | 15× faster | 8.0× faster |
| write at 4 KiB in 32 MiB | 107 | 7.9 | 14× faster | 5.1× faster |
| insert 4 KiB, start of 32 MiB | 116 | 9.5 | 12× faster | 3.5× faster |
| insert 4 KiB, middle of 32 MiB | 88.5 | 7.6 | 12× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 109 | 12.0 | 9.1× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 108 | 12.1 | 8.9× faster | 4.6× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.43 | 5.8× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.4 | 0.89 | 4.9× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.22 | 4.9× faster | 34× faster |
| head | 0.76 | 0.19 | 3.9× faster | 13× faster |
| get 4 KiB | 0.83 | 0.21 | 3.9× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.78 | 3.2× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 194 | 98.7 | 2.0× faster | 2.1× faster |
| get 1 MiB | 1.1 | 0.76 | 1.4× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 101 | 80.4 | 1.3× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,202 | 996 | 1.2× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 301 | 275 | 1.1× faster | 2.4× slower |
| append 4 KiB to 1 MiB | 4.0 | 3.8 | 1.1× faster | 1.0× faster |
| get 32 MiB | 24.6 | 25.8 | 1.1× slower | 12× faster |
| stream get 64 MiB | 45.2 | 48.7 | 1.1× slower | 15× faster |
| insert 4 KiB, start of 1 MiB | 4.4 | 4.9 | 1.1× slower | 1.7× slower |
| write at 4 KiB in 1 MiB | 4.0 | 4.6 | 1.1× slower | 1.6× slower |
| stream get 256 MiB | 173 | 196 | 1.1× slower | 16× faster |
| delete 4 KiB, middle of 1 MiB | 4.0 | 4.6 | 1.2× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 4.0 | 4.7 | 1.2× slower | 2.1× slower |
| delete 4 KiB, start of 1 MiB | 4.3 | 5.3 | 1.2× slower | 1.5× slower |
| insert 4 KiB, middle of 1 MiB | 4.3 | 5.3 | 1.2× slower | 1.4× slower |
| get 64 MiB | 44.9 | 55.5 | 1.2× slower | 17× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.3 | 5.0 | 1.5× slower | 2.2× slower |
| overwrite 4 KiB | 1.2 | 2.0 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.3 | 8.9 | 1.7× slower | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.7 | 8.4 | 1.8× slower | 2.8× slower |
| put 4 KiB | 0.85 | 1.6 | 1.9× slower | 2.0× slower |
| put 64 MiB | 150 | 298 | 2.0× slower | 1.1× slower |
| put 32 MiB | 73.8 | 153 | 2.1× slower | 1.2× slower |
| overwrite 1 MiB | 3.9 | 9.5 | 2.5× slower | 1.9× slower |
| put 1 MiB | 3.0 | 7.8 | 2.6× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.3 | 20.6 | 4.8× slower | 1.5× slower |
