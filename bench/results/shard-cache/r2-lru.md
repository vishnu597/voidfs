# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260929T003008Z` |
| When | 2026-09-29T00:30:08Z |
| voidfs | a voidfs server (endpoint not recorded) |
| bucket | Cloudflare R2 (location not recorded), reached from this Mac over a home internet connection |
| client | Darwin 27.0.0, 15 vCPUs |
| voidfs server | main at 32d8216, release build: least recently used first, cached copies; on the client host, over that same bucket, 64 MiB shard cache (--cache-mib 64, so that it fills) |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 23 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Operations | scaled by 0.25 from the harness defaults |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| get 4 KiB | – | 0.45 | – | 23× faster |
| move dir 200 × 64 KiB | – | 286 | – | 18× faster |
| head | – | 0.27 | – | 13× faster |
| list 200 keys | – | 1.7 | – | 9.1× faster |
| fanout get 200 × 256 KiB, 32 at once | – | 2.2 | – | 6.2× faster |
| fanout get 1000 × 4 KiB, 32 at once | – | 0.61 | – | 5.3× faster |
| get 1 MiB | – | 1.6 | – | 4.5× faster |
| fanout get 1000 × 4 KiB, 64 at once | – | 1.8 | – | 2.6× faster |
| append 4 KiB to 1 MiB | – | 761 | – | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | – | 738 | – | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | – | 758 | – | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | – | 767 | – | 1.5× slower |
| delete 4 KiB, start of 1 MiB | – | 743 | – | 1.5× slower |
| write at 4 KiB in 1 MiB | – | 759 | – | 1.6× slower |
| insert 4 KiB, start of 1 MiB | – | 767 | – | 1.7× slower |
| put 1 MiB | – | 758 | – | 1.7× slower |
| overwrite 1 MiB | – | 708 | – | 1.9× slower |
| put 4 KiB | – | 629 | – | 2.0× slower |
| truncate 4 KiB, end of 1 MiB | – | 799 | – | 2.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | – | 613 | – | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | – | 588 | – | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | – | 706 | – | 2.8× slower |
| overwrite 4 KiB | – | 635 | – | 3.1× slower |
