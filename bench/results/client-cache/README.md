# The client core's block cache, measured (step 4, item 3)

*2 October 2026, this Mac (Apple M5 Pro, 15 CPUs, macOS 27), branch `client-cache` on `e0b7808`.
Observed unless marked.*

`crates/voidfs-client`'s cache reads through the API in 8 MiB blocks, keeps them in memory (192
MiB) and on disk (20 GiB), and reads ahead for a reader that goes forward. This measures it the way
the Mac head-to-head ([mac-head-to-head](../mac-head-to-head/README.md)) measured the two apps:
300 random 4 KiB reads of a 1 GiB file of random bytes, and the file read once, 1 MiB at a time,
each read waiting for the last (as Finder's copy reads).

[`bench/scripts/client-randread.sh`](../../scripts/client-randread.sh) runs versitygw on local disk
as the bucket, `voidfs-server` (release) with its pool there, and the
[`randread`](../../../crates/voidfs-client/examples/randread.rs) example against the server over
loopback. Before every pass but `warm`, the server's shard and page caches are dropped (SIGUSR1), so
that its reads reach the bucket. The passes:
- **direct:** each read is one ranged GET through the SDK, as the spike's mount did.
- **cold:** through a new, empty cache.
- **warm:** the cold pass's 300 reads again, with that cache (memory, then disk).

Three setups, two runs each (seeds 1 and 2):
- **loopback:** the bucket on this Mac's disk;
- **12 ms, capped:** `voidfs-bench delay --one-way-ms 6 --bandwidth s3` between `voidfs-server`
  and the bucket, the benchmark's emulation of S3 12 ms away (95 MB/s down per connection).

- **R2:** the pool in the user's Cloudflare R2 bucket (location hint ENAM), over this Mac's home
  internet, as in the head-to-head; a fresh pool for each run, purged afterwards (897 objects).

The head-to-head ran against R2 on 1 October. The link was slower on 2 October: a direct 4 KiB
read, which makes the server fetch a whole shard of about 2.4 MB, took 262–296 ms at p50 against
the spike's 140–164 ms then. Compare within a row group, and across days with that in mind.

## Random 4 KiB reads (ms)

| | p50 | p90 | p99 | Blocks fetched |
|---|--:|--:|--:|--:|
| loopback, direct | 1.44, 1.51 | 2.54, 2.44 | 4.73, 4.47 | |
| loopback, cold | 0.064, 0.058 | 5.75, 5.42 | 8.62, 7.25 | 114, 115 |
| loopback, warm | 0.026, 0.029 | 0.031, 0.034 | 0.037, 0.043 | 0 |
| 12 ms capped, direct | 41.5, 41.5 | 61.6, 61.1 | 94.7, 87.1 | |
| 12 ms capped, cold | 0.105, 0.099 | 69.8, 65.3 | 94.3, 104.9 | 114, 115 |
| 12 ms capped, warm | 0.028, 0.041 | 0.038, 0.050 | 0.060, 0.066 | 0 |
| R2, direct | 296, 262 | 538, 441 | 873, 628 | |
| R2, cold | 0.146, 0.150 | 585, 538 | 1,009, 886 | 114, 115 |
| R2, warm | 0.039, 0.045 | 0.052, 0.062 | 0.112, 0.083 | 0 |
| *Head-to-head, Space (R2)* | *1.2* | *270–299* | *366–582* | |
| *Head-to-head, voidfs's spike (R2)* | *140–164* | *246–327* | *479–702* | |

- **The p50 is now a hit, as Space's is.** 300 random reads touch about 115 of the file's 128
  blocks, so most reads find their block already fetched: 0.06–0.15 ms cold, R2 included, against
  Space's 1.2 ms and the spike's 140–164 ms. A miss fetches a whole 8 MiB block, as Space's misses
  do: p90 65–70 ms through the relay, and 538–585 ms on R2 on a day when a direct read's p90 was
  441–538 ms (Space's was 270–299 ms on 1 October). A miss costs more than a direct read; the
  reads after it are hits.
- **Warm, every read is a hit:** 52–53 from memory and 247–248 from disk, at 0.03–0.04 ms.
- The 912–920 MiB fetched for 1.2 MB of reads is the cost of 8 MiB blocks, the same as Space's
  (inferred from its block size).

## Sequential, 1 MiB at a time (MB/s)

| | direct | cold, with read-ahead |
|---|--:|--:|
| loopback | 1,121, 1,132 | 5,219, 5,542 |
| 12 ms capped | 56.5, 55.4 | 758, 763 |
| R2 | 8.7, 10.5 | 39.8, 37.2 |
| *Head-to-head, Finder's copy of the 1 GiB file (R2)* | *spike: 16–19 (57–67 s)* | *Space: 36–38 (28–30 s)* |

- **One read at a time no longer means one fetch at a time.** Through the relay, reading directly
  runs at 56 MB/s, each read waiting for its shard. With read-ahead, the reader fetches up to 64
  MiB ahead, 8 blocks at once, and runs at 760 MB/s: 13.5 times as fast, up to what the relay
  allows several connections at once.
- **On R2, 4 times as fast:** 37–40 MB/s with read-ahead against 9–11 MB/s directly, which is the
  rate Space's Finder copy (36–38 MB/s) and both apps' `dd` (32–42 MB/s) reached on 1 October.
- This is what Finder's copy lacked on voidfs (the head-to-head's 452 shards fetched one at a
  time). Through a mount, the kernel's own read-ahead comes on top (step 5).

## Files

| File | What |
|---|---|
| `loopback-1.txt`, `loopback-2.txt` | The example's output, seeds 1 and 2, the bucket on loopback |
| `rtt12-s3cap-1.txt`, `rtt12-s3cap-2.txt` | The same with the bucket 12 ms away and capped |
| `r2-1.txt`, `r2-2.txt` | The same against R2 (`BENCH_R2=1`) |
