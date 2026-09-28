# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T013512Z` |
| When | 2026-09-28T01:35:12Z |
| Bare bucket | http://127.0.0.1:7071, bucket bench, prefix voidfs-bench/bare/ |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | a9ea745 (with uncommitted changes) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 49 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

voidfs is faster in **25 of 49** scenarios and slower in the other **24**. Geometric mean speed-up over the bare bucket: **1.0×** (SpaceFS's, same rows: 2.8×).

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| head | 13.7 | 0.23 | 60× faster | 13× faster |
| range 64 KiB of 64 MiB | 12.6 | 0.29 | 44× faster | 34× faster |
| get 4 KiB | 13.9 | 0.33 | 43× faster | 23× faster |
| list 200 keys | 39.2 | 1.1 | 37× faster | 9.1× faster |
| get 1 MiB | 14.1 | 1.4 | 10× faster | 4.5× faster |
| move dir 200 × 64 KiB | 465 | 105 | 4.4× faster | 18× faster |
| insert 4 KiB, start of 64 MiB | 298 | 91.4 | 3.3× faster | 6.8× faster |
| write at 4 KiB in 64 MiB | 304 | 98.2 | 3.1× faster | 6.2× faster |
| truncate 4 KiB, end of 64 MiB | 299 | 97.9 | 3.1× faster | 13× faster |
| append 4 KiB to 64 MiB | 296 | 97.7 | 3.0× faster | 15× faster |
| delete 4 KiB, start of 64 MiB | 278 | 94.6 | 2.9× faster | 6.9× faster |
| insert 4 KiB, middle of 64 MiB | 279 | 95.9 | 2.9× faster | 11× faster |
| stream get 256 MiB | 512 | 183 | 2.8× faster | 16× faster |
| delete 4 KiB, middle of 64 MiB | 276 | 98.9 | 2.8× faster | 11× faster |
| stream get 64 MiB | 122 | 46.5 | 2.6× faster | 15× faster |
| get 64 MiB | 115 | 49.5 | 2.3× faster | 17× faster |
| write at 4 KiB in 32 MiB | 173 | 99.5 | 1.7× faster | 5.1× faster |
| delete 4 KiB, middle of 32 MiB | 164 | 96.2 | 1.7× faster | 4.8× faster |
| insert 4 KiB, start of 32 MiB | 151 | 93.4 | 1.6× faster | 3.5× faster |
| delete 4 KiB, start of 32 MiB | 144 | 93.7 | 1.5× faster | 4.6× faster |
| truncate 4 KiB, end of 32 MiB | 149 | 98.3 | 1.5× faster | 5.9× faster |
| patch 16 × 4 KiB in 64 MiB | 285 | 193 | 1.5× faster | 2.1× faster |
| append 4 KiB to 32 MiB | 150 | 102 | 1.5× faster | 8.0× faster |
| insert 4 KiB, middle of 32 MiB | 141 | 101 | 1.4× faster | 3.8× faster |
| rename 64 MiB | 122 | 106 | 1.2× faster | 7.9× faster |
| fanout get 1000 × 4 KiB, 32 at once | 12.7 | 13.0 | 1.0× slower | 5.3× faster |
| fanout get 1000 × 4 KiB, 64 at once | 12.0 | 12.3 | 1.0× slower | 2.6× faster |
| fanout get 200 × 256 KiB, 32 at once | 14.4 | 15.2 | 1.1× slower | 6.2× faster |
| patch 16 × 4 KiB in 32 MiB | 137 | 153 | 1.1× slower | 1.4× faster |
| get 32 MiB | 62.2 | 76.2 | 1.2× slower | 12× faster |
| multipart put 256 MiB × 16 MiB | 917 | 1,256 | 1.4× slower | 1.8× slower |
| multipart put 64 MiB × 8 MiB | 241 | 409 | 1.7× slower | 2.4× slower |
| put 32 MiB | 89.7 | 194 | 2.2× slower | 1.2× slower |
| put 64 MiB | 169 | 384 | 2.3× slower | 1.1× slower |
| write at 4 KiB in 1 MiB | 27.4 | 91.3 | 3.3× slower | 1.6× slower |
| delete 4 KiB, middle of 1 MiB | 27.7 | 92.2 | 3.3× slower | 1.4× slower |
| truncate 4 KiB, end of 1 MiB | 26.8 | 89.9 | 3.4× slower | 2.1× slower |
| patch 16 × 4 KiB in 1 MiB | 27.1 | 91.7 | 3.4× slower | 1.5× slower |
| append 4 KiB to 1 MiB | 26.9 | 95.6 | 3.6× slower | 1.0× faster |
| delete 4 KiB, start of 1 MiB | 26.7 | 95.2 | 3.6× slower | 1.5× slower |
| insert 4 KiB, start of 1 MiB | 26.5 | 95.9 | 3.6× slower | 1.7× slower |
| insert 4 KiB, middle of 1 MiB | 26.3 | 95.7 | 3.6× slower | 1.4× slower |
| put 1 MiB | 13.7 | 85.6 | 6.2× slower | 1.7× slower |
| overwrite 1 MiB | 13.6 | 85.3 | 6.3× slower | 1.9× slower |
| put 4 KiB | 12.4 | 102 | 8.3× slower | 2.0× slower |
| overwrite 4 KiB | 11.8 | 102 | 8.6× slower | 3.1× slower |
| fanout put 200 × 256 KiB, 32 at once | 12.8 | 367 | 29× slower | 2.8× slower |
| fanout put 1000 × 4 KiB, 32 at once | 12.2 | 411 | 34× slower | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 12.2 | 815 | 67× slower | 2.4× slower |
