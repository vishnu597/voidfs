# voidfs wire protocol

**Status:** Draft 1 · **Protocol version:** 1 · Conventions: [README](README.md#conventions)

A voidfs server exposes drives through the Amazon S3 API: a drive is a bucket and a file is an
object. Stock S3 clients work unchanged. The `x-voidfs-*` extensions add what S3 cannot express:
writes at an offset, byte insertion and removal, renames, history and rollback, forks, and the
calls a filesystem mount needs.

How the data is stored is defined separately, in [format.md](format.md).

---

## 1. Model

| S3 term | voidfs meaning |
|---|---|
| Bucket | A **drive**. The bucket name is the drive's alias (S3 bucket naming rules, unique per deployment) or its id (`d-<uuid>`). |
| Key | A path in the drive: `/`-separated, no leading `/`, at most 1,024 bytes of UTF-8. Each segment follows the name rules in [format §7.6](format.md#76-names-and-paths). |
| Object | A file, a folder (a key ending in `/`) or a symbolic link. |
| Version | An immutable snapshot of an object's content and metadata. |

### 1.1 Versions

- Every successful mutation creates exactly one version and returns its id in
  `x-amz-version-id`. There is no unversioned mode.
- A version can belong to several objects, with the same id in each one's history: a folder
  restore's version is the folder's and that of every object the restore changes (§4.6); a write
  that creates folders is their first version (§1.2); a rename that replaces a file is that
  file's deletion (§4.7). A server whose drives don't keep such versions (voidfs-server on a pool
  without the `multi-object-versions` feature, format §3.1) gives a version to the object at the
  request's key alone.
- History is append-only. Nothing, including a rollback, removes a version. Retention policies
  configured by an administrator are the only exception.
- Version ids are opaque strings of at most 128 ASCII characters. Clients MUST NOT parse them.
- A rename, and a change to attributes only, are versions too.

### 1.2 Folders

Folders are real objects, not key prefixes.

- `PutObject` of a key ending in `/` with an empty body creates a folder.
- Writing `a/b/c` creates the folders `a/` and `a/b/` if they are missing, in the same version,
  which is each one's first (§1.1).
- A file and a folder cannot share a path. `PutObject a` while `a/` exists, or `PutObject a/x`
  while the file `a` exists, fails with `409 PathConflict`.
- `ListObjectsV2` without a delimiter returns files, plus folders only while they are empty.
  With `delimiter=/`, empty and non-empty folders alike appear as `CommonPrefixes`.
- Deleting a file leaves its parent folders in place. Deleting a folder key succeeds and does
  nothing while the folder still has children.

### 1.3 ETag

`ETag` is a strong, quoted, opaque validator that changes whenever an object's content changes,
and does not change on rename or attribute changes. It is **not** an MD5 of the content. Clients
MUST NOT compare it with a locally computed MD5.

### 1.4 Consistency

Read-after-write: once a mutation has succeeded, every later request to the same deployment
observes that version or a newer one (GET, HEAD, list, history). Writes made through mounts are
ordinary mutations and follow the same rule.

A deployment that serves one drive from several regions is out of scope for protocol 1. The
header `x-voidfs-consistent` is reserved for it.

### 1.5 Concurrency

Writers on different keys never affect each other. Writers on the same key get S3 semantics:
the last one wins, both succeed, and both versions are in the history. A writer that must not
lose another's update sends a precondition (§4.0). Of two racing guarded writers, exactly one
succeeds.

---

## 2. Endpoint and authentication

- **Endpoint:** set by the deployment, for example `https://voidfs.example.com`. Any region
  string is accepted in the signing scope and ignored. `auto` and `us-east-1` both work.
- **Addressing:** path-style (`/drive/key`) MUST be supported. Virtual-host style
  (`drive.voidfs.example.com/key`) SHOULD be supported when the deployment has a wildcard name.
- **Signing:** AWS Signature Version 4, service `s3`. Servers MUST support:
  - header authentication and presigned query authentication;
  - `UNSIGNED-PAYLOAD`;
  - `aws-chunked` bodies in the forms `STREAMING-UNSIGNED-PAYLOAD-TRAILER`,
    `STREAMING-AWS4-HMAC-SHA256-PAYLOAD` and `STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER`;
  - trailing checksums `x-amz-checksum-{crc32,crc32c,crc64nvme,sha1,sha256}`, which they MUST
    verify.

  A mismatch is `403 SignatureDoesNotMatch` or `400 BadDigest`, and nothing is written.
  A checksum given as a request header (`x-amz-checksum-*`) is verified the same way.
- Signature Version 2 (`Authorization: AWS …`, or `AWSAccessKeyId` and `Signature` in the query)
  is not supported and answers `400 InvalidRequest`. Some clients use it for presigned URLs
  unless configured otherwise (boto3: `signature_version="s3v4"`).
- **Extension headers** (`x-voidfs-*`) MUST be included in `SignedHeaders`. A server MUST reject
  a request that carries an unsigned `x-voidfs-*` header with `400 InvalidArgument`.
- **Host:** `host` MUST be included in `SignedHeaders`, as Signature Version 4 requires, in
  either addressing style: in virtual-host style it names the drive. A server MUST reject a
  signature that leaves it out with `403 AccessDenied`, as S3 does.
- **Payload hash:** a request signed in the `Authorization` header MUST send
  `x-amz-content-sha256` (a hash, `UNSIGNED-PAYLOAD` or an `aws-chunked` form). A server MUST
  reject one without it with `400 InvalidRequest`, as S3 does. A presigned URL carries none: its
  payload is unsigned.
- **Access keys:** the id is `VF` followed by 18 characters from `A–Z2–7` (20 in total). The
  secret is 40 characters. Keys are issued by the deployment's control plane.

| Scope | Allows |
|---|---|
| `read` | GET, HEAD, listings, history, change feed, read-only storage credentials |
| `write` | `read`, plus every object mutation |
| `admin` | `write`, plus creating, forking and deleting drives |

A key MAY be restricted to a list of drives. A drive outside that list does not exist for the
key: requests for it answer `404 NoSuchBucket`, and `ListBuckets` leaves it out.

---

## 3. Standard S3 subset

| Operation | Notes |
|---|---|
| ListBuckets | Drives visible to the key |
| HeadBucket, CreateBucket, DeleteBucket | Drive lifecycle, §5. DeleteBucket soft-deletes and does not require the drive to be empty |
| GetBucketLocation | An empty `LocationConstraint` |
| GetBucketVersioning | Always `Enabled`. PutBucketVersioning answers `501 NotImplemented` |
| ListObjectsV2, ListObjects | `prefix`, `delimiter` (only `/`), `max-keys`, `continuation-token`, `start-after`, `marker`, `encoding-type=url` |
| ListObjectVersions | Full history for matching keys; `IsLatest` marks the head. No delete markers are returned; deleted objects are absent (see §4.10 for them) |
| HeadObject, GetObject | `Range` (a single range), `versionId`, `If-Match`, `If-None-Match`, `If-Modified-Since`, `If-Unmodified-Since`. `partNumber` is not supported |
| PutObject | `If-Match`, and `If-None-Match: *` |
| CopyObject | Within a drive: by reference, with no bytes copied. Across drives: copies bytes, unless both drives are in the same pool, in which case it is also by reference. A `versionId` in `x-amz-copy-source` is a version of the object at the source key, as for GetObject (§4.5) |
| DeleteObject, DeleteObjects | With `versionId`: `501 NotImplemented`, because history is immutable |
| CreateMultipartUpload, UploadPart, UploadPartCopy, CompleteMultipartUpload, AbortMultipartUpload, ListParts, ListMultipartUploads | A completed upload is one version |

Anything else answers `501 NotImplemented` with the standard S3 XML error body.

---

## 4. Object extensions

An extension request is identified by a query parameter beginning `x-voidfs-`.

### 4.0 Common headers

| Request header | Meaning |
|---|---|
| `x-voidfs-if-version: <id>` | The object's current version MUST be `<id>`, or the request fails with `412 PreconditionFailed` |
| `If-Match: <etag>` | The object's current ETag MUST match, or `412` |
| `x-voidfs-mtime: <timestamp>` | Sets the modification time recorded with the new version (mounts use this). Default: the commit time |
| `x-voidfs-mode: <octal>` | Sets POSIX permission bits, for example `0755` |

The `x-voidfs-mtime` and `x-voidfs-mode` headers are also accepted on PutObject,
CopyObject, CreateMultipartUpload and the rename extension (§4.7).

Every successful mutation responds with `x-amz-version-id`, `ETag` and `x-voidfs-size` (the
object's size afterwards). A `412` carries the current version's id in `x-amz-version-id` when
the object exists.

HEAD and GET additionally return `x-voidfs-object-id`, `x-voidfs-kind` (`file`, `folder` or
`symlink`), `x-voidfs-mtime` and `x-voidfs-mode`.

### 4.1 Write at an offset: `PUT /{drive}/{key}?x-voidfs-write`

| Header | Required | Meaning |
|---|---|---|
| `x-voidfs-offset` | yes | Decimal byte offset at which the body is written |
| `x-voidfs-size` | no | The object's size after the write. Larger than the current size extends it with zeros; smaller truncates it. Default: `max(current size, offset + body length)` |
| `Content-Length` | yes | At most 64 MiB |

The effect equals `pwrite(fd, body, offset)` followed by `ftruncate(fd, size)` if a size is
given, applied atomically as one version. If the object does not exist it is created with zeros
before `offset`, unless a precondition is present, in which case the answer is `412`.

**Truncate or extend:** an empty body, `x-voidfs-offset: 0` and `x-voidfs-size`.

`x-amz-meta-*` headers set user-metadata entries on the new version: each names an entry, which
takes the header's value. Entries no header names keep their values from the version before.
Without such headers the user metadata is unchanged. When a write creates the object, its user
metadata is the headers' entries.

Errors: `400 InvalidArgument`, `409 PathConflict`, `412 PreconditionFailed`,
`413 EntityTooLarge`.

### 4.2 Batched edits: `POST /{drive}/{key}?x-voidfs-patch`

Applies many writes as one version. `Content-Type: application/vnd.voidfs.patch`. An optional
`x-voidfs-size` works as in §4.1; the default is `max(current size, max(offset + length))`.

```
magic    4 bytes  "VFSP"
version  u8       1
reserved 3 bytes  zero
count    u32      number of edits, 1 to 10,000
repeat count times:
  offset u64
  length u64
  bytes  [length]
```

- Edits are applied in order, and later edits win where they overlap.
- The whole body is at most 64 MiB.
- A body that is truncated, has trailing bytes, has a wrong magic or version, or non-zero
  reserved bytes fails with `400 InvalidPatch` and changes nothing.
- `x-amz-meta-*` headers work as in §4.1.

### 4.3 Splice (insert or remove bytes): `PUT /{drive}/{key}?x-voidfs-splice`

| Header | Required | Meaning |
|---|---|---|
| `x-voidfs-offset` | yes | Decimal offset of the splice point |
| `x-voidfs-remove` | no | Bytes removed at the offset before the body is inserted. Default 0 |
| `Content-Length` | yes | The bytes inserted, at most 64 MiB. May be 0 |

The object becomes `object[..offset] ++ body ++ object[offset + remove..]`, as one version. The
object MUST exist (`404 NoSuchKey`), and `offset + remove` MUST NOT be past its end
(`400 InvalidArgument`). A splice that neither inserts nor removes is `400 InvalidArgument`.
`x-amz-meta-*` headers work as in §4.1.

A client MUST NOT retry an unguarded splice after an ambiguous failure, such as a timeout. The
first attempt may have succeeded, and a second would insert or remove twice. Clients SHOULD send
`x-voidfs-if-version` on splices.

### 4.4 History: `GET /{drive}/{key}?x-voidfs-versions`

→ `200 application/json`:

```json
{
  "key": "cuts/a.mov",
  "versions": [
    { "versionId": "…", "isLatest": false, "size": 67108864, "etag": "\"…\"",
      "lastModified": "2026-09-21T09:00:00.000000Z", "operation": "put" },
    { "versionId": "…", "isLatest": true, "size": 67108864, "etag": "\"…\"",
      "lastModified": "2026-09-21T09:05:00.000000Z", "operation": "restore",
      "restoredFrom": "…" }
  ],
  "nextContinuationToken": null
}
```

- Versions are ordered oldest to newest.
- `operation` is one of `put`, `write`, `copy`, `restore`, `rename`, `attrs`, `delete` or
  `other`. Rename, attrs and delete versions are included only with `x-voidfs-all=true`, because
  by default the listing follows content. A delete version is in the history of an object that
  was deleted and brought back.
- `isLatest` marks the newest version listed. With `x-voidfs-all=true` that is the object's
  current version, whose id a plain `GET` returns.
- History follows the object: after a rename, the whole history answers at the new key.
- `max-keys` (default 1,000) and `continuation-token` paginate.
- `lastModified` carries microseconds. Pass it unchanged to `x-voidfs-as-of`.

### 4.5 Read the past

- `GET`/`HEAD /{drive}/{key}?versionId=<id>`: that version of the object now at `{key}`, or, if
  it has none, of an object deleted from `{key}` (§4.10), the most recently deleted first.
  Otherwise the answer is `404 NoSuchVersion`.
- `GET`/`HEAD /{drive}/{key}` with `x-voidfs-as-of: <timestamp>`: the version that was current
  at that instant, for the object now at `{key}`. If that object did not exist then, the answer
  is `404 NoSuchVersion`.
- `versionId` and `x-voidfs-as-of` together are `400 InvalidArgument`.
- A response for anything but the current version carries the served version in
  `x-amz-version-id`.

### 4.6 Roll back: `POST /{drive}/{key}?x-voidfs-restore`

With `versionId=<id>`, creates a new current version whose content and attributes equal version
`<id>`. No bytes are copied. It honors preconditions. The response `x-amz-version-id` is the
**new** version, and `x-voidfs-restored-from` echoes `<id>`. An unknown id is
`404 NoSuchVersion`.

With a folder key and `x-voidfs-as-of: <timestamp>` instead of `versionId`, restores the whole
subtree to its state at that instant, as one version. It is the folder's version and that of
every object the restore changes (§1.1):
- files changed since then are restored;
- files created since then are removed;
- files deleted since then are brought back.

A file it restores or brings back lists the version with `operation: restore` and `restoredFrom`
naming the file's own version that was current at the instant, and `GET ?versionId=<id>` at the
file serves it as restored. A file it removes is listed by §4.10, with the version before the
restore as its `lastVersionId`. Files already as they were keep their current version.

A server MAY answer `400 InvalidArgument` for an instant outside the drive's point-in-time
window (format §8.5).

With a file key and `x-voidfs-as-of`, restores the version that was current at that instant,
exactly as if its `versionId` had been given.

### 4.7 Rename or move: `PUT /{drive}/{new-key}?x-voidfs-rename`

| Header | Required | Meaning |
|---|---|---|
| `x-voidfs-source` | yes | The current key, percent-encoded UTF-8, in the same drive |
| `x-voidfs-replace` | no | `true` lets a file replace an existing file at the destination (POSIX `rename` semantics). Default `false` |

- Moves the object as one metadata-only version. Content, history and ETag are unchanged.
- Both keys MUST be files, or both folders (ending in `/`). A folder moves with its whole
  subtree, in constant time.
- Missing parent folders of the destination are created.
- A folder cannot move into itself.
- Without `x-voidfs-replace: true`, the destination MUST NOT exist (`409 PathConflict`). With
  it, an existing destination file is removed in the same version. That is the atomic save that
  desktop applications perform, where a temporary file is written and renamed over the
  original. The removed file is listed by §4.10 with its own last version as `lastVersionId`.
- Preconditions apply to the source.

Errors: `404 NoSuchKey`, `409 PathConflict`, `412 PreconditionFailed`, `400 InvalidArgument`.

### 4.8 Attributes: `GET` and `POST /{drive}/{key}?x-voidfs-attrs`

GET → `200 application/json`:

```json
{ "objectId": "o-…", "kind": "file", "mtime": "…", "mode": "0644",
  "xattrs": { "com.apple.FinderInfo": "<base64>" }, "flags": ["hidden"],
  "contentType": "text/plain", "meta": {} }
```

POST with a JSON body changes attributes as one `attrs` version:

```json
{ "mtime": "…", "mode": "0600", "xattrs": { "set": { "user.tag": "<base64>" }, "remove": ["com.apple.quarantine"] },
  "flags": ["hidden"] }
```

Members that are left out are unchanged. Extended attributes total at most 64 KiB per object
(`413 EntityTooLarge`). Preconditions apply.

### 4.9 Listing with attributes: `GET /{drive}?x-voidfs-list`

One folder's children, with everything a filesystem needs to answer `readdir` and `getattr` in a
single request.

| Query | Meaning |
|---|---|
| `prefix` | The folder's key, ending in `/`, or empty for the root |
| `max-keys`, `continuation-token` | Pagination. Default and maximum 1,000 |

→ `200 application/json`:

```json
{
  "prefix": "cuts/", "seq": 1842,
  "entries": [
    { "name": "a.mov", "kind": "file", "objectId": "o-…", "versionId": "…", "size": 67108864,
      "etag": "\"…\"", "mtime": "…", "mode": "0644", "hasXattrs": true },
    { "name": "old/", "kind": "folder", "objectId": "o-…", "mtime": "…", "mode": "0755" }
  ],
  "nextContinuationToken": null
}
```

`seq` is the drive position the listing reflects. A client resumes the change feed (§5.6) from
it.

### 4.10 Recently deleted: `GET /{drive}?x-voidfs-deleted`

Lists objects that were deleted and are still retained. `prefix`, `max-keys` and
`continuation-token` work as in ListObjectsV2.

```json
{ "deleted": [ { "key": "cuts/a.mov", "objectId": "o-…", "deletedAt": "…",
                 "lastVersionId": "…", "size": 67108864 } ], "nextContinuationToken": null }
```

- `lastVersionId` is the object's last version *before* it was deleted.
- To bring one back, use §4.6 with `versionId=<lastVersionId>` at the key it should return to.
  This puts the same object back, and its history continues.
- A deleted folder is listed once, and restoring it brings back its subtree as it was when the
  folder was deleted.

### 4.11 Direct upload (optional): `?x-voidfs-upload-plan`, then `?x-voidfs-upload-commit`

For content the drive's pool mostly holds already, the client sends only the missing shards,
straight to the bucket. A server MAY omit this extension. A client MUST fall back to an ordinary
`PutObject` if either step fails with anything but `409` or `412`.

**Plan:** `POST /{drive}/{key}?x-voidfs-upload-plan`, with a JSON body
`{ "shards": [ { "hash": "<sha256>", "length": n }, … ] }` (at most 4,096). The shards are the
content cut as [format §4.1](format.md#41-chunking) specifies.

→ `200`:

```json
{ "token": "…", "expiresSeconds": 900, "held": 12,
  "upload": [ { "hash": "…", "length": 2097152, "url": "https://…", "headers": { "x-amz-checksum-sha256": "…", "If-None-Match": "*" } } ] }
```

**Upload:** `PUT` each listed shard's exact bytes to its `url` with exactly the listed
`headers`, in any order and in parallel.

The server MUST issue upload URLs that bind the shard's SHA-256 checksum. It MUST also bind
create-if-absent where the backend supports it. The storage then rejects bytes that do not match
the name (see §9).

**Commit:** `PUT /{drive}/{key}?x-voidfs-upload-commit`, with a JSON body
`{ "token": "…", "size": n, "contentSha256": "…", "shards": [ … ] }`. The `shards` are the plan's
list in content order, covering exactly `size` bytes.

- Preconditions (`x-voidfs-if-version`, `If-Match`, `If-None-Match: *`) apply as for PutObject.
- `x-voidfs-content-type` carries the content type.
- `x-amz-meta-*` headers apply as for PutObject.
- The server MUST confirm that every shard it did not already reference is present with the
  right length before committing ([format §12.4](format.md#124-what-writers-must-do)).

→ `200` with the common headers. Errors: `400 InvalidArgument` (malformed body, wrong or expired
token, missing shard), `409 PathConflict`, `412 PreconditionFailed`.

---

## 5. Drive extensions

### 5.1 Create: `PUT /{drive}` (CreateBucket)

Creates a drive whose alias is `{drive}`.

| Header | Meaning |
|---|---|
| `x-voidfs-display-name` | Human name (percent-encoded UTF-8). Default: the alias |
| `x-voidfs-pool` | The pool to create it in, when the deployment has several. Default: the deployment's default pool |

Response header: `x-voidfs-drive-id`. `409 BucketAlreadyOwnedByYou` if the alias exists. The
header `x-voidfs-placement` is reserved.

### 5.2 Fork: `PUT /{drive}` with `x-voidfs-fork-source: <drive>`

Creates `{drive}` as a copy-on-write fork of the source's current state. It is ready when the
call returns, and no content is copied. Afterwards the two drives are independent. History up
to the fork point is shared and visible in both.

- The fork holds every write the source acknowledged before the fork was requested.
- Response headers: `x-voidfs-drive-id`, `x-voidfs-fork-source-id`, `x-voidfs-fork-point`
  (timestamp).
- Errors: `404 NoSuchBucket` (source), `409 BucketAlreadyOwnedByYou`, and `400 ForkUnsupported`
  when the source is in another pool.
- With `x-voidfs-fork-mode: copy`, a fork across pools is accepted as an asynchronous full
  copy: `202` with `x-voidfs-operation-id`.
- There is no limit on fork depth. A source MAY be deleted while its forks live.

### 5.3 Delete: `DELETE /{drive}`

- Soft-deletes the drive. It can be recovered for the retention window (default 30 days).
- With `x-voidfs-hard-delete: true`, deletes it permanently. Content still referenced by other
  drives in the pool is kept for them.
- `POST /{drive}?x-voidfs-undelete` recovers a soft-deleted drive.

### 5.4 Describe: `GET /{drive}?x-voidfs-drive`

→ `200 application/json`:

```json
{ "driveId": "d-…", "alias": "footage", "displayName": "Footage", "createdAt": "…",
  "forkOf": { "driveId": "d-…", "forkPoint": "…" }, "forks": ["d-…"],
  "seq": 1842, "usageBytes": 123, "uniqueBytes": 45 }
```

- `usageBytes` is the logical size of the current state.
- `uniqueBytes` is the stored content no other drive in the pool references. It MAY be omitted
  when it is expensive to compute.

### 5.5 Storage credentials: `GET /{drive}?x-voidfs-credentials`

Exchanges the access key for short-lived, **read-only** credentials to the drive's storage. A
mount or bulk reader can then fetch shards and metadata straight from the bucket, and the
server carries no content bytes.

```json
{ "driveId": "d-…", "accessGeneration": 3,
  "storage": { "backend": "s3", "bucket": "…", "root": "voidfs/", "region": "…", "endpoint": "https://…",
               "forcePathStyle": false,
               "readable": ["voidfs.json", "shards/", "pages/", "drives/d-…/"],
               "credentials": { "accessKeyId": "…", "secretAccessKey": "…", "sessionToken": "…",
                                "expiresAt": "…" } },
  "storageBudget": { "limitBytes": 10995116277760, "usedBytes": 1048576 } }
```

- The credentials can read only the listed paths under `root`, and list the drive's prefix. An
  entry ending in `/` is a prefix. `voidfs.json` is there because a reader checks the pool's
  format and features before anything else ([format §3.1](format.md#31-feature-flags)). Writes
  always go through the API or direct-upload URLs (§4.11).
- They expire at `expiresAt`, typically within an hour. A long-running client asks again before
  then. Revoking the access key stops the next exchange; it does not revoke credentials already
  issued.
- `accessGeneration` changes whenever the drive's access rules change. Clients key any cache
  derived from these credentials on it.
- Errors: `404 NoSuchBucket`, and `501 NotImplemented` when the backend cannot issue scoped
  credentials. Clients then read through the API.

### 5.6 Change feed: `GET /{drive}?x-voidfs-changes&since=<seq>`

Reports every change after drive position `since`. Mounts use it to show other machines' saves
within seconds.

With `Accept: text/event-stream`, the answer is a Server-Sent Events stream. Each event's `id`
is a `seq`, and a reconnecting client sends `Last-Event-ID`.

```
id: 1843
data: {"seq":1843,"time":"…","changes":[{"op":"write","key":"cuts/a.mov","objectId":"o-…","versionId":"…","kind":"file"}]}
```

Otherwise the answer is a JSON long poll. `x-voidfs-wait: <seconds>` (0–60, default 0) waits
for at least one change. The body is
`{ "seq": <last seq included>, "changes": [ … ], "more": false }`.

- There is one change per key a version affected, in order.
- `op` is the version's operation (§4.4), or `delete`.
- Folders created implicitly by a write are reported first, as `create` changes with the same
  `seq`.
- Objects the version removed besides the one named, such as the file a rename replaces or the
  files a folder restore removes, come next, as `delete` changes at their old keys, children
  before their folders.
- Then the change of the object the request named, then one for each other object the version
  changed, folders before their children: for a folder restore, each object it restores or
  brings back, with `op: restore`.
- A change that moved an object (a rename, or a folder restore that moves one back) carries
  `fromKey`.
- A folder rename is **one** change for the folder, not one per descendant.
- If `since` is older than the server can replay, the answer is `410 ChangesExpired`. The client
  then relists (§4.9) and resumes from the `seq` that listing returns.

---

## 6. Errors

Errors use the standard S3 XML body: `<Error><Code/><Message/><RequestId/></Error>`.
voidfs-specific codes:

| Code | HTTP | When |
|---|---|---|
| `InvalidPatch` | 400 | Malformed §4.2 body |
| `ForkUnsupported` | 400 | §5.2, source in another pool |
| `PathConflict` | 409 | A file and a folder would share a path, or a rename destination exists |
| `PreconditionFailed` | 412 | `x-voidfs-if-version` or `If-Match` mismatch. Carries the current `x-amz-version-id` when the object exists |
| `ChangesExpired` | 410 | §5.6 |
| `NotImplemented` | 501 | Anything outside §3–§5 |

Retry guidance *(informative)*:
- `500`, `503` and network errors are retryable for idempotent requests (GET, HEAD, PUT of a
  whole object, and any request carrying a precondition).
- Unguarded splices (§4.3) are not retryable.
- On `412`, re-read from the version the error names, then retry.

---

## 7. Limits

| Limit | Value |
|---|---|
| Extension request body (§4.1–§4.3) | 64 MiB |
| Edits per patch | 10,000 |
| Single PutObject | 5 GiB |
| Multipart object | 5 TiB, 10,000 parts |
| Key length | 1,024 bytes of UTF-8 |
| Name segment | 255 bytes |
| Extended attributes per object | 64 KiB |
| Direct-upload plan | 4,096 shards |

Anything over a limit answers `413 EntityTooLarge` or `400 InvalidArgument`. Nothing is silently
truncated.

---

## 8. Versioning of this protocol

- Servers send `x-voidfs-protocol: 1` on every response.
- Additive changes keep the number: new optional headers, new operations, new JSON members.
  Clients MUST ignore JSON members they do not recognize.
- A change that breaks existing clients requires a new number. A server then supports the old
  number for at least 12 months after the new one is released. Clients select a version with
  `x-voidfs-protocol` on requests; without it, a server uses the oldest version it supports.

---

## 9. Security considerations *(informative)*

- **Pools are the isolation boundary for content.** Read-only storage credentials (§5.5) cover
  a pool's shared `shards/` and `pages/` prefixes. A key limited to one drive can therefore
  fetch any shard in the same pool whose SHA-256 it knows. Knowing a shard's hash generally
  means already knowing its content, but deployments that need strict separation between
  tenants SHOULD give each tenant its own pool (at the cost of copy-on-write forks between
  them).
- **Clients never write shared prefixes with credentials.** Direct uploads use URLs that bind
  the shard's checksum (§4.11), so a client cannot store bytes under a hash they do not match.
  Without that binding, a client could corrupt every drive that shares the shard.
- **Direct uploads show what a pool holds.** A plan's `held` count (§4.11) tells a writer whether
  content exists anywhere in the pool, and a commit can reference a shard by its hash alone. As
  with credentials, the pool is the boundary.
- **Bucket credentials** held by the deployment reach every drive in a pool. Deployments SHOULD
  prefer role assumption (for example AWS IAM roles) over stored static keys, and SHOULD encrypt
  stored credentials at rest.

---

## Appendix A: relation to Space's s3sdk protocol *(informative)*

voidfs follows the same model as the s3sdk protocol v1 published by Space Computer, Inc.
(docs.spacefs.com): the operations, the version-per-mutation rule, preconditions, history,
rollback, forks and credential exchange. That keeps a compatibility layer cheap if one is ever
wanted. It differs in:

- Extensions use `x-voidfs-*` rather than `x-s3sdk-*`, and the patch magic is `VFSP` rather
  than `S3SP`.
- **Added:** `x-voidfs-replace` on rename, attributes (§4.8), listing with attributes (§4.9),
  recently deleted (§4.10), subtree restore over the protocol (§4.6), undelete of drives, and
  the change feed (§5.6).
- **Forks:** no depth limit, and a parent may be deleted while its forks live.
- **Storage credentials** are always read-only.
- **Not in protocol 1:** placement and multi-region consistent reads (the headers are reserved).
