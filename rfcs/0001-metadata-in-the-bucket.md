# RFC 0001: Drive metadata as a commit log and checkpoints in the bucket

| | |
|---|---|
| Status | Proposed |
| Author(s) | voidfs maintainers |
| Created | 2026-09-26 |
| Affects | format (and the cost model of the protocol) |
| Implemented by | [`spec/format.md`](../spec/format.md) draft 1 |

## Summary

Every drive's metadata (its namespace, the current state of every file, and the history of every
version) lives in the user's bucket. It is stored as an append-only **commit log** of JSON
objects, one per commit, each created with a create-if-absent write. Periodic **checkpoints**
hold the full state as sorted, content-addressed segments. Nothing about a drive's contents is
held only by the voidfs server.

## Motivation

voidfs promises "your bucket, your data" and no lock-in. That only holds if someone with the
bucket and the spec can read every drive without a voidfs server, and if a new server can take
over a drive after the old one is gone. It also gives the service a property worth having: it
keeps no durable drive state of its own, so it needs no backup.

The metadata engine decides how that promise is kept, what a write costs, and how hard the
format is to specify.

## Design

In short (the normative text is [`spec/format.md`](../spec/format.md)):

- **Commit log.** `drives/<id>/log/<seq>.json`. A commit holds one or more transactions; each
  transaction is exactly one version. A writer claims sequence number `n` by creating that
  object with `If-None-Match: *`. Two writers racing for `n` cannot both succeed, and whoever
  wrote `n` must have seen `n-1`. That makes the log linearizable without a lease. Leases then
  only exist to stop two servers from thrashing, not for correctness.
- **Group commit.** The authority may pack many transactions into one commit object. That
  amortizes the one-PUT-per-commit cost under load.
- **Checkpoints.** Sorted segments for four tables (`entries`, `objects`, `history`, `removed`), stored
  content-addressed under `pages/` and listed by `checkpoints/<seq>.json`. A new checkpoint
  reuses every segment that did not change. Readers load the newest checkpoint and replay the
  log after it.
- **Forks** write their own first checkpoint at the fork point. Because segments are
  content-addressed, that checkpoint mostly reuses the parent's segments. A fork is
  self-contained from birth, so a parent can be deleted while forks live, and fork chains have
  no depth limit.
- **Cost.** A mutation costs its new shards plus its share of one commit object. Space's
  documented design costs four objects per version (shard, transaction, version locator and
  record head). Checkpoints add an amortized cost.

## Compatibility

This is the initial format, so there is nothing to be compatible with. The format carries a
version and feature flags from the start (`voidfs.json`).

## Conformance

The format is exercised indirectly by every protocol case. A format-level suite (fixtures of
bucket contents with the state a reader must derive from them) will be added in Phase 1.

## Alternatives

| Option | Why not (for now) |
|---|---|
| **SlateDB** (an LSM key-value store on object storage, with single-writer fencing) | Mature fencing and compaction for free. But the on-bucket format becomes SlateDB's SST and manifest format, which voidfs does not control and cannot specify. Readers would need SlateDB, which weakens the "readable with just the spec" promise. It stays a candidate for the **server's cache** of drive state, which is not part of the format |
| **Postgres (or another database) as the source of truth** | Simple and fast, but drive state then lives outside the bucket. Losing the database loses the drives, which breaks the core promise |
| **One locator object per version** (Space's documented design) | Simple to read, but every version costs extra PUTs and the history is spread over many small objects |
| **One database file per drive, shipped to the bucket** (Litestream style) | The format becomes SQLite's page format plus a WAL-shipping format, and concurrent takeover is hard to make safe |

## Open questions

1. **Backends without create-if-absent.** Backblaze B2 and some other S3-compatible stores do
   not document conditional PUT. The draft requires an external guard for them
   (`commit_guard: "external"`). Is single-node-only acceptable for those backends in v1?
2. **GC safety.** The draft's two-phase GC with a pending-deletion set needs a model check
   (TLA+ or a deterministic simulation) before the format is marked stable.
3. **Checkpoint cadence.** Every 1,000 commits or 16 MiB of log, whichever comes first, is a
   guess to be tuned with the benchmark harness.
4. **Encoding.** JSON keeps the format readable. If manifests and segments turn out to be too
   large or too slow to parse, a binary encoding would arrive behind an incompatible feature
   flag.
