# voidfs

**An open-source filesystem on your own bucket.** Mount one drive on every Mac, reach the same
drive over S3 from any program, and let AI agents work on a fork of it. Every write is a
version, a fork costs nothing whatever the drive's size, and files stream by byte range, so a
terabyte drive uses no local disk.

voidfs is the service layer only. The bytes live in a bucket you already have: Amazon S3,
Cloudflare R2, MinIO, and others. There is no hosted voidfs; you run it.

> **Status: Phase 1 (engine and S3 server), pre-alpha.** The server passes the whole
> [conformance suite](spec/conformance/) and works with stock boto3, rclone and the AWS CLI. There is
> no Mac drive yet. Garbage collection reclaims deleted drives and abandoned uploads, but every
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

## Repository

| Path | What |
|---|---|
| [`spec/`](spec/) | Normative specs: [wire protocol](spec/protocol.md), [on-bucket format](spec/format.md), [conformance suite](spec/conformance/) |
| [`crates/voidfs-core`](crates/voidfs-core/) | The engine: format types, chunking, in-place edits, manifests, drive state, planning |
| [`crates/voidfs-server`](crates/voidfs-server/) | The S3 server: storage backends, commit log, checkpoints, forks, SigV4, change feed |
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
