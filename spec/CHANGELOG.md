# Spec changelog

Every change to the [protocol](protocol.md), the [format](format.md) and the
[conformance suite](conformance/) is recorded here, newest first. Drafts may change
incompatibly; entries say when they do.

## Draft 1, revision 8: storage credentials read the pool's descriptor (2026-10-04)

Step 4, item 6. Additive: a reader with storage credentials can now check the pool before reading
a drive, as the format requires.

- **Protocol §5.5:** `readable` lists `voidfs.json` as well as `shards/`, `pages/` and the
  drive's prefix, since a reader checks the pool's format and features first (format §3.1). The
  credentials list the drive's prefix too, and an entry ending in `/` is a prefix.
- **Conformance:** `bucket` steps, a request to the storage that the case's last storage
  credentials describe, signed with them: an object by its `path` under the pool's root, the
  `shard` that holds some content, or a `list` of a prefix. `storage-credentials-not-offered`,
  `storage-credentials-describe-the-drives-storage`, `storage-credentials-read-the-drive`,
  `storage-credentials-reach-no-further` and `storage-credentials-for-read-keys`, which a server
  that answers `501` skips but the first.

## Draft 1, revision 7: direct uploads, as built (2026-10-03)

Step 4, item 5, recorded here on 4 October. Additive.

- **Protocol §4.11:** `x-amz-meta-*` headers apply to a commit as to a PutObject.
- **Protocol §9:** direct uploads show what a pool holds: a plan's `held` count, and a commit
  that references a shard by its hash.
- **Conformance:** `upload` steps, the `plan`, `commit` and `bytes` bodies, `unique` content and
  `skip_if`, with seven cases, `direct-upload-*`, which a server without direct uploads skips but
  `direct-upload-not-offered`.

## Draft 1, revision 6: a version for every object it changes (2026-10-02)

[RFC 0004](../rfcs/0004-a-version-for-every-object-it-changes.md). Readers that do not implement
it refuse pools that use it, through the new feature flag. Clients see more history rows and feed
changes; no request, header or response member is added or removed.

- **Format §3.1:** the `multi-object-versions` incompatible feature, added by an operator once
  every writer of the pool implements it.
- **Format §7.1, §7.5:** with it, a transaction's version is its target's and that of every
  object it changes, each with a `history` row; an object it takes out gets a `delete` row, and
  one it leaves as it was gets none. Without it, the rule is unchanged and now written down.
- **Format §8.2:** `history` rows list `attrs`, and `removed` rows `last_version`, `kind` and
  `size`, which writers already stored.
- **Protocol §1.1, §1.2:** a version can belong to several objects: a folder restore's, a write's
  that creates folders, a rename's that replaces a file.
- **Protocol §3, §4.5:** a `versionId`, in a read or a copy source, is a version of the object at
  the key, or of one deleted from it. A version of another object is `404 NoSuchVersion`.
- **Protocol §4.4:** `delete` is listed among the operations, and `isLatest` is the newest
  version listed.
- **Protocol §4.6, §4.7:** what a folder restore and a replacing rename record for each file.
- **Protocol §5.6:** a version's changes in order: implicit folders, what it removed, its
  target, then the other objects it changed; a folder restore that moves an object back carries
  `fromKey`.
- **Conformance:** `requires` accepts `feature:<name>`, and the runner takes `--feature`.
  `folder-restore-versions-each-object`, `writes-version-the-folders-they-make` and
  `folder-restore-in-the-feed` need `multi-object-versions`;
  `replaced-file-comes-back-by-its-last-version` and `version-of-another-object` do not.

## Draft 1, revision 5: small content inside the content descriptor (2026-09-29)

[RFC 0003](../rfcs/0003-small-content-in-descriptors.md). Readers that do not implement it refuse
pools that use it, through the new feature flag; nothing else changes for them.

- **Format §3.1:** the `inline-data` incompatible feature, added by an operator once every writer
  of the pool implements it.
- **Format §5:** a third extent kind, the data extent `{ "d": "<base64>" }`, holds up to 4,096
  bytes of a file's content in its descriptor, so a small file needs no shard. Not in manifest
  pages (§5.1) or multipart staging records (§11).
- **Format §7.7:** a data extent contributes to the ETag exactly what a shard extent with the same
  bytes would, so where the bytes live never shows.
- **Format §8.2:** checkpoints never carry data extents: their writer stores the bytes as shards
  and lists shard extents instead, and history rows carry an equivalent descriptor.
- **Format §12:** data extents reference nothing; the shards a checkpoint stores for them are
  referenced by it.
- **Conformance:** `small-files-roundtrip`, `small-file-edits` and `small-file-history`. A client
  cannot tell whether a server used data extents; these check that nothing shows either way.

## Draft 1, revision 4: header authentication sends its payload hash (2026-09-29)

- **Protocol §2:** a request signed in the `Authorization` header MUST send
  `x-amz-content-sha256`, and a server rejects one without it with `400 InvalidRequest`, as S3
  does. Servers had assumed `UNSIGNED-PAYLOAD` instead, so a client that signed the body's hash
  without sending the header (curl 7.88 does) got `403 SignatureDoesNotMatch`, which pointed at
  the wrong problem. SDKs, boto3, the AWS CLI, rclone and curl 8 always send it. There is no
  conformance case: the runner's signer always sends it. `tests/interop/aws_chunked.py` checks
  it.

## Draft 1, revision 3: the signature covers `host` (2026-09-29)

- **Protocol §2:** `host` MUST be in `SignedHeaders`, in either addressing style, and a server
  rejects a signature that leaves it out with `403 AccessDenied`, as S3 does. Signature Version
  4 already required it, and every SDK, boto3, the AWS CLI, rclone and curl sign it; servers had
  not checked. With virtual-host addressing the host names the drive, so an unsigned host would
  let one signature reach any drive. There is no conformance case: the runner signs with
  `aws-sigv4`, which always signs `host`. `tests/interop/aws_chunked.py` checks it.

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
