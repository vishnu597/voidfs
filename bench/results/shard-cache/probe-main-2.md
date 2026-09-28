# voidfs against a bare bucket

> **Not comparable with SpaceFS's figures.** This run was not made in SpaceFS's setup (a client in GCP us-east4, a bucket in AWS S3 us-east-1). The SpaceFS column is their published result, shown for orientation only; compare voidfs with the bare bucket beside it.

| | |
|---|---|
| Run | `20260928T203854Z` |
| When | 2026-09-28T20:38:54Z |
| voidfs | http://127.0.0.1:9000 (drives vfbench-*) |
| bucket | versitygw 1.8.0 on local disk, 127.0.0.1:7070 |
| commit | 634267e (main) |
| distance to the bucket | emulated with `voidfs-bench delay --one-way-ms 4` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host |
| probe | step 3, item 2: the probe sequence of bench/results/probes/cache-*.json (the first rows of the full run, then fan-out get) |
| setup | one machine over loopback: harness, voidfs-server and the S3 server (15 CPUs, Darwin arm64) |
| voidfs server | release build of main 634267e (cargo build --release -p voidfs-server), s3: store in that bucket, 512 MiB shard cache, moka's default admission (TinyLFU) |
| Harness host | macos aarch64, 15 CPUs |
| Shape | 11 scenarios, 2 rounds, 3 warm-up operations each, concurrency 8 unless the scenario names its own, at most 64 requests in flight |
| Figure | Median of each round's p50, in milliseconds |
| SpaceFS column | Their published run 20260920T055107Z (build s3sdk@fff9779), [https://docs.spacefs.com/benchmarks/](https://docs.spacefs.com/benchmarks/) |

| Scenario | Bucket alone (ms) | voidfs (ms) | Result | SpaceFS result |
|---|--:|--:|---|---|
| range 64 KiB of 64 MiB | – | 0.24 | – | 34× faster |
| get 4 KiB | – | 0.27 | – | 23× faster |
| move dir 200 × 64 KiB | – | 26.9 | – | 18× faster |
| get 64 MiB | – | 51.8 | – | 17× faster |
| stream get 256 MiB | – | 187 | – | 16× faster |
| stream get 64 MiB | – | 47.7 | – | 15× faster |
| append 4 KiB to 64 MiB | – | 36.3 | – | 15× faster |
| head | – | 0.22 | – | 13× faster |
| truncate 4 KiB, end of 64 MiB | – | 43.8 | – | 13× faster |
| get 32 MiB | – | 54.3 | – | 12× faster |
| fanout get 1000 × 4 KiB, 32 at once | – | 13.1 | – | 5.3× faster |
