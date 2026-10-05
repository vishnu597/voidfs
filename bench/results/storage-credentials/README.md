# Reading from the bucket against reading through the server

*4 October 2026, step 4, item 6 ([plan](../../../docs/step-4-client.md#item-6-short-lived-storage-credentials)).
[`bench/scripts/bucket-read.sh`](../../scripts/bucket-read.sh) and the client's
[`bucketread`](../../../crates/voidfs-client/examples/bucketread.rs) example, in a Linux cloud
session against MinIO (its last release, built from source as CI builds it). The runs name `main`
at `36c784a`, with the client's pull request on top, not yet committed.*

A 64 MiB file of random bytes is read cold, from start to end, six times: three times through the
API (`ApiFetcher`) and three straight from the bucket with storage credentials (`BucketFetcher`,
protocol §5.5), alternating A B B A. Each read starts with a new, empty block cache and fetcher, and
the server's caches dropped (SIGUSR1). It reads the first 4 KiB, then the whole file 1 MiB at a
time, each read waiting for the one before (as Finder's copy reads), with the cache's read-ahead.
Then, with the same fetcher and cache, the first 4 KiB of another file of 8 MiB: the next file a
mount opens in a drive it reads already.

The client reaches the server through a relay of 12 ms each way (`voidfs-bench delay`), and the
bucket through another, which the server's own requests to the bucket cross too: through the server
the bytes cross both relays, from the bucket one.

## 40 MB/s down on each relay ([raw](local-64m.txt))

| | Whole 64 MiB | First 4 KiB | Next file's first 4 KiB |
|---|--:|--:|--:|
| Through the API | 3,205 ms (21 MB/s) | 488 ms | 466 ms |
| From the bucket | 1,916 ms (35 MB/s) | 426 ms | 259 ms |

Medians of three; each of the three is within 3% of the median for the whole file and within 7% for
the first 4 KiB. From the bucket the file streams at what the client's link carries, where
through the server each block waits for the server's own fetch from the bucket first. The first 4
KiB from the bucket includes asking the server for credentials and reading the drive's state from
the bucket (the pool's descriptor, the drive's descriptor, its checkpoints and log); the next file
only its shards. Each read from the bucket fetched exactly the two files' 75.5 MB, in 34 shards,
each checked against its hash, and the server sent no content bytes.

## No bandwidth limit, 12 ms each way ([raw](local-64m-uncapped.txt))

| | Whole 64 MiB | First 4 KiB | Next file's first 4 KiB |
|---|--:|--:|--:|
| Through the API | 323 ms | 94 ms | 94 ms |
| From the bucket | 398 ms | 220 ms | 82 ms |

When latency and not bandwidth bounds a read, the first read from the bucket pays for the drive's
state: two rounds of requests to the bucket (descriptors and the list of checkpoints at once, then
the newest checkpoint and the log after it at once), after the request for credentials. That is
once per drive and set of credentials, not per file: the next file starts as fast as through the
server. Reading the state one request after another, as the server's loader does, cost 255–300 ms
for the first 4 KiB in an earlier run; the client reads it in two rounds since.

## What the buckets do with minted credentials ([MinIO](minio-check.txt))

[`bench/scripts/credentials-check.sh`](../../scripts/credentials-check.sh): a server on a fresh pool
in the bucket, its check at start of what minted credentials reach, the five storage-credentials
conformance cases against it, and `probe`; the pool purged after.

| Bucket | Minted by | voidfs.json, its drive | The pool's root, another drive, a write | Conformance |
|---|---|---|---|---|
| MinIO (built from source) | its STS, AssumeRole with a session policy | read (200) | refused (403) | 4 passed, 1 skipped (the one for servers without them) |
| versitygw 1.8.0 | none: its S3 port answers AssumeRole 405 | — | — | 1 passed (the `501`), 4 skipped |
| AWS S3 ([after role fix](aws-check-after-role-fix.txt), `a108f12`) | STS, AssumeRole with a session policy | read/list (200) | all four refused (403 AccessDenied) | 4 passed, 1 skipped; capability check exits 0 |
| Cloudflare R2 ([live check](r2-check.txt), `d12cde1-dirty`) | Cloudflare's API, account-level API token and static parent key | read/list (200) | all four refused (403 AccessDenied) | 4 passed, 1 skipped; capability check exits 0 |

### AWS user-supplied check, 4 October

The report began at `2026-10-04T22:39:21Z`, build `d12cde1`, with a fresh pool at
`voidfs-bench/credentials-check-20261004T223921Z/` in the env file's bucket. STS minted credentials:
the server then made six signed S3 requests. Reading `voidfs.json` and listing the synthetic
drive both returned 403; listing the pool's root, listing/reading another drive and writing also
returned 403. The server correctly withheld credentials because required reads failed.

Conformance reported **1 passed, 0 failed, 4 skipped**: the pass exercises a server that does
not offer storage credentials, not a successful scoped-read session. Create-if-absent was
honored, four pool objects were purged, and the counts under `voidfs-bench/` were **0 before and
0 after**. This is a sanitized summary of the user's pasted output; no raw results file is
claimed or created for that run.

The code now reports safe S3 error codes alongside status, so the next check can distinguish
access policy denials from token/signature/region problems. The assumed role's S3 policy is the
first item to review, because the caller's admin/read-write grant does not supply role access.
See [the AWS diagnostic and role-policy guide](../../../docs/aws-storage-credentials.md).
The [approved rerun](aws-check.txt) began at `2026-10-05T00:15:23Z` (4 October in Toronto)
with `d12cde1-dirty`, the release build containing this PR's uncommitted changes. STS again
minted a session and all six S3 requests returned `403 AccessDenied`. The script correctly
exited 1, while conformance alone reported the fallback pass and four skips. Startup and
`probe` agree. Four pool objects were deleted; two objects present before the check were
left in place (two before, two after).

The user then supplied the IAM policies. The caller's S3 policy and `.env.aws` named the same
bucket, but the read role's two resource ARNs named a different bucket. The role ARN matched;
STS success did not supply the missing S3 grants. After the user corrected both bucket ARNs
in the role policy, the [successful rerun](aws-check-after-role-fix.txt) began at
`2026-10-05T00:38:01Z` (4 October in Toronto), build `a108f12`. Startup and `probe` both read
the descriptor and list the drive (200), while the pool root, another drive's listing/descriptor
and a write are refused (403 AccessDenied). Conformance reports **4 passed, 0 failed, 1 skipped**;
the skip is the no-credentials case. The script exits 0, honors create-if-absent and purges four
objects from its own pool. The two pre-existing objects remain (two before, two after).
The earlier failed result is preserved in `aws-check.txt`.

### R2 follow-up status

R2 minting now uses Cloudflare's temporary-credentials API with `object-read-only`, the exact
pool `voidfs.json` object and `shards/`, `pages/`, `drives/<id>/` prefixes. The account is derived
from the R2 endpoint and the server's explicit static S3 access key id is the parent. This is
verified against the real bucket in the [approved live check](r2-check.txt), which began at
`2026-10-05T00:14:58Z` (4 October in Toronto), build `d12cde1-dirty`. Startup and `probe`
both read `voidfs.json` and list the scoped drive (200), while the pool root, another drive's
listing/descriptor and a write are refused (403 AccessDenied). All four applicable
conformance cases pass; the no-credentials case skips. Create-if-absent is honored. The
script exits 0, deletes four test-pool objects, and leaves `voidfs-bench/` empty (zero before
and after). This settles that R2's object-read-only credentials can list the granted drive
prefix while refusing the broader listings required by the startup check. Jurisdiction
endpoints and lifetimes below 900 seconds were not exercised.

Local verification passes the workspace tests, clippy, conformance validator, and memory/fs/
versitygw interoperability (boto3 skipped; MinIO unavailable locally). The fake R2 API passes
the four applicable credentials conformance cases and rejects overly broad listings. Five
script regression tests pass; 27 Rust and 16 script mutations were detected, with all files
restored. Script output labels an uncommitted checkout with `-dirty` so the provider rerun is
distinguishable from the original `d12cde1` report.

## Reproduce

```bash
cargo build --release -p voidfs-server -p voidfs-bench -p voidfs-conformance
cargo build --release -p voidfs-client --example bucketread
bench/scripts/bucket-read.sh                       # MinIO on PATH
BENCH_DOWNLINK=- bench/scripts/bucket-read.sh
bench/scripts/credentials-check.sh <env file>      # a bucket, purged after
BENCH_AWS=1 bench/scripts/bucket-read.sh           # the pool in the AWS bucket .env.aws names
```

For AWS, the env file also names `VOIDFS_STORAGE_CREDENTIALS_ROLE`; review the
[role policy](../../../docs/aws-storage-credentials.md) and rebuild so failed startup requests
include S3 error codes. For R2, use the account endpoint, static S3 key pair, region `auto`, and
`VOIDFS_R2_API_TOKEN` with account-level **Workers R2 Storage Write** access. The check script
also accepts `VOIDFS_TOKEN_VALUE` as a fallback token name and selects `auto` when the R2 region
is absent. The server itself requires `VOIDFS_R2_API_TOKEN`; the script exports the fallback
under that name. `VOIDFS_STS_ENDPOINT`, if set, is the Cloudflare API base override for R2.

After reviewing the AWS role and setting the R2 API token, run the provider checks separately:

```bash
bench/scripts/credentials-check.sh .env.aws | tee bench/results/storage-credentials/aws-check.txt
bench/scripts/credentials-check.sh .env.r2 | tee bench/results/storage-credentials/r2-check.txt
```

Record provider output only after running the check. A scoped-reader pass needs the first two
requests to succeed and the other four to be refused; four skipped direct-read cases do not
establish it. The writable mount and its direct/API read paths follow
[step 5's plan](../../../docs/step-5-macos.md).
