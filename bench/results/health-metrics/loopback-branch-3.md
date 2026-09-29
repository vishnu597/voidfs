# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T142745Z` |
| When | 2026-09-29T14:27:45Z |
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

voidfs is faster in **30 of 49** scenarios and slower in the other **19**. Geometric mean speed-up over the bare bucket: **3.1×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 395 | 0.95 | 416× faster | 18× faster |
| rename 64 MiB | 113 | 0.72 | 157× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 198 | 5.7 | 35× faster | 13× faster |
| list 200 keys | 35.7 | 1.1 | 32× faster | 9.1× faster |
| insert 4 KiB, start of 64 MiB | 189 | 6.0 | 31× faster | 6.8× faster |
| append 4 KiB to 64 MiB | 192 | 7.2 | 27× faster | 15× faster |
| write at 4 KiB in 64 MiB | 176 | 7.8 | 23× faster | 6.2× faster |
| delete 4 KiB, start of 64 MiB | 188 | 9.3 | 20× faster | 6.9× faster |
| truncate 4 KiB, end of 32 MiB | 102 | 5.7 | 18× faster | 5.9× faster |
| append 4 KiB to 32 MiB | 114 | 6.7 | 17× faster | 8.0× faster |
| insert 4 KiB, middle of 64 MiB | 191 | 11.8 | 16× faster | 11× faster |
| insert 4 KiB, start of 32 MiB | 118 | 8.3 | 14× faster | 3.5× faster |
| delete 4 KiB, start of 32 MiB | 112 | 9.4 | 12× faster | 4.6× faster |
| delete 4 KiB, middle of 64 MiB | 197 | 19.0 | 10× faster | 11× faster |
| delete 4 KiB, middle of 32 MiB | 107 | 10.4 | 10× faster | 4.8× faster |
| insert 4 KiB, middle of 32 MiB | 101 | 10.9 | 9.3× faster | 3.8× faster |
| write at 4 KiB in 32 MiB | 102 | 11.6 | 8.9× faster | 5.1× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.5 | 0.43 | 5.7× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.8 | 0.86 | 5.5× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.24 | 4.6× faster | 34× faster |
| head | 0.73 | 0.18 | 4.0× faster | 13× faster |
| get 4 KiB | 0.85 | 0.22 | 3.8× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.7 | 0.77 | 3.5× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 202 | 91.6 | 2.2× faster | 2.1× faster |
| truncate 4 KiB, end of 1 MiB | 5.9 | 4.4 | 1.4× faster | 2.1× slower |
| patch 16 × 4 KiB in 32 MiB | 103 | 81.0 | 1.3× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,172 | 981 | 1.2× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 286 | 269 | 1.1× faster | 2.4× slower |
| get 1 MiB | 1.0 | 0.99 | 1.0× faster | 4.5× faster |
| write at 4 KiB in 1 MiB | 4.7 | 4.5 | 1.0× faster | 1.6× slower |
| stream get 256 MiB | 193 | 212 | 1.1× slower | 16× faster |
| get 32 MiB | 23.9 | 26.5 | 1.1× slower | 12× faster |
| stream get 64 MiB | 46.0 | 52.5 | 1.1× slower | 15× faster |
| delete 4 KiB, middle of 1 MiB | 4.3 | 5.0 | 1.2× slower | 1.4× slower |
| insert 4 KiB, start of 1 MiB | 4.3 | 5.2 | 1.2× slower | 1.7× slower |
| get 64 MiB | 48.2 | 58.8 | 1.2× slower | 17× faster |
| delete 4 KiB, start of 1 MiB | 4.1 | 5.0 | 1.2× slower | 1.5× slower |
| append 4 KiB to 1 MiB | 3.9 | 5.3 | 1.3× slower | 1.0× faster |
| insert 4 KiB, middle of 1 MiB | 4.1 | 5.7 | 1.4× slower | 1.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.6 | 6.9 | 1.5× slower | 2.8× slower |
| overwrite 4 KiB | 1.0 | 1.6 | 1.5× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.0 | 4.9 | 1.7× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 5.1 | 8.9 | 1.8× slower | 2.4× slower |
| put 4 KiB | 0.81 | 1.5 | 1.8× slower | 2.0× slower |
| put 64 MiB | 154 | 305 | 2.0× slower | 1.1× slower |
| put 32 MiB | 75.4 | 156 | 2.1× slower | 1.2× slower |
| overwrite 1 MiB | 3.0 | 7.6 | 2.5× slower | 1.9× slower |
| put 1 MiB | 3.0 | 7.8 | 2.6× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.2 | 15.4 | 3.6× slower | 1.5× slower |
