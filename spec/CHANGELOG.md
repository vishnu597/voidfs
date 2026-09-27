# Spec changelog

Every change to the [protocol](protocol.md), the [format](format.md) and the
[conformance suite](conformance/) is recorded here, newest first. Drafts may change
incompatibly; entries say when they do.

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
  ([RFC 0001](../rfcs/0001-metadata-in-the-bucket.md), Proposed.)
- **Conformance:** 35 cases and the case file format, with `validate.py`.
