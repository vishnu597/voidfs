# voidfs

**An open-source filesystem on your own bucket.** Mount one drive on every Mac, reach the same
drive over S3 from any program, and let AI agents work on a fork of it. Every write is a
version, a fork costs nothing whatever the drive's size, and files stream by byte range, so a
terabyte drive uses no local disk.

voidfs is the service layer only. The bytes live in a bucket you already have: Amazon S3,
Cloudflare R2, MinIO, and others. There is no hosted voidfs; you run it.

> **Status: Phase 1 (engine and S3 server), pre-alpha.** The server passes the whole
> [conformance suite](spec/conformance/) and works with stock boto3 and the AWS CLI. There is
> no Mac drive yet, and no garbage collection, so deleted data is not reclaimed. The protocol
> and on-bucket format are drafts and will change. See the
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

## Repository

| Path | What |
|---|---|
| [`spec/`](spec/) | Normative specs: [wire protocol](spec/protocol.md), [on-bucket format](spec/format.md), [conformance suite](spec/conformance/) |
| [`crates/voidfs-core`](crates/voidfs-core/) | The engine: format types, chunking, in-place edits, manifests, drive state, planning |
| [`crates/voidfs-server`](crates/voidfs-server/) | The S3 server: storage backends, commit log, checkpoints, forks, SigV4, change feed |
| [`crates/voidfs-conformance`](crates/voidfs-conformance/) | Runs the conformance suite against any endpoint |
| [`tests/interop/`](tests/interop/) | Checks with stock S3 clients (boto3) |
| [`rfcs/`](rfcs/) | Proposals for changes to the specs |
| [`docs/`](docs/) | Research, architecture and roadmap |

## Contributing

Contributions are welcome under the [Developer Certificate of Origin](DCO): sign off every
commit with `git commit -s`. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[Apache License 2.0](LICENSE). The name "voidfs" is covered by the [trademark policy](TRADEMARKS.md).
