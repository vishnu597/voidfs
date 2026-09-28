# voidfs on-bucket format

**Status:** Draft 1 · **Format version:** 1 · Conventions: [README](README.md#conventions)

This document defines how voidfs stores drives in an object-storage bucket: file content,
folders, every version of every file, and forks. A reader that implements it can open any drive
with nothing but read access to the bucket. The design rationale is in
[RFC 0001](../rfcs/0001-metadata-in-the-bucket.md).

---

## 1. Model

| Term | Meaning |
|---|---|
| **Pool** | A bucket, or a prefix of a bucket, that holds drives and the content they share. Deduplication and garbage collection work across one pool. |
| **Drive** | A filesystem: a tree of folders and files with a complete, ordered history. |
| **Object** | A file, folder or symbolic link in a drive. Identified by an **object id** that does not change when the object is renamed or moved. |
| **Shard** | An immutable run of content bytes, named by its SHA-256. |
| **Content** | The bytes of one version of a file, described as an ordered list of **extents** (shards and runs of zeros). |
| **Commit** | One object in a drive's log. It holds one or more transactions and has a sequence number. |
| **Transaction** | An atomic change to a drive. Every transaction is exactly one **version**. |
| **Checkpoint** | The complete state of a drive at one sequence number, stored as sorted segments. |
| **Authority** | The process that writes a drive's log. |

State at sequence `n` is defined as: the state of the newest checkpoint at or before `n`, with
every transaction of commits `checkpoint.seq + 1 … n` applied in order.

---

## 2. Layout

A pool lives under a **root** prefix, which MAY be empty. Every path below is relative to it.

```
voidfs.json                                  pool descriptor                     §3
shards/<h0h1>/<h2h3>/<sha256>                shard                               §4
pages/<h0h1>/<h2h3>/<sha256>                 manifest page or checkpoint segment §5, §8
drives/<drive-id>/drive.json                 drive descriptor                    §6
drives/<drive-id>/log/<seq>.json             commit                              §7
drives/<drive-id>/checkpoints/<seq>.json     checkpoint index                    §8
drives/<drive-id>/_last_checkpoint           hint                                §8.4
drives/<drive-id>/deleted.json               soft-delete marker                  §10
drives/<drive-id>/uploads/<upload-id>/...    multipart staging                   §11
gc/pending.json                              garbage-collection pending set      §12
```

- `<h0h1>/<h2h3>` are the first four hex characters of the hash. They spread keys for backends
  that partition by prefix.
- `<seq>` is the sequence number in decimal, zero-padded to 20 digits, so that lexical order is
  numeric order: `00000000000000000042`.
- `<drive-id>` is `d-` followed by a lowercase UUID (RFC 9562, version 4 or 7).
- JSON objects are UTF-8, without a byte-order mark. Readers MUST ignore members they do not
  recognize, unless a feature flag (§3.1) says otherwise.
- Writers MUST NOT store anything else under these paths. Other objects in the bucket, outside
  the root, are untouched and ignored.

---

## 3. Pool descriptor: `voidfs.json`

Written once when the pool is created, with a create-if-absent write. It is never changed,
except to add feature flags as §3.1 allows.

```json
{
  "format": 1,
  "pool_id": "p-0192a7a4-8c1e-7b61-9d3f-3c6a1f0e2b77",
  "created": "2026-09-26T21:00:00Z",
  "features": { "compatible": [], "incompatible": [] },
  "chunking": { "algorithm": "fastcdc-2020", "min": 262144, "avg": 2097152, "max": 16777216 },
  "hash": "sha256",
  "commit_guard": "create-if-absent"
}
```

| Member | Meaning |
|---|---|
| `format` | The format's major version. A reader MUST refuse a pool whose `format` it does not implement. |
| `chunking` | How writers cut content into shards (§4.1). Readers do not need it; writers MUST use it so that identical content deduplicates. |
| `commit_guard` | How concurrent authorities are kept from both writing a log: `create-if-absent` (§7.2) or `external` (§7.3). |

### 3.1 Feature flags

- A writer that uses a feature which older readers would misread MUST add its name to
  `features.incompatible` before writing any such data.
- A reader MUST refuse to open a pool that lists an incompatible feature it does not implement.
- Features in `features.compatible` MAY be ignored by readers.
- Feature names are lowercase words joined with hyphens. Names are allocated by RFC.

Reserved names, not defined in format 1: `shard-zstd` (compressed shards), `encryption`
(encrypted shards and metadata), `external-extents` (extents that point at existing objects in
the bucket, for adopting a bucket in place), `binary-metadata`.

---

## 4. Shards

A shard is stored at `shards/<h0h1>/<h2h3>/<sha256(bytes)>`, and its object body is exactly
those bytes. In format 1 there is no header, no compression and no encryption. A ranged GET of a
shard therefore returns content bytes directly, which lets a reader fetch part of a shard
without downloading it all.

- Shard length MUST be between 1 byte and 16 MiB.
- Readers SHOULD verify the hash of every whole shard they fetch, and MUST NOT serve a shard
  whose hash does not match.
- Shards are immutable. A writer that finds a shard already present MUST NOT overwrite it with
  different bytes (it cannot; the name is the hash). It MAY rewrite it with the same bytes to
  refresh its modification time (§12).

### 4.1 Chunking

Writers cut new content with FastCDC as published in 2020 (the "2020" variant with normalized
chunking level 1). They use the `min`, `avg` and `max` sizes from `voidfs.json`, and the gear
table in [Appendix A](#appendix-a-fastcdc-gear-table).

When an edit changes a region of existing content, a writer SHOULD re-chunk only from the first
shard boundary before the change until a new boundary coincides with an existing boundary
after the change, and reuse every shard outside that window. Writers MAY chunk differently (for
example at multipart part boundaries, §11). Only deduplication suffers; correctness does not
depend on chunk boundaries.

*(informative)* Content-defined boundaries are why an insert in the middle of a large file
rewrites about one shard. The boundaries after the insert re-converge with the old ones, so the
shards after it are byte-for-byte the same.

---

## 5. Content and manifests

The content of a file version is a **content descriptor**, one of:

```json
{ "extents": [ { "s": "<sha256>", "n": 2097152 }, { "z": 4096 }, { "s": "<sha256>", "n": 1048576 } ] }
```

```json
{ "root": "<sha256>", "size": 1099511627776 }
```

- An extent `{ "s": h, "n": len }` is the whole shard `h`, which is `len` bytes long.
- An extent `{ "z": len }` is `len` zero bytes that are not stored (sparse regions, and the gap
  left by extending a file).
- The content is the concatenation of its extents in order. Its size is the sum of their
  lengths. The empty file is `{ "extents": [] }`.
- Adjacent `z` extents SHOULD be merged. Extent lengths MUST be greater than zero.
- A descriptor with more than 1,024 extents MUST be stored as a **manifest tree** and referenced
  with `root`.

### 5.1 Manifest trees

A manifest page is a JSON object stored at `pages/<h0h1>/<h2h3>/<sha256(page bytes)>`:

```json
{ "kind": "leaf", "extents": [ { "s": "…", "n": 2097152 }, … ] }
```

```json
{ "kind": "node", "children": [ { "page": "<sha256>", "size": 2147483648 }, … ] }
```

- A page holds at most 1,024 extents or children.
- `size` is the total content size under that child.
- `root` names a page, and `size` in the descriptor is the total size.
- The tree need not be balanced, but writers SHOULD keep leaves between 256 and 1,024 extents so
  that an edit rewrites one leaf and its ancestors.
- Pages are immutable and shared between versions, files and drives exactly as shards are.

*(informative)* A 1 TiB file with 2 MiB shards is about 524,288 extents: 512 leaves under one
root. An edit anywhere rewrites one leaf page and the root, not a 25 MB manifest.

---

## 6. Drive descriptor: `drives/<drive-id>/drive.json`

Written with a create-if-absent write when the drive is created. Never changed afterwards.

```json
{
  "format": 1,
  "drive_id": "d-3f8e1b2a-4c5d-4e6f-8a9b-0c1d2e3f4a5b",
  "created": "2026-09-26T21:05:00Z",
  "alias": "footage",
  "fork_of": { "drive_id": "d-…", "seq": 1841 }
}
```

- `alias` is the name the drive was created with. The current alias and display name are
  control-plane data and MAY change; this member records only the original.
- `fork_of` is present only for forks (§9). A new, unforked drive's log starts at `seq` 1.

---

## 7. The commit log

### 7.1 Commits

A commit is stored at `drives/<drive-id>/log/<seq>.json`:

```json
{
  "format": 1,
  "seq": 42,
  "time": "2026-09-26T21:14:05.123456Z",
  "authority": "a-6b1f…",
  "txns": [
    {
      "target": "o-01J8Z…",
      "op": "write",
      "actor": { "kind": "key", "id": "VF3KQ7M2N8P4R6T1W5Y9" },
      "changes": [
        { "set": { "oid": "o-01J8Z…", "content": { "extents": [ … ] }, "size": 67108864,
                   "etag": "\"42.0\"", "attrs": { "content_type": "video/quicktime" } } }
      ]
    }
  ]
}
```

- `seq` MUST equal the number in the object's key.
- `time` MUST NOT be earlier than the previous commit's `time` in the same drive's lineage. An
  authority whose clock is behind uses the previous commit's time.
- `authority` identifies the writer instance, for diagnostics.
- `txns` has at least one transaction. The **version id** of transaction `i` (counting from 0)
  in commit `seq` is the string `"<seq>.<i>"`, for example `"42.0"`.
- Clients of the protocol treat version ids as opaque. Only this format gives them structure.
- A transaction's `target` is the object whose version it is.
- `op` records what made it: `put`, `write` (offset write, patch, splice, truncate), `copy`,
  `restore`, `rename`, `attrs` (attributes only), `delete` or `other`.
- `actor` records who made it (`key`, `user`, `mount` or `system`, with an id), for audit.
- `changes` are applied in order. A transaction applies all of its changes or none.

### 7.2 Claiming a sequence number: `create-if-absent`

An authority that has applied every commit up to `n - 1` writes commit `n` with
`If-None-Match: *` (or the backend's equivalent). If the write fails because the object already
exists, another authority wrote `n`. The failing authority MUST discard every transaction it
planned against the state at `n - 1` and reload from the log. It MUST NOT report those
transactions as successful.

Consequences:
- Every commit was built on the state at exactly the commit before it.
- A drive's history is linear, whatever the number of authorities.
- Preconditions checked against state `n - 1` are therefore sound.

A lease that assigns each drive to one authority is RECOMMENDED for performance. Correctness
does not depend on it.

### 7.3 `external` guard

For a backend without create-if-absent writes, the pool descriptor says
`"commit_guard": "external"`. The deployment MUST then guarantee that at most one authority
writes a drive's log at a time (for example one server, or a lock held in a database). A writer
MUST verify that `log/<n>.json` does not exist before writing it, but that check alone is not
sufficient.

### 7.4 Durability order

Before writing a commit, the authority MUST have confirmed that every shard and page the commit
references is durably stored, by the checks in §12.4. Those checks apply to objects it has just
written too: writing an object is not enough on its own while garbage collection runs. A commit
MUST NOT reference an object that is not yet stored.

A successful write of the commit object is the moment the transactions become durable. A server
MUST NOT acknowledge a mutation before that.

### 7.5 Changes

A transaction's `changes` are drawn from this set. Paths do not appear in the log; the namespace
is a tree of `(parent, name)` entries (§7.6).

| Change | Members | Effect |
|---|---|---|
| `create` | `oid`, `parent`, `name`, `kind` (`file`, `folder`, `symlink`) | Adds a namespace entry. `parent` is a folder's oid, or `"root"`. For a new `oid` this creates the object. For an `oid` that has history but no entry (a deleted object), it puts the object back, and its record and history continue. |
| `set` | `oid`, and any of `content`, `size`, `etag`, `attrs`, `restored_from` | Replaces the object's current record with these members. Members not given keep their values. Creates a version of the object when it is the target. |
| `move` | `oid`, `parent`, `name` | Changes the entry's parent and name. A folder moves with its whole subtree. |
| `remove` | `oid`, `recursive` | Removes the object's entry from the namespace. A folder MUST be empty unless `recursive: true`; a recursively removed folder keeps its children attached to it, so they leave the namespace with it and come back with it (§7.5 `create`). The object's history is kept, and a `removed` row records it (§8.2). |

Invariants, which every transaction MUST preserve:
1. Every entry's `parent` is an existing folder, or `"root"`.
2. No two entries share `(parent, name)`.
3. No folder is its own ancestor.
4. `size` of a file equals the size of its `content`.

### 7.6 Names and paths

- A `name` is a non-empty UTF-8 string, at most 255 bytes, that does not contain `/` or NUL and
  is not `.` or `..`.
- Names compare by their UTF-8 bytes. They are case-sensitive and not normalized. A macOS
  client that needs case-insensitive or normalization-insensitive lookup does that itself; the
  format stores names exactly as written.
- The **key** of an object, as seen over S3, is the names from the root joined by `/`, with a
  trailing `/` for a folder.
- **Order:** children of a folder are ordered by their *segment*, which is the name, plus `/` for
  a folder, compared as bytes. A depth-first walk in that order visits objects in exactly the
  lexical order of their keys, which is the order S3 listings require.
- `oid` values are `o-` followed by a ULID (Crockford base32, 26 characters) and are unique
  within a pool.

### 7.7 Object records

The current record of an object, as `set` builds it:

| Member | Type | Meaning |
|---|---|---|
| `content` | content descriptor | Files only |
| `size` | integer | Files: content size. Folders: 0 |
| `etag` | string | Quoted, opaque, and changes whenever the content changes. Writers SHOULD derive it from the content descriptor, so that a restore or a same-pool copy keeps the ETag: `"\"" + hex(sha256(d)) + "\""`, where `d` is `voidfs-inline\0` followed, for each extent, by `s`, the 32-byte hash and the u64 length, or by `z` and the u64 length; or, for a tree, `voidfs-tree\0`, the root hash and the u64 size |
| `attrs.content_type` | string | MIME type |
| `attrs.meta` | object | User metadata (`x-amz-meta-*`), names lowercased |
| `attrs.mtime` | timestamp | Modification time as set by a client (mounts). Defaults to the commit's `time` |
| `attrs.mode` | integer | POSIX permission bits (for example `493` for `0755`). Default `420` for files and `493` for folders |
| `attrs.xattrs` | object | Extended attributes, name to base64 value, at most 64 KiB in total. Larger values are a future feature |
| `attrs.flags` | array of strings | For example `hidden` |
| `target` | string | Symlinks only: the link target |
| `restored_from` | version id | Set by a `restore` |

---

## 8. Checkpoints

### 8.1 Index

A checkpoint at `seq` is stored at `drives/<drive-id>/checkpoints/<seq>.json`:

```json
{
  "format": 1,
  "seq": 1000,
  "time": "2026-09-26T23:00:00Z",
  "tables": {
    "entries": [ { "page": "<sha256>", "first": "…", "last": "…", "count": 4096 } ],
    "objects": [ … ],
    "history": [ … ],
    "removed": [ … ]
  },
  "stats": { "objects": 18231, "bytes": 5497558138880 }
}
```

The index MUST be written only after every segment it lists is stored.

### 8.2 Segments

A segment is a page (§5.1 storage rules), a JSON object `{ "kind": "segment", "table": …, "rows":
[ … ] }`, with rows sorted by the table's key. The `first` and `last` members in the index are
the keys of the first and last rows, encoded as strings in the forms below.

| Table | Key | Row |
|---|---|---|
| `entries` | `parent` then `segment` (§7.6), encoded `"<parent>/<segment>"` | `{ "parent", "name", "oid", "kind" }` |
| `objects` | `oid` | `{ "oid", ...the object record (§7.7), "head": "<version id>" }` |
| `history` | `oid` then version order, encoded `"<oid>@<seq, 20 digits>.<i>"` | `{ "oid", "version", "time", "op", "size", "etag", "content", "restored_from", "actor" }` |
| `removed` | the key the object had when removed, then `oid`, encoded `"<key>@<oid>"` | `{ "key", "oid", "version", "time" }`: one row per object that is out of the namespace and still retained. Deleted when the object is put back or its history expires. Serves the protocol's "recently deleted" listing |

`history` holds every version of every object that is still retained (§10), including versions
inherited from a fork's parent (§9). It carries each version's content descriptor, so a reader
never needs an old commit to read an old version.

### 8.3 Reuse

Segments are content-addressed, so a new checkpoint lists the same page for every segment whose
rows did not change. An authority SHOULD keep segment boundaries stable between checkpoints (for
example by cutting segments where a row's key hash has 12 low zero bits, bounded to 256–8,192
rows) so that a checkpoint rewrites only the segments that changed.

### 8.4 Finding the latest state

1. Read `_last_checkpoint` (a small JSON object `{ "seq": n }`). It is a hint and MAY be stale
   or missing.
2. List `checkpoints/` to find the highest `seq` at or above the hint, and load it.
3. List `log/` starting after that `seq`, and apply every commit in order. A gap in the sequence
   numbers means a commit is still being written or was lost; readers MUST stop at the gap.

An authority SHOULD write a checkpoint at least every 1,000 commits or every 16 MiB of log,
whichever comes first.

### 8.5 Point-in-time window and truncating the log

The state of a whole drive at an arbitrary instant `t` (needed to restore a folder as it was)
is the newest checkpoint at or before `t`, plus the commits after it up to `t`. The **point-in-time
window** is the period for which a deployment keeps enough checkpoints and commits to do that.

- By default, commits and checkpoints are kept for as long as the versions they describe are
  retained, and the window covers the drive's whole retained history. Checkpoints cost little,
  because unchanged segments are shared (§8.3).
- A deployment MAY configure a shorter window. It MAY then delete commits and checkpoints older
  than the window, but it MUST keep the newest checkpoint and every commit after it.
- Readers MUST NOT depend on commits before the newest checkpoint for the current state.
- A fork's window starts at its fork point: its namespace before then is only in its first
  checkpoint (§9).
- Per-object history is unaffected by the window. It lives in the `history` table.

---

## 9. Forks

A fork of drive `P` at sequence `s` is a new drive `F` whose state at `s` equals `P`'s state at
`s`:

1. The authority materializes `P`'s state at `s`, writing new segments only where that state
   differs from `P`'s newest checkpoint at or before `s`.
2. It writes `F`'s checkpoint `checkpoints/<s>.json`, whose segments are mostly `P`'s.
3. It writes `F`'s `drive.json` with `fork_of: { drive_id: P, seq: s }`. The drive exists from
   this write.

- `F`'s log continues from `s + 1`. Version ids at or before `s` are shared with `P` and name the
  same versions.
- After step 3, `F` depends on nothing in `P`'s prefix. `P` MAY be deleted, and a fork MAY be
  forked again without limit.
- Nothing is copied except checkpoint segments that differ, which is none when `P` has a
  checkpoint at exactly `s`.
- A fork into another pool is not copy-on-write. It is a full copy of content and is outside
  this format.
- `s` MUST include every transaction the protocol acknowledged on `P` before the fork was
  requested.

---

## 10. Retention and deletion

- **Versions.** A version is retained while it is within the drive's retention policy. The
  default is to keep every version forever. A retention policy is control-plane configuration;
  its effect on the format is only that a checkpoint MAY omit history rows for versions outside
  it, and those versions stop being GC roots. The head version of every live object is always
  retained.
- **Soft delete of a drive** writes `deleted.json` (`{ "time": …, "actor": … }`). The drive is
  no longer served, but its state stays intact and remains a GC root until it is hard-deleted.
  Removing the marker restores the drive. After the retention window (default 30 days, counted
  from `time`), the authority or the garbage collector MAY hard-delete it.
- **Hard delete** removes everything under `drives/<drive-id>/`, starting with `drive.json`: the
  drive stops existing, and so stops being a GC root, when that object goes. Before deleting it,
  the authority MUST stop starting operations on the drive and let those in progress finish,
  because an operation that references content through the drive relies on the drive being a
  root (§12.4, option 1). Shards and pages that are no longer referenced are then reclaimed by
  GC.

---

## 11. Multipart staging

While a multipart upload is open, each uploaded part is chunked into shards immediately. The
part is recorded at `drives/<drive-id>/uploads/<upload-id>/<part number, 5 digits>.json`:

```json
{ "part": 3, "size": 16777216, "etag": "\"…\"", "extents": [ … ] }
```

and the upload itself at `uploads/<upload-id>/upload.json` (`{ "key", "created", "actor",
"attrs" }`). Completing the upload is one transaction whose content is the concatenation of the
parts' extents. The staging prefix is then deleted.

An upload open longer than 7 days MAY be aborted by the authority. While open, its records are
GC roots.

---

## 12. Garbage collection

A shard or page is **referenced** if any of these reaches it (the **roots**):
- any retained version in any drive of the pool, through its content descriptor and manifest
  tree, including soft-deleted drives until they are hard-deleted (§10);
- any checkpoint index still readable under §8.5, through its segments and their rows;
- any open multipart staging record (§11).

GC deletes shards and pages that are not referenced. Because content is shared across drives
and uploads race with collection, it MUST follow this protocol. The design and the argument for
it are in [RFC 0002](../rfcs/0002-gc-safe-against-writers.md).

### 12.1 The run record: `gc/pending.json`

```json
{
  "format": 1,
  "run": "3a1f0c7e-9b2d-4c61-8e5f-0d7a2b9c4e18",
  "phase": "waiting",
  "t1": "2026-09-27T21:00:00Z",
  "grace": 86400,
  "candidates": ["<sha256>", "…"]
}
```

| Member | Meaning |
|---|---|
| `run` | A UUID that identifies the run |
| `phase` | `marking`, `waiting` or `deleting` |
| `t1` | The Last-Modified time of this object when the run created it, truncated to whole seconds. Absent in `marking` |
| `grace` | The run's grace period in seconds |
| `candidates` | Hashes proposed for deletion. A hash stands for both the shard and the page of that name. Absent in `marking` |

- `grace` MUST be at least 86,400 (24 hours). A collector MAY use a shorter grace, including
  zero, only while no writer is active in the pool (for example, every authority is stopped).
- At most one collector works on a pool at a time. The create-if-absent write in §12.2 stops two
  runs from starting together. The deployment MUST ensure that no two collectors continue the
  same run.
- A run in `waiting` or `deleting` whose collector stopped MAY be continued by the next
  collector. A run left in `marking` MAY be deleted once it is more than `grace` old.

### 12.2 Mark and propose (phase 1)

1. Create `gc/pending.json` as `{ "format": 1, "run": "<uuid>", "phase": "marking", "grace": g }`
   with a create-if-absent write. If it already exists, do not start a run. Read the object's
   Last-Modified time, truncated to whole seconds: that is `t1`, on the bucket's clock.
2. Compute the referenced set across every drive in the pool.
3. List `shards/` and `pages/`. Every object that is not referenced *and* was last modified
   before `t1 - grace` is a candidate.
4. If there are no candidates, delete `gc/pending.json`: the run is over.
5. If more than `grace / 2` has passed since `t1` on the collector's clock, delete
   `gc/pending.json`: the run is abandoned.
6. Otherwise overwrite `gc/pending.json` with `phase: "waiting"`, `t1` and the candidates.

### 12.3 Confirm and delete (phase 2)

1. Once `t1 + grace` has passed on the collector's clock, overwrite `gc/pending.json` with
   `phase: "deleting"`. Read its Last-Modified time. If that is earlier than `t1 + grace`, write
   `phase: "waiting"` back and try again later.
2. Recompute the referenced set. The recompute MUST start after step 1 completed.
3. Observe each candidate's Last-Modified time after step 1 completed (by listing, or with a
   HEAD). Delete each candidate that is still unreferenced **and** was last modified before
   `t1`.
4. Delete `gc/pending.json`.

*(informative)* A conditional delete cannot replace the `deleting` mark. A rewritten shard has
the same bytes, so the same ETag, and few backends offer a condition on the last-modified time.

### 12.4 What writers must do

Before committing, a writer MUST have done one of the following for **every** shard and page
the commit references, whether or not it uploaded the object itself:

1. **Referenced.** It saw the object referenced by a root (a drive's state, a readable
   checkpoint, or an open staging record) as that root stood at time `r`.
2. **Not a candidate, then present.** It read `gc/pending.json` at time `r` and found no run, or
   a run that does not list the object. Then it confirmed the object exists with a request that
   started after `r`: an upload (PUT), a GET or a HEAD.
3. **Rescued.** It read `gc/pending.json` at time `r` and found the object listed by a run in
   phase `waiting`. After `r`, it rewrote the object with its (identical) bytes. It then read
   `gc/pending.json` again and found the same run, still not `deleting`.
   - If that second read finds the run `deleting`, the writer MUST NOT commit. It waits until
     the run has ended and then uses option 2.
   - If it finds another run, or none, the writer starts again.

A writer MUST NOT commit more than 12 hours (half the minimum grace) after the `r` it relies on.

A writer MAY cache `gc/pending.json` if it re-reads it at least once an hour. Then:
- A check under option 2 stays valid while every re-read omits the object, with `r` taken as the
  latest re-read. It lapses as soon as a re-read lists the object, or when an hour passes
  without a successful re-read.
- An object rescued under option 3 stays rescued for as long as the same run is in
  `gc/pending.json`.

*(informative)* An upload of bytes identical to a candidate *is* a rewrite, whether or not the
writer knew. That is why these checks apply to uploads too.

---

## 13. Versioning of this format

- `format` in `voidfs.json`, `drive.json`, commits, checkpoints and pages is the major version,
  currently 1.
- Additive changes, such as new optional members or new compatible features, keep it.
- Changes that an older reader would misread use an incompatible feature flag (§3.1).
- A new major version is reserved for a redesign. A reader for version `N` MUST refuse version
  `N + 1`.

---

## Appendix A: FastCDC gear table

The 256-entry table of 64-bit values used by FastCDC-2020 is taken, unchanged, from the
reference implementation used by the `fastcdc` Rust crate (`v2020` module). It will be
reproduced here, in full, before this spec is marked Stable, so that the format does not depend
on any one implementation.
