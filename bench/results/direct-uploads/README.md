# Direct uploads against puts

*4 October 2026, step 4, item 5 ([plan](../../../docs/step-4-client.md#item-5-direct-uploads)).
[`bench/scripts/direct-reupload.sh`](../../scripts/direct-reupload.sh) and the SDK's
[`reupload`](../../../crates/voidfs-sdk/examples/reupload.rs) example, on one Mac.*

A file is uploaded once, then re-uploaded twelve times, each time with one region of 4 KiB
changed somewhere in its middle half: six times as an ordinary put, six as a direct upload
(protocol §4.11), alternating A B B A. "Sent" is every byte of the client's request bodies, counted
by the SDK's bandwidth limiter (`Bandwidth::taken`): the put's body, or the plan, the shards and the
commit.

The client is where a client away from the server would be: it reaches the server through a relay
of 12 ms each way and 20 MB/s up (`voidfs-bench delay`), and the bucket, versitygw, through another
such relay, which the server's own requests to the bucket cross too. So a put sends the whole file
over the client's link, and the server then sends the bucket only the new shard (it holds the rest);
a direct upload sends the new shard over the client's link to the bucket, and the server only checks
it.

## 32 MiB, written whole first ([raw](local-32m.txt))

| | Median time | Median sent | Each of 6 |
|---|--:|--:|---|
| Put | 2,023 ms | 33.5 MB | 1,776–2,029 ms, 33.5 MB |
| Direct upload | 404 ms | 2.6 MB | 329–490 ms, 1.3–4.4 MB |

The file is 13 shards; each plan held 12, and the direct upload sent the one around the change (1.3
to 4.4 MB, the shard's size) and two small JSON bodies. 5× faster and a thirteenth of the bytes, on a
link this slow; the gain shrinks as the link gets faster than the bucket, and grows with the file.

## 128 MiB, written first in 16 MiB parts ([raw](local-128m-multipart.txt))

A multipart upload is cut at its parts' boundaries (format §11), which a whole-file cut doesn't
share. A plan of the unchanged file, straight after its multipart upload, held **45 of its 56
shards**: 35 MB of 134 MB would be sent again, the shards around the seven boundaries. FastCDC finds
its own boundaries again a shard or two after each, so such a file still goes direct (the queue's
threshold is half). The re-uploads after it, the first of them a put of the whole file:

| | Median time | Median sent |
|---|--:|--:|
| Put | 6,983 ms | 134.2 MB |
| Direct upload | 649 ms | 4.2 MB |

## Reproduce

```bash
cargo build --release -p voidfs-server -p voidfs-bench
cargo build --release -p voidfs-sdk --example reupload
bench/scripts/direct-reupload.sh
BENCH_FIRST_MULTIPART=1 BENCH_SIZE_MIB=128 BENCH_ROUNDS=2 bench/scripts/direct-reupload.sh
```
