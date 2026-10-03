# voidfs

**An open-source filesystem on your own bucket.** Mount one drive on every Mac, reach the same
drive over S3 from any program, and let AI agents work on a fork of it. Every write is a
version, a fork costs nothing whatever the drive's size, and files stream by byte range, so a
terabyte drive uses no local disk.

voidfs is the service layer only. The bytes live in a bucket you already have: Amazon S3,
Cloudflare R2, MinIO, and others. There is no hosted voidfs; you run it.

> **Status: Phase 1 (engine and S3 server), pre-alpha.** The server passes the whole
> [conformance suite](spec/conformance/) and works with stock boto3, rclone and the AWS CLI, and
> there is a [command line](#the-command-line) and a [Rust SDK](#the-rust-sdk). There is no Mac
> drive yet. Garbage collection reclaims deleted drives and abandoned uploads, but every
> version of a file in a live drive is kept, because there are no retention policies yet. The
> protocol and on-bucket format are drafts and will change. See the
> [roadmap](docs/RESEARCH_AND_PLAN.md#9-phased-roadmap).

## Why

- **Your bucket, your data.** Drive contents *and* drive metadata live in your bucket, in a
  [documented format](spec/format.md). Lose the voidfs server and you lose nothing.
- **Plain S3 for programs.** Any S3 client works unchanged. [Extensions](spec/protocol.md) add
  what S3 cannot express: writes at an offset, byte insertion and removal, O(1) renames of whole
  folders, a version for every change, time travel, and copy-on-write forks of a whole drive.
- **A native drive on the Mac.** Built on Apple's FSKit, with no kernel extension. Windows
  comes next.
- **Built for agents.** Forks are sandboxes, scoped keys are fences, versions are undo, and
  preconditions stop agents from overwriting each other.

## Try it

You need Rust 1.90 or later.

```bash
cargo run --release -p voidfs-server -- --store fs:./voidfs-data
```

The server prints an admin access key it generated for the run. Use it with any S3 client,
with Signature Version 4:

```bash
export AWS_ACCESS_KEY_ID=<printed id> AWS_SECRET_ACCESS_KEY=<printed secret> AWS_DEFAULT_REGION=us-east-1
aws --endpoint-url http://127.0.0.1:9000 s3 mb s3://footage
aws --endpoint-url http://127.0.0.1:9000 s3 cp ./clip.mov s3://footage/cuts/
```

`--store` also takes `memory`, and `s3:<bucket>[/<prefix>]` for Amazon S3 or, with
`--s3-endpoint`, any S3-compatible bucket such as Cloudflare R2 or MinIO. Run the conformance
suite against a server:

```bash
VOIDFS_ENDPOINT=http://127.0.0.1:9000 VOIDFS_ACCESS_KEY_ID=<id> VOIDFS_SECRET_ACCESS_KEY=<secret> \
  cargo run -p voidfs-conformance
```

### The command line

`void` ([`crates/voidfs-cli`](crates/voidfs-cli/)) works with drives from a terminal: create, fork
and delete them, upload into them, and read and restore their history.

```bash
cargo install --path crates/voidfs-cli
eval "$(void keys generate --scope admin --format env)"   # sets VOIDFS_ACCESS_KEY_ID and _SECRET_ACCESS_KEY
cargo run --release -p voidfs-server -- --store fs:./voidfs-data &   # takes its admin key from them

void drive create footage
void upload clip.mov renders/ footage:/cuts   # files and folders; large files go up in parts
void drives
void history footage cuts/clip.mov            # every version, and the time of each
void show footage cuts/clip.mov --at 2026-10-01T09:00:00Z -o old.mov
void restore footage cuts --at 2026-10-01T09:00:00Z   # a whole folder, as one new version
void fork footage footage-try                 # a copy-on-write copy, ready at once
void drive delete footage-try                 # asks for the name; recoverable for 30 days
```

- The server and key come from `VOIDFS_ENDPOINT` (default `http://127.0.0.1:9000`),
  `VOIDFS_ACCESS_KEY_ID` and `VOIDFS_SECRET_ACCESS_KEY`, or from `--endpoint` and `--key-file`
  (a file `void keys generate --format json` or `--format env` wrote).
- Drives are named by alias or id. `history`, `show` and `restore` also take
  `--bucket <drive> <path>`, as SpaceFS's CLI does. `--at` takes RFC 3339 or Unix seconds.
- A folder's history lists every version of every file in it. Asked about a deleted file,
  `history`, `show` and `restore` say how to bring it back.
- With `--json`, every command prints one JSON document on stdout, and errors go to stderr as
  `{"error": {"code", "status", "message", …}}`, with exit status 1 (2 for a usage error).
- `void keys generate --scope read|write|admin` makes more keys: give each to the server as
  `--key <id>:<secret>:<scope>`.
The daemon is the per-user agent that owns the client core: the upload queue, the block cache and
the link to the server. It answers on a socket in the state directory
(`~/Library/Application Support/voidfs`, or `VOIDFS_STATE_DIR`), in HTTP and JSON. With it,
uploads go on when the command that started them stops, and survive the daemon's restart.

```bash
void daemon start                     # in the background; it keeps the server and key it was given, for you only
void upload --detach renders/ footage:/cuts   # hand it the batch and return
void uploads --watch                  # what is uploading, the batches, the rate and the time left
void uploads pause --all              # also: resume, cancel, by id, --batch or --drive
void uploads limit 10MiB              # at once; `unlimited` lifts it
void status                           # the daemon, the server and the link to it, uploads and the cache
void daemon info                      # its build, what its journal holds unpublished, and whether a restart is safe
```

- `void upload` without `--detach` hands the batch to the daemon and follows it until it is up;
  Ctrl-C leaves it uploading. With no daemon running it uploads in the foreground, and
  `--detach` starts one.
- The mount table comes next ([step 4's plan](docs/step-4-client.md)).

### The Rust SDK

[`crates/voidfs-sdk`](crates/voidfs-sdk/) is the official AWS SDK for S3 (`client.s3()`, for
multipart uploads and anything else standard) plus a typed call for every extension: offset
writes, patches, inserts and removals, renames, history and rollback, forks, attributes, listings
with attributes and the change feed.

```toml
[dependencies]
voidfs-sdk = { git = "https://github.com/vishnu597/voidfs" }
```

```rust
use voidfs_sdk::{Client, Edit, Preconditions, WriteOptions};

// VOIDFS_ENDPOINT (default http://127.0.0.1:9000), VOIDFS_ACCESS_KEY_ID, VOIDFS_SECRET_ACCESS_KEY
let client = Client::from_env()?;
client.create_drive("footage", Default::default()).await?;
let v1 = client.put_object("footage", "cut.txt", "hello world", Default::default()).await?;
client.write_at("footage", "cut.txt", 6, "WORLD", WriteOptions { if_version: Some(v1.version_id), ..Default::default() }).await?;
client.patch("footage", "cut.txt", &[Edit::new(0, "H")], Default::default()).await?;
let head = client.head_object("footage", "cut.txt", Default::default()).await?;
client.insert("footage", "cut.txt", 5, ",", Preconditions::if_version(head.version_id)).await?;
for v in client.list_versions("footage", "cut.txt", false).await? {
    println!("{} {} {}", v.version_id, v.last_modified, v.operation);
}
```

Every call fails with one error type, which carries the status, the S3 code and, on a `412`, the
version that won. Calls are retried on server errors and broken connections, except an insert or
removal without a precondition, which is never sent twice: pass `if_version` to make it safe to
retry. The client core comes next ([step 4's plan](docs/step-4-client.md)).

### Virtual-host addressing

Drives are always served path-style, at `https://voidfs.example.com/<drive>/<key>`. To also
serve `https://<drive>.voidfs.example.com/<key>`, which some S3 clients use by default:

1. Point a wildcard DNS name, `*.voidfs.example.com`, at the server as well as
   `voidfs.example.com` itself.
2. Give TLS a certificate for both names (for example from Let's Encrypt, which issues wildcard
   certificates through its DNS challenge).
3. Start the server with `--virtual-host-domain voidfs.example.com` (or
   `VOIDFS_VIRTUAL_HOST_DOMAIN`). Repeat it, or separate names with commas, to serve several
   names. The longest name that matches wins.

Requests to `voidfs.example.com` itself, and to any other name or address, stay path-style.
Clients sign the `Host` header, so a proxy in front of the server must pass it through
unchanged (nginx: `proxy_set_header Host $host;`). A wildcard certificate covers only one
label, so a drive whose name contains dots, such as `a.b`, gets a certificate error at
`a.b.voidfs.example.com`: reach it path-style.

To try it locally, `curl` and browsers resolve every name under `localhost` to the loopback address:

```bash
cargo run --release -p voidfs-server -- --store memory --virtual-host-domain s3.localhost
curl --aws-sigv4 aws:amz:us-east-1:s3 --user <id>:<secret> -X PUT http://footage.s3.localhost:9000/
VOIDFS_ENDPOINT=http://127.0.0.1:9000 VOIDFS_ACCESS_KEY_ID=<id> VOIDFS_SECRET_ACCESS_KEY=<secret> \
  cargo run -p voidfs-conformance -- --virtual-host s3.localhost
```

`tests/interop/` has more checks in this style: `virtual_host.sh` (curl), and `boto3_smoke.py` and
`aws_chunked.py` with `VOIDFS_ADDRESSING=virtual`.

### With Docker Compose

[`deploy/compose/`](deploy/compose/) runs the server in a container, with its pool in a volume, in
a bucket of yours, or in a local S3 server beside it:

```bash
cd deploy/compose && cp example.env .env   # then set the keys in .env
docker compose up -d --build --wait
```

### Checking the bucket

Commits rely on the bucket refusing to create an object that already exists (`If-None-Match: *`,
[format §7.2](spec/format.md#72-claiming-a-sequence-number-create-if-absent)). A server checks
that before it writes the pool, and refuses a bucket that ignores it. It also refuses lifecycle
rules that would delete or archive the pool's objects. `probe` reports these and the rest of
what the bucket supports, and stores nothing:

```bash
cargo run --release -p voidfs-server -- probe --store s3:<bucket>/<prefix> --s3-endpoint <url>
```

For a bucket without conditional writes, create the pool with `--commit-guard external`. At most
one server, and one garbage collector, may then write it
([§7.3](spec/format.md#73-external-guard)).

### Small files in the log

A file of up to 4 KiB can be held in the log itself, so that writing it takes one request to the
bucket instead of two, one after the other: its shard, then the log entry. This is the pool's
`inline-data` feature ([RFC 0003](rfcs/0003-small-content-in-descriptors.md),
[format §5](spec/format.md#5-content-and-manifests)). Servers and readers older than it refuse a
pool that has it, so it is off until you turn it on:

- **A new pool:** start the server that creates it with `--new-pool-feature inline-data` (or
  `VOIDFS_NEW_POOL_FEATURES=inline-data`). The option changes nothing in a pool that exists.
- **An existing pool:** once every server that writes it runs this version or later, add the
  feature, then restart them, since servers read the pool's `voidfs.json` when they start:

  ```bash
  cargo run --release -p voidfs-server -- pool enable inline-data --store s3:<bucket>/<prefix> --s3-endpoint <url>
  ```

A feature cannot be removed again. Checkpoints store small files as shards, in the background, so
only a drive's log since its last checkpoint holds them, and the server keeps no more of them in
memory than that: about 12 MB of small files a drive.

### A version for every object it changes

With the pool's `multi-object-versions` feature
([RFC 0004](rfcs/0004-a-version-for-every-object-it-changes.md),
[format §7.5](spec/format.md#75-changes)), a folder restore is one version in the history of the
folder and of every file it rolls back, brings back or removes, so each file's history lists it
and `?versionId=` reads the file as restored; a write that creates folders is their first
version. Without it, only the folder's history has the restore. Turn it on as `inline-data`
above (`--new-pool-feature multi-object-versions`, or `pool enable multi-object-versions`). History
already in checkpoints when it is turned on stays as it was recorded.

### Garbage collection

Content that nothing references any more, such as the content of hard-deleted drives, is
deleted in two phases at least a day apart ([format §12](spec/format.md#12-garbage-collection)).
Each run of `gc` does whichever phase is due; `--dry-run` only reports:

```bash
cargo run --release -p voidfs-server -- gc --store fs:./voidfs-data --dry-run
cargo run --release -p voidfs-server -- gc --store fs:./voidfs-data
```

Or let the server collect on its own with `--gc-interval 1h`. A run also hard-deletes drives
soft-deleted more than 30 days ago (`--expire-deleted-drives`) and aborts multipart uploads open
more than 7 days (`--abort-uploads`). Only when no server is using the pool, `--offline --grace 0`
collects everything unreferenced in one run.

### Health checks and metrics

`--admin-listen <address>` (or `VOIDFS_ADMIN_LISTEN`) serves three endpoints on a port of their
own, since on the S3 port every path is a drive. It is off unless given.

| Endpoint | Answers |
|---|---|
| `GET /healthz` | 200 while the process is up |
| `GET /readyz` | 200 once the pool is open and the S3 port listening, while the bucket answers; 503 and the reason otherwise, and while shutting down. The server's own requests to the bucket vouch for it if the last one succeeded within 30 seconds; otherwise a HEAD of `voidfs.json` checks, at most once every 5 seconds |
| `GET /metrics` | Prometheus's text format, below |

```bash
cargo run --release -p voidfs-server -- --store memory --admin-listen 127.0.0.1:9001
curl http://127.0.0.1:9001/readyz
```

Nothing on the admin port is authenticated, as is usual for these endpoints: keep it on loopback
or a private network, and never publish it. No label names a drive or a key.

| Series | Labels | What |
|---|---|---|
| `voidfs_s3_requests_total` | `op`, `status` (`2xx` to `5xx`) | Requests to the S3 port |
| `voidfs_s3_request_duration_seconds` | `op` | Their latency until the response's headers (histogram) |
| `voidfs_bucket_requests_total`, `voidfs_bucket_request_errors_total` | `op` | Requests to the bucket (or the `fs:` or `memory` store), and those that failed |
| `voidfs_bucket_request_duration_seconds` | `op` | Their latency (histogram) |
| `voidfs_cache_hits_total`, `voidfs_cache_misses_total`, `voidfs_cache_evictions_total` | `cache` (`shard`, `page`) | Reads served from memory, and from the bucket (one request each); entries evicted for room |
| `voidfs_cache_coalesced_total` | `cache` | Reads that missed while another read was fetching the same shard or page, and waited for that fetch rather than ask the bucket again |
| `voidfs_cache_bytes`, `voidfs_cache_entries`, `voidfs_cache_capacity_bytes` | `cache` | What each cache holds, and the most it may |
| `voidfs_cache_drops_total` | | Times the caches were emptied on SIGUSR1 (below) |
| `voidfs_commit_transactions` | | Transactions in each log entry written: group commit's batches (histogram) |
| `voidfs_commit_log_write_seconds` | | Time to write each log entry (histogram) |
| `voidfs_commits_total`, `voidfs_checkpoints_total` | `outcome` | Log entries written, lost to another server's, or failed; checkpoints written or failed |
| `voidfs_checkpoint_write_seconds` | | Time to write each checkpoint, in the background after the commit that made it due (histogram) |
| `voidfs_checkpoint_spilled_shards_total` | | Shards checkpoints stored for small files held in the log (`inline-data`) |
| `voidfs_checkpoint_swap_seconds` | | Time each such checkpoint then held its drive's commits, to take those shards into memory in place of the files' bytes (histogram) |
| `voidfs_gc_phase` | `phase` | The garbage-collection run in `gc/pending.json`: `none`, `marking`, `waiting` or `deleting` |
| `voidfs_gc_steps_total`, `voidfs_gc_last_step`, `voidfs_gc_last_step_timestamp_seconds` | `outcome` | Steps of `--gc-interval` collection, and the last one |
| `voidfs_gc_deleted_objects_total`, `voidfs_gc_deleted_bytes_total` | | What collection deleted |
| `voidfs_drives` | `state` (`live`, `deleted`) | Drives open |
| `voidfs_uptime_seconds`, `voidfs_build_info` | `version` | The process |

The S3 port's `op` is one of `list_drives`, `drive`, `list`, `get`, `head`, `put`, `copy`,
`delete`, `multipart`, `extension` (the `x-voidfs-*` requests) and `other`. The bucket's is one of
`get`, `head`, `put`, `put_new` (create-if-absent), `delete`, `delete_prefix` and `list`; a listing
or a deletion by prefix may take several requests.

`SIGUSR1` empties the shard and page caches, so that the next reads go to the bucket as after a
restart; the benchmark's cold runs use it. Only someone who can signal the process can do it, and
nothing on either port does. The drives' state, which the server serves from, stays in memory.

## Repository

| Path | What |
|---|---|
| [`spec/`](spec/) | Normative specs: [wire protocol](spec/protocol.md), [on-bucket format](spec/format.md), [conformance suite](spec/conformance/) |
| [`crates/voidfs-core`](crates/voidfs-core/) | The engine: format types, chunking, in-place edits, manifests, drive state, planning |
| [`crates/voidfs-server`](crates/voidfs-server/) | The S3 server: storage backends, commit log, checkpoints, forks, SigV4, change feed |
| [`crates/voidfs-sdk`](crates/voidfs-sdk/) | The Rust SDK: the AWS SDK for S3 plus typed calls for the extensions |
| [`crates/voidfs-cli`](crates/voidfs-cli/) | `void`, the command line, on the SDK |
| [`crates/voidfs-client`](crates/voidfs-client/) | The client core the daemon runs, and the mount will: the block cache with read-ahead, the write journal and upload queue, and the change-feed client with connectivity, on the SDK |
| [`crates/voidfs-daemon`](crates/voidfs-daemon/) | The per-user daemon `void daemon run` runs: the client core behind a Unix socket, with HTTP and JSON over it |
| [`crates/voidfs-conformance`](crates/voidfs-conformance/) | Runs the conformance suite against any endpoint |
| [`crates/voidfs-bench`](crates/voidfs-bench/), [`bench/`](bench/) | SpaceFS's 49 benchmark scenarios, run through voidfs and against the bare bucket; scripts and results |
| [`tests/interop/`](tests/interop/) | Checks with stock S3 clients (boto3, rclone); `run.sh` runs them and the conformance suite over each kind of store |
| [`deploy/compose/`](deploy/compose/) | Docker Compose files, and a local S3 server to try it with |
| [`.github/workflows/`](.github/workflows/) | CI: tests, lints, conformance and clients over memory, disk, versitygw and MinIO, Compose, and on `main` the GC model and a small benchmark |
| [`rfcs/`](rfcs/) | Proposals for changes to the specs |
| [`docs/`](docs/) | Research, architecture and roadmap |

## Contributing

Contributions are welcome under the [Developer Certificate of Origin](DCO): sign off every
commit with `git commit -s`. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[Apache License 2.0](LICENSE). The name "voidfs" is covered by the [trademark policy](TRADEMARKS.md).
