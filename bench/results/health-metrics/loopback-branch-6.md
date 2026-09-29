# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T144338Z` |
| When | 2026-09-29T14:43:38Z |
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

voidfs is faster in **27 of 49** scenarios and slower in the other **22**. Geometric mean speed-up over the bare bucket: **3.1×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 363 | 0.89 | 407× faster | 18× faster |
| rename 64 MiB | 106 | 0.75 | 141× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 202 | 4.6 | 43× faster | 15× faster |
| list 200 keys | 36.0 | 1.1 | 34× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 180 | 5.9 | 30× faster | 13× faster |
| insert 4 KiB, start of 64 MiB | 222 | 8.0 | 28× faster | 6.8× faster |
| truncate 4 KiB, end of 32 MiB | 111 | 4.7 | 23× faster | 5.9× faster |
| write at 4 KiB in 64 MiB | 181 | 8.3 | 22× faster | 6.2× faster |
| delete 4 KiB, middle of 64 MiB | 226 | 13.1 | 17× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 183 | 10.8 | 17× faster | 6.9× faster |
| insert 4 KiB, start of 32 MiB | 114 | 8.3 | 14× faster | 3.5× faster |
| insert 4 KiB, middle of 64 MiB | 188 | 14.3 | 13× faster | 11× faster |
| delete 4 KiB, start of 32 MiB | 95.3 | 9.2 | 10× faster | 4.6× faster |
| write at 4 KiB in 32 MiB | 92.2 | 8.9 | 10× faster | 5.1× faster |
| append 4 KiB to 32 MiB | 96.7 | 9.4 | 10× faster | 8.0× faster |
| insert 4 KiB, middle of 32 MiB | 115 | 11.6 | 9.9× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 104 | 12.3 | 8.5× faster | 4.8× faster |
| range 64 KiB of 64 MiB | 1.3 | 0.22 | 5.7× faster | 34× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 0.47 | 5.4× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.5 | 0.91 | 5.0× faster | 2.6× faster |
| head | 0.77 | 0.19 | 4.1× faster | 13× faster |
| get 4 KiB | 0.83 | 0.20 | 4.1× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.74 | 3.6× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 240 | 103 | 2.3× faster | 2.1× faster |
| get 1 MiB | 1.0 | 0.67 | 1.5× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 118 | 79.9 | 1.5× faster | 1.4× faster |
| multipart put 64 MiB × 8 MiB | 326 | 316 | 1.0× faster | 2.4× slower |
| multipart put 256 MiB × 16 MiB | 1,321 | 1,333 | 1.0× slower | 1.8× slower |
| stream get 64 MiB | 49.0 | 51.3 | 1.0× slower | 15× faster |
| stream get 256 MiB | 184 | 202 | 1.1× slower | 16× faster |
| get 32 MiB | 22.5 | 24.9 | 1.1× slower | 12× faster |
| delete 4 KiB, middle of 1 MiB | 5.3 | 5.9 | 1.1× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.2 | 4.9 | 1.2× slower | 1.6× slower |
| append 4 KiB to 1 MiB | 4.8 | 5.6 | 1.2× slower | 1.0× faster |
| insert 4 KiB, start of 1 MiB | 4.6 | 5.4 | 1.2× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 5.2 | 6.3 | 1.2× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 4.9 | 6.2 | 1.3× slower | 2.1× slower |
| delete 4 KiB, start of 1 MiB | 4.5 | 5.8 | 1.3× slower | 1.5× slower |
| get 64 MiB | 42.8 | 56.3 | 1.3× slower | 17× faster |
| fanout put 1000 × 4 KiB, 32 at once | 3.7 | 5.6 | 1.5× slower | 2.2× slower |
| overwrite 4 KiB | 1.4 | 2.3 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.6 | 9.2 | 1.7× slower | 2.4× slower |
| put 64 MiB | 187 | 359 | 1.9× slower | 1.1× slower |
| put 32 MiB | 92.1 | 182 | 2.0× slower | 1.2× slower |
| put 4 KiB | 1.1 | 2.1 | 2.0× slower | 2.0× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.4 | 9.1 | 2.1× slower | 2.8× slower |
| overwrite 1 MiB | 4.3 | 10.6 | 2.5× slower | 1.9× slower |
| put 1 MiB | 3.4 | 8.7 | 2.6× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.7 | 19.4 | 4.1× slower | 1.5× slower |
