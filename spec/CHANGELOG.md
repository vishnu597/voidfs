# Spec changelog

Every change to the [protocol](protocol.md), the [format](format.md) and the
[conformance suite](conformance/) is recorded here, newest first. Drafts may change
incompatibly; entries say when they do.

## Draft 1, revision 2: garbage collection (2026-09-27)

[RFC 0002](../rfcs/0002-gc-safe-against-writers.md). Drafts may change incompatibly; this does
for collectors and writers, not for readers.

- **Format §12:** `gc/pending.json` has a `phase` (`marking`, `waiting`, `deleting`), is created
  with a create-if-absent write, and takes `t1` from its own Last-Modified time. Phase 2 marks
  the run `deleting` before it recomputes and deletes. The grace is at least 24 hours unless no
  writer is active; phase 1 finishes within half of it.
- **Format §12.4** (was §12.3): a writer's checks cover every shard and page it references,
  including ones it has just uploaded. A rescued candidate is re-checked for the `deleting`
  mark. References seen through any root count. Cached run records are allowed if they are
  re-read hourly.
- **Format §7.4:** writing an object no longer confirms it on its own.
- **Format §10:** a soft-deleted drive stays a root until it is hard-deleted, and MAY be
  hard-deleted after its window. A hard delete starts with `drive.json`, after operations on the
  drive have finished.

## Draft 1, revised by the first implementation (2026-09-26)

Changes found while building the Phase 1 server. Drafts may change incompatibly; these do.

- **Format §7.7:** the ETag is derived from the content descriptor, not from a hash of the
  bytes or the version id. A restore or a copy within a pool keeps the ETag.
- **Format §7.5:** a recursively removed folder keeps its children attached, so removing and
  restoring a folder is one change each.
- **Format §8.5:** a fork's point-in-time window starts at its fork point.
- **Protocol §2:** checksums in request headers are verified too, and Signature Version 2 is
  refused with `400 InvalidRequest`.
- **Protocol §3:**
  - GetBucketLocation and ListObjects (v1) added.
  - `encoding-type=url` supported. The AWS CLI's `aws s3` commands depend on it.
  - Only `/` is supported as a delimiter.
- **Protocol §4.6:** a file key with `x-voidfs-as-of` restores the version current then.

## Draft 1 (2026-09-26)

First drafts.

- **Protocol 1:**
  - The standard S3 subset.
  - Object extensions: offset writes, patches, splices, history, reads as of an instant,
    rollback including subtree restore, rename with replace, attributes, listing with
    attributes, recently deleted, and direct upload.
  - Drive extensions: create, fork, soft and hard delete, undelete, describe, read-only storage
    credentials, and the change feed.
- **Format 1:**
  - The pool descriptor with feature flags, and raw content-addressed shards (FastCDC-2020).
  - Manifest trees.
  - A commit log with create-if-absent sequence claims.
  - Content-addressed checkpoint segments for four tables: `entries`, `objects`, `history` and
    `removed`.
  - Self-contained forks.
  - Retention, multipart staging, and two-phase garbage collection.
  ([RFC 0001](../rfcs/0001-metadata-in-the-bucket.md), accepted 2026-09-27.)
- **Conformance:** 35 cases and the case file format, with `validate.py`.
