# RFC 0004: A version for every object a transaction changes

| | |
|---|---|
| Status | Implemented (accepted 2026-10-02) |
| Author(s) | voidfs maintainers |
| Created | 2026-10-02 |
| Affects | both |
| Implemented by | [`spec/format.md`](../spec/format.md) §3.1, §7.1, §7.5, §8.2 and [`spec/protocol.md`](../spec/protocol.md) §1.1, §1.2, §3, §4.4–§4.7, §5.6 (draft 1, revision 6), five conformance cases, voidfs-core and voidfs-server ([#28](https://github.com/vishnu597/voidfs/pull/28)) |

## Summary

A transaction's version belongs to every object whose state it changes, not only to its target.
Each such object gets a history row under the same version id, and its head moves to that version.
A folder restore stays **one version with one id**, and that id appears in the history of every
file it rolls back, brings back or takes out. A write that creates missing folders is each one's
first version, and a rename that replaces its destination records the destination's deletion.

- Format: §7.1 and §7.5 say which objects a version belongs to and what row each gets; §8.2's
  `history` table already keys rows by object and version, so no table changes. The `removed` row
  documents the `last_version` it already carries.
- Every lookup by a version id alone becomes a lookup by object and version.
- An incompatible feature, `multi-object-versions`, guards it: readers that implement it derive a
  different history from the same log, so older ones must not write the pool.
- Protocol: no new request, header or field. §1.1, §4.4, §4.6 and §5.6 say what a client now sees.

## Motivation

### What happens today

`ops::restore_subtree` (voidfs-server at `e0b7808`) plans a folder restore as one transaction
whose target is the folder: it removes everything inside, then puts back what was there at the
instant, with its content and attributes as they were. `DriveState::apply_txn` moves the head of
every object a change names to the new version, but writes one history row, the target's. The
CLI's tests ([step 4, item 2](../docs/step-4-client.md#item-2-the-cli-on-the-protocol)) found what
follows:

1. **A restored file's history doesn't list its current version.** `?x-voidfs-versions` marks an
   older row `isLatest`, and `GET ?versionId=<its current version>` answers `404 NoSuchVersion`:
   the version maps to the folder, and the folder isn't at the file's key.
2. **Reading the past reads the wrong content.** `x-voidfs-as-of` at an instant after the restore
   finds the file's last own row, from before the restore, so it serves what the restore undid.
3. **Files the restore left alone move too.** Everything inside is removed and put back, so an
   unchanged file's head becomes the restore's version, which its history doesn't have either.
4. **The folder's head keeps its old version** (no change names the folder itself), while its
   history lists the restore as the newest row.
5. **Implicitly created folders have no history.** A put of `a/b/c` creates `a/` and `a/b/` "in
   the same version" (protocol §1.2). Their head is the put's version, but their history is empty,
   so once deleted, their `lastVersionId` names a version of another object.
6. **The change feed reports a folder restore as one change**, for the folder. Protocol §5.6 says
   "one change per key a version affected", and a mount needs to know which files changed.

The `lastVersionId` fix (removed objects record their own head, not the target's) is separate and
needs none of this; with it, cases 1–5 remain.

### Why it matters

- The format contradicts itself: §7.1 says a transaction's version is its target's, and §8.2 says
  `history` "holds every version of every object". For a transaction that changes several objects,
  both can't hold.
- **Parity.** SpaceFS documents (docs.spacefs.com/llms-full.txt, read 1 October): "Every
  successful mutation creates exactly one version, always"; "isLatest identifies the current
  head"; the version list is "a complete, ordered record of every content change … nothing a
  writer can skip"; `restore` "given a folder … rolls the whole subtree back"; and the overwritten
  version "can still be read or restored". voidfs breaks the second and the third for every file a
  folder restore touches.
- Clients that read history (the CLI's `history`, the mount's version browser, step 5) show wrong
  answers, and a restore by the id a folder restore returned fails for every file in it.

### What SpaceFS does (step 1)

Step 1 ([step 4, §1.4](../docs/step-4-client.md#14-uploads-and-a-folder-restore-observed-through-computer-use-2-october-02333);
Space 0.2.333, 2 October) observed:
- Versions are per object: each file and folder has its own version ids, and a history lists only
  that object's. A deleted file's history answers `404`, and there is no recently-deleted listing.
- Through the S3 API, a restore takes a version id only (`x-s3sdk-as-of` on a restore is `400
  InvalidArgument`, "versionId is required"), and restoring a folder by its own version restores the
  folder object alone: one `restore` version of the folder, nothing inside it changed. voidfs's
  restore by version does the same.
- The mount gave a folder a new version when its entries changed (a file added, one deleted),
  though the API's history of the folder doesn't list it.
- Only Space's CLI rolls a folder's subtree back, and it needs a signed-in session. Inferred from
  its strings, it writes Space's engine transactions directly (`RESTORE`, `GROUP_MOVE_IN`,
  `GROUP_MOVE_OUT`, `restore_from_version`).
- The one subtree restore tried, to an instant when every file in the folder existed, printed
  "restored 3 record(s)" and no version id, and **removed the folder and everything in it**: the
  API and the CLI both lost the folder, and the files' old versions could no longer be read by id.
  (Inferred: the CLI's history began after the folder and one file were created, so it took them
  as not existing at that instant.)

So whether a working Space folder restore gives each file a `restore` entry, and whether one id
names it, remains unknown. Space did show what design B prevents: a folder restore that makes
history unreadable. In voidfs every file keeps its history through a restore, and under this RFC
the restore's id also reads each file as restored. Design B follows what SpaceFS documents ("exactly one version", "isLatest
identifies the current head", a history "nothing a writer can skip") and voidfs's own §1.1 and
§8.2. Should a later observation show Space giving each file its own id, design A (below) is the
match.

## Design

### Which objects a version belongs to (format §7.1, §7.5)

An object's **state** is its record (§7.7, every member but `head`), its entry (parent and name),
and whether it is in the namespace. A transaction **changes** an object if the object's state
after the transaction differs from its state before, or if the transaction creates it.

- The version of a transaction belongs to its **target** and to **every object it changes**.
- Each of them gets one history row at that version (below), and its `head` becomes that version.
- An object that a change names but that ends up as it was (a folder restore removes and puts back
  everything inside the folder, so files it leaves alone are named twice and unchanged) gets no row,
  and its head stays.
- The descendants of a folder that moves, or that is removed with `recursive: true`, are not
  changed: their records and entries stay; only their keys follow the folder. As today, a folder
  rename is one row, the folder's (and one feed change, §5.6).
- The target always gets its row and its head, even when nothing about it changed (a folder
  restore of a folder already in the state restored to). Every mutation's version id then names a
  version of the object at the key the request named.

§7.1's "A transaction's `target` is the object whose version it is" becomes: "A transaction's
`target` is the object the request named. Its version is the target's, and that of every object it
changes (§7.5)." §7.5's `set` row, "Creates a version of the object when it is the target",
becomes "The object's version is the transaction's (§7.5, below)".

### The rows (format §8.2)

Each row is built from the object's record after the transaction, as the target's is today:
`{ "oid", "version", "time", "op", "size", "etag", "content", "attrs", "restored_from", "actor" }`.

- `op` is the transaction's `op`, except for an object the transaction takes out of the namespace
  (removed, and not put back): its row's `op` is `delete`. A plain delete's target already gets
  `delete`. So a rename that replaces `doc.txt` gives the source a `rename` row and the old
  `doc.txt` a `delete` row; a folder restore gives the files it takes out `delete` rows.
- `restored_from` is the value a `set` in this transaction gave the object, and is absent
  otherwise. A transaction that changes an object without setting it clears the record's
  `restored_from` (today only the target's is cleared), so no later row carries a stale one.
- A **restore planner** sets `restored_from` on every object it rolls back or brings back, to that
  object's head at the instant restored to, which under this RFC is always a version in the
  object's own history. (voidfs-server's `restore_subtree` sets it today only where the content or
  attributes differ; an object brought back unchanged would get none.)
- The `removed` row is documented with what the implementation already stores:
  `{ "key", "oid", "version", "last_version", "time", "kind", "size" }`, where `version` is the
  version that took the object out and `last_version` its head before that transaction (protocol
  §4.10's `lastVersionId`).

Rows of one version under several objects need no new key: `history` is keyed `<oid>@<seq>.<i>`.

*(informative)* A folder restore that changes `n` files adds `n + 1` rows, each with its file's
content descriptor (a tree's root or a short extent list), where today it adds one. Files it leaves
alone add nothing.

### Implicitly created folders

A write that creates missing folders (protocol §1.2, §4.7) changes each of them (it creates them),
so each gets a row at the write's version, with the write's `op` (`put`, `write`, `copy`,
`rename`), size 0 and the folder ETag `""`. A folder's history then starts with the version that
made it, its head is in its history, and its `lastVersionId`, once deleted, brings it back.

### Lookups by object and version

A version id no longer names one row on its own, so every lookup also names the object:

| Where | Today | With this RFC |
|---|---|---|
| `DriveState::version`, the `versions` map | version → its target | gone; `version(oid, v)` reads the object's history |
| `GET`/`HEAD ?versionId=` (protocol §4.5) | the version's target, which must be at the key or a removed object last there | the object at the key, or a removed object whose `removed` row is at the key, then its row at `v` |
| CopyObject's `x-amz-copy-source: …?versionId=` | any object's version in the drive, whatever the key | as for `GET` |
| Restore by version (§4.6, §4.10) | the version's target | as for `GET` |
| A checkpoint's spill (`with_spilled`, RFC 0003) | keyed by version | keyed by object and version, as `history` is |

Several removed objects can share a key (a file deleted, made again and deleted again). The lookup
takes the one whose history has `v`; should more than one have it, the most recently removed.

Forks are unchanged: version ids at or before the fork point are shared and name the same rows.

### Checkpoints

No new table, key or segment rule. A reader that loads a checkpoint written under this RFC sees
rows for one version under several objects; a reader that replays the log after it applies the
rule above to each transaction.

Checkpoints written before the feature keep their rows. A drive's history before its newest such
checkpoint stays as it was recorded, with the gaps above, unless it is rebuilt: by default every
commit is kept (§8.5), and replaying a drive's log from its first commit (or its fork point's
checkpoint) under the new rule gives the history it would have had. voidfs-server can offer that
as part of enabling the feature (open question 2).

### Protocol text

- **§1.1:** after "Every successful mutation creates exactly one version": "A version can belong to
  several objects, with the same id in each one's history: a folder restore's version is the
  folder's and that of every object the restore changes (§4.6); a write that creates folders is
  their first version (§1.2); a rename that replaces a file is that file's deletion (§4.7)."
- **§4.4:** "`isLatest` marks the object's current version, the one a plain `GET` returns."
- **§4.6, folder restore:** "as one version of the folder" becomes "as one version. It is the
  folder's version and that of every object the restore changes: a file it rolls back or brings
  back lists it with `operation: restore` and `restoredFrom` naming the file's own version that was
  current at the instant, and `GET ?versionId=<id>` at that file serves it as restored. A file it
  takes out is listed by §4.10, with the version before the restore as its `lastVersionId`."
- **§5.6:** "There is one change per key a version affected": for a folder restore, one change per
  object it changed (`restore`, or `delete` for one it took out), each with the restore's
  `versionId`, after the folder's own.

## Compatibility

- **The log** doesn't change: the same commits, transactions and changes. What changes is the state
  a reader derives from them. Two writers that derive different histories from one log must not
  share a pool, so this is an incompatible feature.
- **`multi-object-versions`**, in `features.incompatible` (§3.1), added by an operator once every
  writer of the pool implements it, as `inline-data` was (RFC 0003). A reader that implements it
  applies the new rule to every commit it replays, written before the flag or after. An older
  reader refuses the pool.
- **Clients** see more history rows (each changed file lists a folder restore, created folders list
  their first version), a folder's HEAD at the restore's version, `GET ?versionId=` and
  `x-voidfs-as-of` that serve restored files as restored, and a feed change per restored file.
  Nothing parses version ids, and no response gains or loses a member. `ListObjectVersions` gains
  the same rows.
- **Old servers** keep today's behaviour; a client can't ask for either.
- **Versions:** format and protocol stay at 1.

## Conformance

New cases, for a server with the feature:
- `folder-restore-history`: change, add and delete files in a folder, restore it with
  `x-voidfs-as-of`; each changed file's history lists the restore's version as its latest, with
  `operation: restore` and `restoredFrom`; `GET ?versionId=<restore id>` at each serves the restored
  bytes; the folder's HEAD carries that id; files left alone keep their version; the added file is
  in `x-voidfs-deleted` with its own `lastVersionId`.
- `folder-restore-as-of`: `x-voidfs-as-of` after the restore serves the restored content.
- `implicit-folder-history`: a put of `a/b/c` gives `a/` and `a/b/` a first version with the put's
  id; deleting `a/b/` and restoring it by its `lastVersionId` brings it back.
- `rename-replace-history`: the replaced file is in `x-voidfs-deleted` with its own last version,
  and restoring that brings back the old content.
- `copy-source-version-of-another-object`: `x-amz-copy-source: /drive/a?versionId=<a version of b>`
  is `404 NoSuchVersion`.
- `feed-folder-restore`: the feed reports one change per changed object, with the restore's id.

voidfs-core unit tests: the rows and heads above for each planner (put with new folders, rename
with replace, delete, folder restore with unchanged, changed, moved-out, added and deleted
objects, a name swap); `as_of` after a folder restore; lookups by object and version; a checkpoint
round trip with shared version ids; `with_spilled` keyed by object and version; a log written
before the feature, replayed under it; each test seen to fail with the rule broken.

## Alternatives

**Design A: one transaction per changed object, in one commit.** A folder restore would commit the
folder's transaction and one per file, each its own version (`seq.0`, `seq.1`, …). It needs no
format change, since each transaction already has its own row, and a commit applies all its
transactions or none. But:
- one restore makes many versions, against protocol §1.1 and SpaceFS's "exactly one version";
- the id the restore returns names only the folder's version: no single id reads the files as
  restored, and the feed reports as many versions as files;
- each transaction must be valid on its own, so a restore that swaps two names (which
  `restore_subtree` already handles) or moves a file back over another needs each such object
  removed in one transaction and put back in another: two versions each, or temporary names;
- implicitly created folders would need transactions of their own, so a put that creates folders
  would make several versions too.

It remains the fallback if the feature flag is unwanted.

**A row for every object a change names**, changed or not. Simpler to state, but a folder restore
would add a row to every file in the folder, and a restore of a large folder with one changed file
would rewrite every file's history.

**One row, with lookups made smarter.** Resolving a file's version through the target's row can't
work: the folder's row doesn't carry the files' content, and once a file changes again, the content
it had at the restore's version is nowhere else.

**A separate table of shared versions** (version → objects), with rows only for targets. It would
answer "which objects did this version change", but every read of a file at such a version would
still need its content, which only a per-object row holds.

## Open questions

Accepted as proposed (2 October 2026), which settles them so:
1. The feature is `multi-object-versions`.
2. History is not rebuilt. A reader that implements the feature applies the rule to every commit
   it replays, written before the feature was added or after; history in checkpoints written
   before stays as it was recorded. An operator turns the feature on as for `inline-data`:
   `--new-pool-feature multi-object-versions` for a pool the server creates, `voidfs-server pool
   enable multi-object-versions` for an existing one. A rebuild can be a later tool.
3. An object a transaction takes out gets a `delete` row, which §4.4's default listing leaves out.
4. Files a folder restore leaves as they were get no row and keep their head.
5. An implicit folder's first row has the write's `op`, so it is in the default listing after a
   `put`, `write` or `copy`, and only with `x-voidfs-all=true` after a `rename`.

The implementation changed two things from the design above:
- **§5.6's order.** The objects a version takes out are reported before its target, children
  before their folders, and the other objects it changed after it, folders before their children.
  A rename that replaces `doc.txt` is then `delete doc.txt` (the old file) followed by `rename
  doc.txt`, so a client that applies changes in order keeps the new file. A folder restore that
  moves an object back reports `fromKey`.
- **The conformance cases** are `folder-restore-versions-each-object` (the history, as-of and
  recently-deleted cases above), `writes-version-the-folders-they-make` (implicit folders, and one
  restored by its `lastVersionId`), `folder-restore-in-the-feed` (with a replacing rename),
  `replaced-file-comes-back-by-its-last-version` and `version-of-another-object` (reads and copy
  sources). The last two hold without the feature.

As they were asked:

1. **The feature's name.** `multi-object-versions` is proposed.
2. **Rebuilding history when the feature is enabled.** Replay each drive's log from its start
   (where it is retained) and checkpoint, so that old folder restores and implicit folders get
   their rows too; or leave history before the newest pre-feature checkpoint as it was.
3. **Removed objects' `op`:** `delete` (proposed), or the transaction's `op` (`restore`,
   `rename`)? `delete` keeps §4.4's default listing, which leaves deletions out, as it is.
4. **Rows for files a folder restore leaves alone:** none (proposed). Step 1 couldn't observe
   what Space does: its one subtree restore removed the folder instead.
5. **Implicit folders' rows in §4.4's default listing:** they follow content (`put`), so they are
   listed; should a folder's first version be listed as `put`, or should folders' histories list
   only explicit operations?
