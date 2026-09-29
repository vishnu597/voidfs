# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T144045Z` |
| When | 2026-09-29T14:40:45Z |
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
| move dir 200 × 64 KiB | 401 | 1.1 | 362× faster | 18× faster |
| rename 64 MiB | 114 | 0.73 | 157× faster | 7.9× faster |
| truncate 4 KiB, end of 64 MiB | 197 | 4.0 | 49× faster | 13× faster |
| append 4 KiB to 64 MiB | 192 | 4.3 | 45× faster | 15× faster |
| list 200 keys | 35.9 | 1.1 | 32× faster | 9.1× faster |
| insert 4 KiB, start of 64 MiB | 185 | 6.7 | 28× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 254 | 11.1 | 23× faster | 6.2× faster |
| insert 4 KiB, middle of 64 MiB | 218 | 9.5 | 23× faster | 11× faster |
| truncate 4 KiB, end of 32 MiB | 95.9 | 4.6 | 21× faster | 5.9× faster |
| delete 4 KiB, middle of 64 MiB | 220 | 12.6 | 17× faster | 11× faster |
| delete 4 KiB, start of 64 MiB | 172 | 10.7 | 16× faster | 6.9× faster |
| delete 4 KiB, start of 32 MiB | 116 | 7.5 | 15× faster | 4.6× faster |
| append 4 KiB to 32 MiB | 92.2 | 7.0 | 13× faster | 8.0× faster |
| insert 4 KiB, start of 32 MiB | 111 | 9.0 | 12× faster | 3.5× faster |
| write at 4 KiB in 32 MiB | 106 | 9.8 | 11× faster | 5.1× faster |
| insert 4 KiB, middle of 32 MiB | 98.6 | 10.0 | 9.9× faster | 3.8× faster |
| delete 4 KiB, middle of 32 MiB | 104 | 12.6 | 8.2× faster | 4.8× faster |
| fanout get 1000 × 4 KiB, 32 at once | 2.7 | 0.49 | 5.5× faster | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 4.6 | 0.91 | 5.0× faster | 2.6× faster |
| range 64 KiB of 64 MiB | 1.1 | 0.23 | 5.0× faster | 34× faster |
| head | 0.76 | 0.19 | 4.0× faster | 13× faster |
| get 4 KiB | 0.91 | 0.24 | 3.8× faster | 23× faster |
| fanout get 200 × 256 KiB, 32 at once | 2.6 | 0.73 | 3.6× faster | 6.2× faster |
| patch 16 × 4 KiB in 64 MiB | 190 | 107 | 1.8× faster | 2.1× faster |
| patch 16 × 4 KiB in 32 MiB | 95.2 | 77.5 | 1.2× faster | 1.4× faster |
| multipart put 256 MiB × 16 MiB | 1,183 | 1,009 | 1.2× faster | 1.8× slower |
| get 1 MiB | 1.0 | 0.89 | 1.1× faster | 4.5× faster |
| multipart put 64 MiB × 8 MiB | 256 | 254 | 1.0× faster | 2.4× slower |
| get 32 MiB | 25.9 | 27.9 | 1.1× slower | 12× faster |
| stream get 256 MiB | 183 | 204 | 1.1× slower | 16× faster |
| stream get 64 MiB | 45.4 | 51.5 | 1.1× slower | 15× faster |
| delete 4 KiB, middle of 1 MiB | 4.1 | 4.8 | 1.2× slower | 1.4× slower |
| get 64 MiB | 47.6 | 57.0 | 1.2× slower | 17× faster |
| insert 4 KiB, middle of 1 MiB | 4.4 | 5.2 | 1.2× slower | 1.4× slower |
| append 4 KiB to 1 MiB | 4.2 | 5.2 | 1.2× slower | 1.0× faster |
| delete 4 KiB, start of 1 MiB | 4.1 | 5.2 | 1.3× slower | 1.5× slower |
| truncate 4 KiB, end of 1 MiB | 4.0 | 5.3 | 1.3× slower | 2.1× slower |
| write at 4 KiB in 1 MiB | 4.1 | 5.4 | 1.3× slower | 1.6× slower |
| insert 4 KiB, start of 1 MiB | 4.2 | 5.5 | 1.3× slower | 1.7× slower |
| fanout put 1000 × 4 KiB, 64 at once | 6.1 | 8.8 | 1.4× slower | 2.4× slower |
| overwrite 4 KiB | 0.89 | 1.4 | 1.6× slower | 3.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 3.0 | 4.9 | 1.6× slower | 2.2× slower |
| put 4 KiB | 0.81 | 1.4 | 1.8× slower | 2.0× slower |
| put 64 MiB | 159 | 299 | 1.9× slower | 1.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 3.4 | 6.5 | 1.9× slower | 2.8× slower |
| put 32 MiB | 74.9 | 151 | 2.0× slower | 1.2× slower |
| overwrite 1 MiB | 3.2 | 7.8 | 2.4× slower | 1.9× slower |
| put 1 MiB | 3.1 | 7.8 | 2.5× slower | 1.7× slower |
| patch 16 × 4 KiB in 1 MiB | 4.0 | 17.5 | 4.4× slower | 1.5× slower |
