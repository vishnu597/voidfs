# RFC 0005: User metadata on edits

| | |
|---|---|
| Status | Implemented (accepted 2026-10-08) |
| Author(s) | voidfs maintainers |
| Created | 2026-10-08 |
| Affects | protocol |
| Implemented by | [`spec/protocol.md`](../spec/protocol.md) §4.1–§4.3 (draft 1, revision 9), the conformance case `user-metadata-on-edits`, voidfs-core, voidfs-server, voidfs-sdk and voidfs-client |

## Summary

The edit extensions, write at an offset (§4.1), batched edits (§4.2) and splice (§4.3), accept
`x-amz-meta-*` headers, as PutObject and a direct upload's commit (§4.11) already do. Each header
sets that user-metadata entry on the new version; entries it doesn't name keep their values from
the version before, as all of them do today. A client can then mark an edit as its own, and
recognize after a lost reply that the edit landed instead of reporting a conflict.

## Motivation

A mount publishes each local save guarded by the version it was based on
(`x-voidfs-if-version`, §4.0), so it never overwrites another Mac's save. When the reply to a
guarded request is lost (a timeout, a dropped connection, the client killed), the client can't
tell whether the request landed. It retries, and if the first attempt had landed the retry fails
with `412`: the guard is now the edit's own result.

For a put, voidfs's client tells these apart already. Every put carries
`x-amz-meta-voidfs-entry: <state id>.<entry id>`, naming the journal entry it publishes, and the
version right after the guard in the object's full history (§4.4, `x-voidfs-all=true`) must carry
that marker. A guarded put lands right after its guard, so the first version with the marker can
only be the put's own; later versions may inherit it, but they aren't right after the guard.

An edit can't carry the marker. A write or a patch keeps the user metadata of the version before
it, so the client's retry of a landed patch gets `412`, and the version right after its guard
carries an older entry's marker or none. Many different edits can follow the same guard, so
nothing else in the history says which edit landed. Today that becomes a **false conflict**: no
data is lost, but the user is asked to resolve a conflict between their own save and itself.

SpaceFS doesn't recognize its own writes either. Its documentation (docs.spacefs.com, read
2026-10-08) says unguarded offset writes and patches are idempotent and guarded writes answer
`412` after a landed attempt, and leaves sorting that out to the caller; its mount appears to
publish unguarded, so a resent edit can overwrite another writer's save. With this change voidfs
keeps the guard and removes the false conflict.

Renames, deletes and attribute changes need no protocol change: the history and the recently
deleted listing (§4.10) identify their outcome exactly. Only content edits need a marker.

## Design

### Protocol text

§4.1, §4.2 and §4.3 each gain:

> `x-amz-meta-*` headers set user-metadata entries on the new version: each names an entry, which
> takes the header's value. Entries no header names keep their values from the version before.
> Without such headers the user metadata is unchanged. When a write creates the object, its user
> metadata is the headers' entries.

§4.0's list of where common headers apply is unchanged; `x-amz-meta-*` isn't one of §4.0's
headers.

Example: a batched edit that a client marks as its journal entry 42.

```
POST /cuts/a.mov?x-voidfs-patch HTTP/1.1
x-voidfs-if-version: 7.3
x-amz-meta-voidfs-entry: 3f9c….42
Content-Type: application/vnd.voidfs.patch

VFSP…
```

→ `200` with the common headers. `HEAD` of the new version returns
`x-amz-meta-voidfs-entry: 3f9c….42`, and every other entry the version before had.

### Server

`voidfs-server` reads the headers in its edit path, as `attrs_for_put` does for a put, and
`voidfs-core`'s attribute patch gains the entries to set, applied over the version before. No
format change: an object record's attributes already carry user metadata (format §7.7's `attrs.meta`).

### Client

- The queue sends `x-amz-meta-voidfs-entry: <state id>.<first entry id>` on every write and patch
  it publishes, as it does on puts; `voidfs-sdk`'s `WriteOptions` gains `metadata`.
- A mount edit whose guard fails checks the version right after its guard for its marker, exactly
  as a put does, and if it is there takes that version as its own.
- A run of entries whose reply may have been lost is retried on its own, without coalescing entries
  queued since. Otherwise a retry could carry more edits than the attempt that landed, and
  recognizing the landed version would mark the extra edits published unsent. An entry that
  followed it in the first attempt is sent again on top of the recognized version, which changes
  nothing: a run of writes and truncates applied again to its own result leaves it as it was.
  This client-side rule needs no protocol change, and it also covers puts.

## Compatibility

- **New client, old server:** an old server ignores the headers on edits (`voidfs-server` up to
  `2f3e217` reads only the offset, size and body, and `x-voidfs-mtime` and `x-voidfs-mode`). The
  edit's version inherits the version before's marker, which names another entry, so the client
  doesn't recognize it and reports the conflict it reports today. Nothing is misrecognized.
- **Old client, new server:** sends no headers; the user metadata is unchanged, as today.
- **Other servers implementing protocol 1:** as an old server, unless they adopt this.
- Additive (§8): the protocol stays 1. No format change, feature flag or migration.

## Conformance

One case, `user-metadata-on-edits`:

1. PUT `meta/f` with `x-amz-meta-colour: blue` and `x-amz-meta-entry: one`.
2. A write at an offset with `x-amz-meta-entry: two`: HEAD has `colour: blue` and `entry: two`.
3. A batched edit with `x-amz-meta-entry: three`: HEAD has `colour: blue` and `entry: three`.
4. A write with no metadata headers: HEAD still has `colour: blue` and `entry: three`.
5. A splice with `x-amz-meta-entry: four`: HEAD has `entry: four`.
6. A write that creates `meta/g` with `x-amz-meta-entry: new`: HEAD has only `entry: new`.

## Alternatives

- **Compare bytes instead.** After a `412`, read the version right after the guard and compare it
  with the guard's bytes plus the edits. No protocol change, but a full read of the object, which
  for a large video file is gigabytes, and a false match is possible when a foreign edit wrote the
  same bytes.
- **A request id the server remembers** (an idempotency key): exact for every operation, but the
  server must keep and expire per-request state, which no other part of the protocol needs.
- **Replace the user metadata, as PutObject does.** An edit carrying any header would drop
  entries it didn't name, which changes an object's metadata as a side effect of saving bytes.
- **Leave it.** The false conflict stays.

## Open questions

None.
