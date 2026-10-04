# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261003T234005Z` |
| When | 2026-10-03T23:40:05Z |
| Bare bucket | the bucket reached directly (endpoint and name not recorded) |
| client | this Mac, home internet |
| space | 0.2.343, protocol 1 |
| target | Space s3sdk endpoint (hosted), drive as bucket, plain S3 calls |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 7 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Operations | scaled by 0.25 from the harness defaults |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| overwrite 1 MiB | 398 | – | – | 1.9× slower |
| put 4 KiB | 115 | – | – | 2.0× slower |
| truncate 4 KiB, end of 1 MiB | 1,031 | – | – | 2.1× slower |
| fanout put 1000 × 4 KiB, 32 at once | 239 | – | – | 2.2× slower |
| fanout put 1000 × 4 KiB, 64 at once | 401 | – | – | 2.4× slower |
| fanout put 200 × 256 KiB, 32 at once | 290 | – | – | 2.8× slower |
| overwrite 4 KiB | 103 | – | – | 3.1× slower |
