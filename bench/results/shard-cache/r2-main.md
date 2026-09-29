# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T002152Z` |
| When | 2026-09-29T00:21:52Z |
| voidfs | a voidfs server (endpoint not recorded) |
| bucket | Cloudflare R2 (location not recorded), reached from this Mac over a home internet connection |
| client | Darwin 27.0.0, 15 vCPUs |
| voidfs server | main at 634267e, release build: moka's default admission (TinyLFU); on the client host, over that same bucket, 64 MiB shard cache (--cache-mib 64, so that it fills) |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 23 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Operations | scaled by 0.25 from the harness defaults |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| get 4 KiB | – | 0.50 | – | 23× faster |
| move dir 200 × 64 KiB | – | 367 | – | 18× faster |
| head | – | 0.42 | – | 13× faster |
| list 200 keys | – | 1.8 | – | 9.1× faster |
| fanout get 200 × 256 KiB, 32 at once | – | 116 | – | 6.2× faster |
| fanout get 1000 × 4 KiB, 32 at once | – | 75.7 | – | 5.3× faster |
| get 1 MiB | – | 0.99 | – | 4.5× faster |
| fanout get 1000 × 4 KiB, 64 at once | – | 75.2 | – | 2.6× faster |
| append 4 KiB to 1 MiB | – | 731 | – | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | – | 980 | – | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | – | 956 | – | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | – | 1,030 | – | 1.5× slower |
| delete 4 KiB, start of 1 MiB | – | 944 | – | 1.5× slower |
| write at 4 KiB in 1 MiB | – | 955 | – | 1.6× slower |
| insert 4 KiB, start of 1 MiB | – | 943 | – | 1.7× slower |
| put 1 MiB | – | 750 | – | 1.7× slower |
| overwrite 1 MiB | – | 745 | – | 1.9× slower |
| put 4 KiB | – | 593 | – | 2.0× slower |
| truncate 4 KiB, end of 1 MiB | – | 878 | – | 2.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | – | 680 | – | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | – | 597 | – | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | – | 697 | – | 2.8× slower |
| overwrite 4 KiB | – | 625 | – | 3.1× slower |
