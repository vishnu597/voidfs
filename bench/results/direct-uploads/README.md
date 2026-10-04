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

## On R2 ([raw](r2-32m.txt))

The same 32 MiB, the pool in Cloudflare R2 (location hint ENAM) over this Mac's home internet. The
client still reaches the server through the relay (12 ms, 20 MB/s up); the server, and the client's
direct uploads, reach R2 over the internet.

| | Median time | Median sent | Each of 6 |
|---|--:|--:|---|
| Put | 2,565 ms | 33.5 MB | 2,276–2,651 ms |
| Direct upload | 963 ms | 2.6 MB | 804–1,221 ms |

A direct upload waits on R2 for a shard's PUT and the commit's HEAD of it, about 0.5 s more than
locally; the put still sends the whole file over the client's link.

## What the buckets enforce ([AWS S3](aws-check.txt), [R2](r2-check.txt))

On 4 October, a server on a fresh pool in each bucket (AWS S3 in us-east-1, and R2), its start-up
check of presigned PUTs, the seven direct-upload conformance cases against it, and `probe`; the pool
was purged after each.

| Bucket | A wrong `x-amz-checksum-sha256` | Without the signed checksum | `If-None-Match` on an existing object | Conformance |
|---|---|---|---|---|
| AWS S3 | refused, 400 | refused, 403 | refused, 412 | 6 passed, 1 skipped (the one for servers without the extension) |
| Cloudflare R2 | refused, 400 | refused, 403 | refused, 412 | 6 passed, 1 skipped |
| versitygw 1.8.0 | refused, 400 | refused, 403 | refused, 412 | 6 passed, 1 skipped |
| Backblaze B2 (through Space's URLs, 3 October) | not checked | an unsigned one refused | 501 | — |

In each, the case that PUTs a shard's URL other bytes was refused by the bucket itself (400).

## Reproduce

```bash
cargo build --release -p voidfs-server -p voidfs-bench
cargo build --release -p voidfs-sdk --example reupload
bench/scripts/direct-reupload.sh
BENCH_FIRST_MULTIPART=1 BENCH_SIZE_MIB=128 BENCH_ROUNDS=2 bench/scripts/direct-reupload.sh
BENCH_R2=1 bench/scripts/direct-reupload.sh    # then: voidfs-bench purge --prefix voidfs-bench/ --yes
```
