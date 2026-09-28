# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T193550Z` |
| When | 2026-09-28T19:35:50Z |
| Bare bucket | http://127.0.0.1:7070, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | ae3a466 (with uncommitted changes) |
| distance to the bucket | none (loopback) |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **27 of 49** scenarios and slower in the other **22**. Geometric mean speed-up over the bare bucket: **2.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| move dir 200 × 64 KiB | 366 | 2.6 | 143× faster | 18× faster |
| rename 64 MiB | 103 | 2.5 | 42× faster | 7.9× faster |
| list 200 keys | 36.1 | 1.1 | 34× faster | 9.1× faster |
| insert 4 KiB, start of 64 MiB | 208 | 10.6 | 20× faster | 6.8× faster |
| delete 4 KiB, start of 64 MiB | 223 | 13.8 | 16× faster | 6.9× faster |
| truncate 4 KiB, end of 64 MiB | 192 | 12.2 | 16× faster | 13× faster |
| write at 4 KiB in 64 MiB | 220 | 14.4 | 15× faster | 6.2× faster |
| append 4 KiB to 64 MiB | 183 | 13.8 | 13× faster | 15× faster |
| truncate 4 KiB, end of 32 MiB | 104 | 8.7 | 12× faster | 5.9× faster |
| insert 4 KiB, middle of 64 MiB | 216 | 18.9 | 11× faster | 11× faster |
| delete 4 KiB, middle of 64 MiB | 197 | 18.4 | 11× faster | 11× faster |
| append 4 KiB to 32 MiB | 106 | 11.2 | 9.5× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 108 | 13.9 | 7.7× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 96.7 | 12.6 | 7.7× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 105 | 14.0 | 7.5× faster | 4.8× faster |
| delete 4 KiB, start of 32 MiB | 102 | 14.9 | 6.8× faster | 4.6× faster |
| insert 4 KiB, middle of 32 MiB | 102 | 17.6 | 5.8× faster | 3.8× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 4.6× faster | 34× faster |
| head | 0.73 | 0.19 | 3.9× faster | 13× faster |
| get 4 KiB | 0.82 | 0.22 | 3.8× faster | 23× faster |
| patch 16 × 4 KiB in 64 MiB | 228 | 130 | 1.7× faster | 2.1× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.5 | 3.3 | 1.4× faster | 2.6× faster |
| multipart put 256 MiB × 16 MiB | 1,373 | 1,123 | 1.2× faster | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 281 | 244 | 1.2× faster | 2.4× slower |
| fanout get 1000 × 4 KiB, 32 at once | 2.6 | 2.3 | 1.1× faster | 5.3× faster |
| get 1 MiB | 1.0 | 1.0 | 1.0× faster | 4.5× faster |
| patch 16 × 4 KiB in 32 MiB | 99.6 | 97.7 | 1.0× faster | 1.4× faster |
| stream get 256 MiB | 193 | 206 | 1.1× slower | 16× faster |
| stream get 64 MiB | 46.6 | 52.9 | 1.1× slower | 15× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.5 | 2.8 | 1.1× slower | 6.2× faster |
| get 64 MiB | 48.2 | 55.1 | 1.1× slower | 17× faster |
| get 32 MiB | 27.8 | 40.6 | 1.5× slower | 12× faster |
| delete 4 KiB, start of 1 MiB | 4.6 | 6.8 | 1.5× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 4.2 | 6.5 | 1.5× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 4.9 | 8.3 | 1.7× slower | 1.4× slower |
| write at 4 KiB in 1 MiB | 4.4 | 7.9 | 1.8× slower | 1.6× slower |
| truncate 4 KiB, end of 1 MiB | 4.3 | 7.9 | 1.8× slower | 2.1× slower |
| put 64 MiB | 182 | 346 | 1.9× slower | 1.1× slower |
| put 1 MiB | 3.7 | 7.4 | 2.0× slower | 1.7× slower |
| put 32 MiB | 87.0 | 176 | 2.0× slower | 1.2× slower |
| delete 4 KiB, middle of 1 MiB | 5.3 | 10.7 | 2.0× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 4.5 | 10.3 | 2.3× slower | 1.0× faster |
| overwrite 1 MiB | 4.1 | 9.5 | 2.3× slower | 1.9× slower |
| overwrite 4 KiB | 1.1 | 2.9 | 2.7× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.5 | 22.4 | 3.5× slower | 2.4× slower |
| put 4 KiB | 0.97 | 3.6 | 3.7× slower | 2.0× slower |
| fanout put 1000 × 4 KiB, 32 at once | 2.8 | 11.2 | 4.0× slower | 2.2× slower |
| patch 16 × 4 KiB in 1 MiB | 4.5 | 22.3 | 5.0× slower | 1.5× slower |
| fanout put 200 × 256 KiB, 32 at once | 4.1 | 23.6 | 5.8× slower | 2.8× slower |
