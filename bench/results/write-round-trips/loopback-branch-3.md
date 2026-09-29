# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T213854Z` |
| When | 2026-09-29T21:38:54Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 41d489e |
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
| move dir 200 × 64 KiB | 377 | 0.98 | 385× faster | 18× faster |
| rename 64 MiB | 107 | 0.80 | 133× faster | 7.9× faster |
| append 4 KiB to 64 MiB | 187 | 2.8 | 66× faster | 15× faster |
| list 200 keys | 37.0 | 1.1 | 35× faster | 9.1× faster |
| truncate 4 KiB, end of 64 MiB | 183 | 5.7 | 32× faster | 13× faster |
| append 4 KiB to 32 MiB | 104 | 3.7 | 28× faster | 8.0× faster |
| write at 4 KiB in 64 MiB | 208 | 9.5 | 22× faster | 6.2× faster |
| insert 4 KiB, start of 64 MiB | 208 | 9.5 | 22× faster | 6.8× faster |
| delete 4 KiB, middle of 64 MiB | 212 | 11.3 | 19× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 216 | 12.6 | 17× faster | 6.9× faster |
| truncate 4 KiB, end of 32 MiB | 95.3 | 6.0 | 16× faster | 5.9× faster |
| write at 4 KiB in 32 MiB | 102 | 6.8 | 15× faster | 5.1× faster |
| insert 4 KiB, middle of 64 MiB | 192 | 14.3 | 13× faster | 11× faster |
| delete 4 KiB, start of 32 MiB | 106 | 10.2 | 10× faster | 4.6× faster |
| insert 4 KiB, start of 32 MiB | 97.9 | 9.8 | 10× faster | 3.5× faster |
| delete 4 KiB, middle of 32 MiB | 102 | 12.5 | 8.2× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 107 | 13.1 | 8.1× faster | 3.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.7 | 0.47 | 5.6× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.7 | 0.94 | 5.0× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.9× faster | 34× faster |
| get 4 KiB | 0.82 | 0.21 | 3.9× faster | 23× faster |
| head | 0.74 | 0.20 | 3.8× faster | 13× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 0.80 | 3.1× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 215 | 104 | 2.1× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 123 | 78.7 | 1.6× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,265 | 915 | 1.4× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 302 | 228 | 1.3× faster | 2.4× slower |
| get 1 MiB | 0.98 | 0.74 | 1.3× faster | 4.5× faster |
| delete 4 KiB, start of 1 MiB | 4.4 | 4.6 | 1.1× slower | 1.5× slower |
| stream get 256 MiB | 180 | 198 | 1.1× slower | 16× faster |
| put 64 MiB | 189 | 209 | 1.1× slower | 1.1× slower |
| insert 4 KiB, start of 1 MiB | 4.4 | 4.9 | 1.1× slower | 1.7× slower |
| put 32 MiB | 96.0 | 108 | 1.1× slower | 1.2× slower |
| stream get 64 MiB | 43.0 | 49.2 | 1.1× slower | 15× faster |
| write at 4 KiB in 1 MiB | 4.1 | 4.7 | 1.2× slower | 1.6× slower |
| get 64 MiB | 44.5 | 52.2 | 1.2× slower | 17× faster |
| truncate 4 KiB, end of 1 MiB | 4.7 | 5.6 | 1.2× slower | 2.1× slower |
| get 32 MiB | 21.9 | 26.6 | 1.2× slower | 12× faster |
| delete 4 KiB, middle of 1 MiB | 5.1 | 6.3 | 1.2× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 4.7 | 6.1 | 1.3× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 5.1 | 7.0 | 1.4× slower | 1.4× slower |
| overwrite 4 KiB | 1.3 | 2.0 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.0 | 8.5 | 1.7× slower | 2.4× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.3 | 5.5 | 1.7× slower | 2.2× slower |
| overwrite 1 MiB | 4.5 | 7.7 | 1.7× slower | 1.9× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.3 | 7.6 | 1.8× slower | 2.8× slower |
| put 1 MiB | 3.3 | 6.0 | 1.8× slower | 1.7× slower |
| put 4 KiB | 1.0 | 2.1 | 2.1× slower | 2.0× slower |
| patch 16 × 4 KiB in 1 MiB | 4.4 | 18.9 | 4.3× slower | 1.5× slower |
