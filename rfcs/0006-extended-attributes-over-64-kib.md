# RFC 0006: Extended attributes over 64 KiB

| | |
|---|---|
| Status | Proposed |
| Author(s) | voidfs maintainers |
| Created | 2026-10-15 |
| Affects | both |
| Implemented by | (links, once implemented) |

## Summary

In a pool that lists a new incompatible feature, `large-xattrs`, an object's extended attributes
may total 64 MiB, and one value may be 16 MiB. Values that don't fit in today's 64 KiB are stored
as content: a descriptor of shards in the object's record, as a file's bytes are, rather than
base64 in every commit and history row. The attributes response keeps listing every value it
lists today and names the others with their sizes. A new call reads or writes one value as raw
bytes, with ranges. Pools without the feature keep the 64 KiB limit.

## Motivation

Protocol §4.8 and §7 allow 64 KiB of extended attributes per object, names and values together,
and format §7.7 calls larger values "a future feature". On the Mac that limit costs data:

- **Finder drops resource forks silently.** In the
  [application matrix](../bench/results/mac-app-matrix/README.md#environment-and-matrix)
  (13 October), a 65,537-byte resource fork was dropped by Finder's copy, duplicate and copy-out
  on the FSKit mount, each of which reported success. APFS kept it. The matrix scores this as a
  failed requirement: a silent loss.
- **Other tools refuse them.** `cp` of a file with a 200 KiB fork copies the data, then exits 1
  with "Argument list too long" (`E2BIG`). `xattr -w` of 70,000 bytes fails the same way
  ([item 4's evidence](../bench/results/mac-fskit-writable/raw/resource-fork.txt), 12 October).
- **It keeps item 4 incomplete.** Step 5's writable FSKit item is built and not complete for two
  reasons: `RENAME_SWAP`, which waits on Apple, and this limit, which is voidfs's own
  ([step 5](../docs/step-5-macos.md#item-4-writable-fskit-and-desktop-semantics)).

APFS has no comparable limit. On the test Mac it sets and reads back a resource fork, and an
ordinary attribute, of 1 GiB, and reports `_PC_XATTR_SIZE_BITS` = 56
([measured 15 October](../bench/results/xattr-limits/README.md)). Whatever voidfs chooses, the goal
is to move the limit far enough that a fork has to be unusual before it is refused.

Raising the inline limit alone would be the wrong fix. The format writes an object's whole
attribute set into every commit that changes it and into every history row of a checkpoint.
A 16 MiB fork inline is about 21 MiB of base64 per version: ten changes of a file's mode would
put over 200 MiB into the log, and as much again into checkpoint history. Stored as content, the
same fork is shards written once and deduplicated like any file, and each version's record
carries a descriptor of about a hundred bytes.

SpaceFS can't write a resource fork at all through its SMB mount ("Operation not supported",
[step 4 §1.8](../docs/step-4-client.md#18-the-mount-observed-3-october-02343)). voidfs already
keeps forks up to 64 KiB; this keeps larger ones.

**What this does not fix.** FSKit's `XattrHandler` gives a module no position, so positional
reads of a fork fail after the first chunk, and `copyfile` copies *from* the mount fail for forks
of any size ([the resource-fork failure](../bench/results/mac-app-matrix/README.md#the-resource-fork-failure)).
That needs Apple. A value over the new limit is still refused. The module advertises the limit,
but Finder may drop such a value just as silently as it drops one over 64 KiB today. Whether
raising the limit is enough for Finder's copies onto the mount is not yet measured
([open question 1](#open-questions)).

## Design

### Limits

| Limit | Without `large-xattrs` | With it |
|---|---|---|
| An object's extended attributes, names and values together | 64 KiB | 64 MiB |
| One value | (within the 64 KiB) | 16 MiB |
| Values listed in the attributes response (protocol §4.8) | all of them | the listed values, at most 64 KiB |
| Values stored inline in a record (format §7.7) | all of them | at most 64 KiB, names and values |
| A name | 1 to 255 bytes | unchanged |

16 MiB is the largest shard (format §4). Cut with a default `voidfs.json`'s chunking, such a value
is about eight shards, and with base64 it fits in one 64 MiB request body.

### Format

**§3.1**, a new feature:

| Feature | Kind | Meaning |
|---|---|---|
| `large-xattrs` | incompatible | Extended attribute values may be stored as content (§7.7), and their shards are referenced (§12). [RFC 0006](../rfcs/0006-extended-attributes-over-64-kib.md) |

It is incompatible because an older reader would misread the records, and an older garbage
collector would delete shards that only an attribute references.

**§7.7**, `attrs.xattrs` becomes:

> Extended attributes, name to value. A value is a base64 string, an **inline value**, or, in a
> pool that lists `large-xattrs`, a content descriptor whose extents are all `s` extents, a
> **stored value**. Its bytes are the concatenation of those shards.

Rules for stored values:

- A stored value has at least one extent and at most 1,024, never a manifest tree (`root`), and no
  `z` or `d` extents. Its length, the sum of its extents' `n`, is at most 16 MiB.
- An empty value is always inline (`""`).
- An object's inline names and values total at most 64 KiB. All its names and values, inline or
  stored, total at most 64 MiB.
- A writer SHOULD cut a stored value with the pool's chunking (§4.1), so that identical values
  share shards with each other and with file content.
- Which values a writer stores is its choice within these limits. *(informative)* voidfs-server
  stores exactly the values the protocol does not list (protocol §4.8) and keeps the rest inline.
  An object then needs no shard read to answer the attributes request, and an object whose
  attributes total at most 64 KiB is stored as it is today. When an object crosses 64 KiB in
  either direction, a change can move another value between inline and stored. This costs that
  value's chunking or its shard reads, at most once per change.

A commit that sets a stored value:

```json
{ "set": { "oid": "o-01J8Z…",
           "attrs": { "mode": 420, "mtime": "2026-10-15T14:02:11.000000Z",
                      "xattrs": { "com.apple.FinderInfo": "VEVYVHR0eHQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                                  "com.apple.ResourceFork": { "extents": [ { "s": "9c1f…", "n": 204800 } ] } } } } }
```

Everything else follows from the existing rules for content:

- **§7.4 and §12.4:** a commit references a stored value's shards as it references content
  shards. The same durability order and checks apply before it is written.
- **§8.2:** history rows carry `attrs` as they are. Stored values have no data extents, so a
  checkpoint writer converts nothing.
- **§9 and copies within a pool:** records and history rows are shared, so stored values are too.
  No bytes are copied.
- **Copies into another pool** copy a stored value's bytes as they copy content. In a pool
  without the feature, the copy's attributes are held to 64 KiB like any others.
- **§11:** an upload's `upload.json` MUST NOT hold stored values. CreateMultipartUpload carries
  no extended attributes.
- **§12**, the first root becomes: "any retained version in any drive of the pool, through its
  content descriptor and manifest tree, and through the descriptors of its stored extended
  attribute values, …".

The format stays 1 (§13): the change is guarded by the incompatible feature.

### Protocol

**§4.8, GET.** The response's `xattrs` holds the object's **listed values**. Order its values
by length, then by name compared as bytes. The listed values are the longest prefix of that order
whose names and values total at most 65,536 bytes. Every other value is named in `largeXattrs`,
with its length:

```json
{ "objectId": "o-…", "kind": "file", "mtime": "…", "mode": "0644",
  "xattrs": { "com.apple.FinderInfo": "<base64>" },
  "largeXattrs": { "com.apple.ResourceFork": { "size": 204800 } },
  "flags": [], "contentType": "text/plain", "meta": {} }
```

- An object whose attributes total at most 64 KiB lists every value, and has no `largeXattrs`:
  the response is the one it gets today. That covers every object in a pool without the feature.
- A client reads the values in `largeXattrs` with §4.12.
- `largeXattrs` is an object of objects so that later members, a hash for example, can be added
  without another change of shape.

**§4.8, POST.** `set` accepts values up to the drive's limits (§5.4), in a body of at most 64 MiB
(§7). One POST is one `attrs` version, so a client that must change a large value together with
other attributes sends them in one POST. Otherwise it sends large values with §4.12, without the
base64.

**§4.12 (new), one extended attribute: `GET` and `PUT /{drive}/{key}?x-voidfs-xattr&name=<name>`.**
`name` is the attribute's name, percent-encoded as any query value. Both work for any value, in
every pool. A server that implements them says so with `xattrLimits` (§5.4), and a client sends
them only to a drive whose description has it.

- **GET** → `200 application/octet-stream` with the value's bytes, and the version read in
  `x-amz-version-id`. `Range` (a single range) answers `206` as for GetObject. `versionId` and
  `x-voidfs-as-of` select a past version as in §4.5. A version without that attribute answers
  `404 NoSuchAttribute`; a missing object or version answers as GetObject does.
- **PUT** with the value's bytes as the body, which may be empty, sets that one attribute as one
  `attrs` version, as the POST `{ "xattrs": { "set": { "<name>": "<base64 of the body>" } } }`
  would. The preconditions of §4.0 apply. → `200` with the common headers. `400 InvalidArgument`
  for a name that isn't 1 to 255 bytes, `413 EntityTooLarge` over a limit (§7), `404 NoSuchKey`.
- Removal stays POST's `remove`.

```
PUT /footage/fonts/Chicago?x-voidfs-xattr&name=com.apple.ResourceFork HTTP/1.1
x-voidfs-if-version: 1841.0
Content-Length: 204800

<the fork's bytes>
```

→ `200`, `x-amz-version-id: 1842.0`. A later
`GET /footage/fonts/Chicago?x-voidfs-xattr&name=com.apple.ResourceFork` with `Range: bytes=0-15`
answers `206` with the fork's first 16 bytes.

**§4.9.** `hasXattrs` is true when an object has any value, listed or not. No change in wording.

**§5.4.** The describe response gains the drive's limits:

```json
{ "driveId": "d-…", …, "xattrLimits": { "totalBytes": 67108864, "valueBytes": 16777216 } }
```

- A server that implements §4.12 includes `xattrLimits` in every describe response. In a pool
  without `large-xattrs`, both are 65536.
- Without `xattrLimits`, the limit is 64 KiB of names and values together, as before, and the
  server may not implement §4.12.
- A mount reads it to tell the operating system the largest value it can store.

**§5.5.** A reader with storage credentials MAY read a stored value's shards from the bucket, as
it reads content, when it has the object's record.

**§6**, a new code: `NoSuchAttribute`, `404`, §4.12: the version has no attribute of that name.

**§7**, the rows become:

| Limit | Value |
|---|---|
| Extended attributes per object | 64 KiB; 64 MiB in a drive whose description says so (§5.4) |
| One extended attribute value | 16 MiB, where the drive allows 64 MiB per object |
| Attribute change body (§4.8) | 64 MiB |

The change feed (§5.6) carries no attribute values and is unchanged.

### voidfs-server and voidfs-core *(informative)*

- `Attrs.xattrs` values become inline or stored. The attribute patch applies `set` and `remove`,
  re-evaluates which values are listed, and hands the server any value that must change storage.
- The server chunks a value it must store as it chunks a put, stores the shards with the §12.4
  checks, and commits them in the same transaction as the record that references them. It reads
  a value back inline from its shards.
- Garbage collection's mark walks stored values in records and history rows.
- In a pool without the feature, the server keeps today's 64 KiB check and answers `413`.
- `name` and `x-voidfs-xattr` join the object queries the server knows (`OBJECT_KNOWN`).

### The client and the Mac *(informative)*

- The mount core keeps large values in staging files beside staged file data, not in the SQLite
  memo, which today holds every value as JSON.
- It publishes changed attributes in one POST when they fit in one body, and large values with
  PUT otherwise.
- It reads stored values with §4.12 and caches them by object version and name. FSKit asks a
  module for the whole value even when the caller only wants its size, so the cache matters for
  repeated `getxattr`.
- The core's advertised `maxXattrBytes`, the daemon session's frame limits, the bridge's XPC
  replies and the module's `maximumXattrSize` follow the drive's `valueBytes`.
- On a drive without the feature, everything stays at 64 KiB and answers `E2BIG` as today.

## Compatibility

- **New client, old server.** The describe response has no `xattrLimits`, so the client keeps
  64 KiB and uses §4.8 as it does today. It never sends §4.12 there. voidfs-server would answer
  `501 NotImplemented` (§3), as it does for any object query it doesn't know, but another
  protocol-1 server that ignored the query would take §4.12's PUT for a PutObject and replace
  the file's bytes with the attribute's.
- **Old client, new server.** For every object whose attributes total at most 64 KiB, which
  includes every object an old client could write, responses are unchanged. On a larger object,
  an old client sees only the listed values and doesn't know the others exist. An old mount
  shows that file without its large values. Its POST `set` and `remove` of other names leave them
  in place. A tool that copies attributes by reading `xattrs` would leave the large values behind.
  That is no worse than such a client can do today: it could never hold them.
- **Old reader or garbage collector, pool with the feature.** It refuses the pool (format §3.1).
  This is required, not merely cautious: an old collector would delete shards only an attribute
  references.
- **Enabling it.** As for `inline-data` and `multi-object-versions`, an operator adds the feature
  after every writer of the pool implements it (format §3.1, informative).
- Additive (§8): the protocol stays 1, with a new member, a new call, a new error, and a larger
  limit in drives that say so. The format stays 1, under the new feature.

## Conformance

Two cases.

`xattr-one-value`, in every pool:

1. GET `?x-voidfs-drive`: `$.xattrLimits.totalBytes` is present.
2. PUT `x/f` with text `one`; capture its ETag.
3. PUT `?x-voidfs-xattr&name=user.tag` with base64 `aGk=`: `200`.
4. GET `?x-voidfs-attrs`: `$.xattrs['user.tag']` equals `aGk=`, `$.largeXattrs` absent.
5. GET `?x-voidfs-xattr&name=user.tag`: body `aGk=` (base64).
6. The same GET with `Range: bytes=1-1`: `206`, body size 1.
7. GET `?x-voidfs-xattr&name=user.none`: `404`, `NoSuchAttribute`.
8. PUT `?x-voidfs-xattr&name=user.empty` with an empty body; GET it: `200`, size 0.
9. HEAD `x/f`: the ETag is unchanged.
10. PUT `?x-voidfs-xattr&name=user.tag` on `x/missing`: `404`, `NoSuchKey`.

`large-xattrs`, requires `feature:large-xattrs`:

1. GET `?x-voidfs-drive`: `$.xattrLimits.valueBytes` equals 16777216 and `totalBytes` 67108864.
2. PUT `x/f` with text `fork`; PUT `?x-voidfs-xattr&name=user.small` with base64 `aGk=`.
3. PUT `?x-voidfs-xattr&name=com.apple.ResourceFork` with bytes `{ "seed": 6006, "size": 204800 }`;
   capture `x-amz-version-id` as `v1`.
4. GET `?x-voidfs-attrs`: `user.small` listed; `$.xattrs['com.apple.ResourceFork']` absent;
   `$.largeXattrs['com.apple.ResourceFork'].size` equals 204800.
5. GET `?x-voidfs-xattr&name=com.apple.ResourceFork`: body is the 204,800 bytes.
6. GET `?x-voidfs-list&prefix=x/`: `$.entries[0].hasXattrs` is true.
7. POST `?x-voidfs-attrs` removing `com.apple.ResourceFork`; GET `?x-voidfs-attrs`: no
   `largeXattrs`.
8. GET `?x-voidfs-xattr&name=com.apple.ResourceFork&versionId=${v1}`: the 204,800 bytes again.
9. PUT `?x-voidfs-xattr&name=user.huge` with bytes of size 16777217: `413`, `EntityTooLarge`.
10. PUTs of 16,776,192-byte values as `user.v1` to `user.v4`: each `200`. With `user.small`, the
    names and values total 67,104,808 bytes.
11. The same PUT as `user.v5`: `413`, since they would total over 64 MiB.

## Alternatives

- **Raise the inline limit.** The smallest change, but every version carries every value in its
  commit and history row, as base64, without deduplication (above).
- **A sidecar object**, such as an AppleDouble `._` file or a hidden `.voidfs/` tree, written
  by the client. No format change, but other clients see stray objects, renames and deletes must
  move them, and item 4 requires that no `._` files appear.
- **POST only, without §4.12.** Values would always travel as base64 in JSON, at a third more
  bytes, with no ranges. The conformance format can send generated bytes only as a raw body. POST
  still accepts large values, for changes that must land as one version.
- **Direct uploads for stored values** (protocol §4.11's plan and commit): the server would carry
  no bytes, but it adds a second upload path for at most 16 MiB a value. It can follow if
  measured.
- **No value limit, as APFS.** The module receives and returns whole values in memory (FSKit has
  no positional handler), as do the bridge and the daemon session. 16 MiB bounds that. A larger
  limit can follow with evidence.
- **Do nothing.** Finder keeps dropping forks over 64 KiB silently, and item 4 stays incomplete.

## Open questions

1. **Does FSKit hand a module values over 64 KiB whole, and does Finder then keep them?** The
   matrix shows only that Finder drops a fork the module refuses. Before this is accepted, a test
   build should check it. Its module would advertise 16 MiB, and its daemon would accept that
   much in memory, without a protocol change. With module tracing on, it would copy files with
   65,537-byte, 1 MiB and 16 MiB forks onto and within the FSKit mount, with Finder and with `cp`,
   and record the size of each `setXattr` and `getXattr`. If the kernel splits or refuses such
   values, this RFC still helps other clients, but the Finder failure stays. The check needs the
   development module and agent registered on the test Mac, and restored afterwards, as the matrix
   did.
2. **The limits.** Is 16 MiB a value and 64 MiB an object right, or should they be larger? APFS
   accepts 1 GiB. Larger limits cost memory along the module, bridge and daemon path, and
   longer single requests.
