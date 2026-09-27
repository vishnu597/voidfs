# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260927T212108Z` |
| When | 2026-09-27T21:21:08Z |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | c434fcb (with uncommitted changes) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| probe | step 3, finding 3: the first rows of the full run, then fan-out get; shard cache as shipped (moka TinyLFU) |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | voidfs-server release build, s3: store in that bucket, 512 MiB shard cache |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 11 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| range 64 KiB of 64 MiB | – | 0.23 | – | 34× faster |
| get 4 KiB | – | 0.23 | – | 23× faster |
| move dir 200 × 64 KiB | – | 101 | – | 18× faster |
| get 64 MiB | – | 49.2 | – | 17× faster |
| stream get 256 MiB | – | 182 | – | 16× faster |
| stream get 64 MiB | – | 43.5 | – | 15× faster |
| append 4 KiB to 64 MiB | – | 91.4 | – | 15× faster |
| head | – | 0.25 | – | 13× faster |
| truncate 4 KiB, end of 64 MiB | – | 90.6 | – | 13× faster |
| get 32 MiB | – | 75.2 | – | 12× faster |
| fanout get 1000 × 4 KiB, 32 at once | – | 12.8 | – | 5.3× faster |
