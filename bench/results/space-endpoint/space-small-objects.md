# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20261003T205752Z` |
| When | 2026-10-03T20:57:52Z |
| Bare bucket | the bucket reached directly (endpoint and name not recorded) |
| client | this Mac, home internet |
| space | 0.2.343, protocol 1 |
| target | Space s3sdk endpoint (hosted), drive as bucket, plain S3 calls |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 16 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Operations | scaled by 0.25 from the harness defaults |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| get 4 KiB | 48.1 | – | – | 23× faster |
| move dir 200 × 64 KiB | 13,043 | – | – | 18× faster |
| head | 47.3 | – | – | 13× faster |
| list 200 keys | 106 | – | – | 9.1× faster |
| fanout get 200 × 256 KiB, 32 at once [^1] | – | – | – | 6.2× faster |
| fanout get 1000 × 4 KiB, 32 at once | 73.2 | – | – | 5.3× faster |
| get 1 MiB | 181 | – | – | 4.5× faster |
| fanout get 1000 × 4 KiB, 64 at once | 88.6 | – | – | 2.6× faster |
| append 4 KiB to 1 MiB | 802 | – | – | 1.0× faster |
| delete 4 KiB, middle of 1 MiB | 932 | – | – | 1.4× slower |
| insert 4 KiB, middle of 1 MiB | 850 | – | – | 1.4× slower |
| patch 16 × 4 KiB in 1 MiB | 818 | – | – | 1.5× slower |
| delete 4 KiB, start of 1 MiB | 666 | – | – | 1.5× slower |
| write at 4 KiB in 1 MiB | 1,272 | – | – | 1.6× slower |
| insert 4 KiB, start of 1 MiB | 1,183 | – | – | 1.7× slower |
| put 1 MiB | 343 | – | – | 1.7× slower |

[^1]: fanout get 200 × 256 KiB, 32 at once on bare: setup failed: uploading fan/o0076: dispatch failure: other: client error (SendRequest): http2 error: stream error received: unspecific protocol error detected (DispatchFailure(DispatchFailure { source: ConnectorError { kind: Other(None), source: hyper_ut
