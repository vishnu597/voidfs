# RFC 0003: Small content inside the content descriptor

| | |
|---|---|
| Status | Implemented (accepted 2026-09-29) |
| Author(s) | voidfs maintainers |
| Created | 2026-09-29 |
| Affects | format |
| Implemented by | [`spec/format.md`](../spec/format.md) §3.1, §5, §7.7, §8.2, §11, §12 (draft 1, revision 5), three conformance cases, and voidfs-server ([#14](https://github.com/vishnu597/voidfs/pull/14), [bench/results/small-content](../bench/results/small-content/README.md)) |

## Summary

A new extent kind, `{ "d": "<base64>" }`, carries up to 4 KiB of a file's bytes inside its content
descriptor, so a small file needs no shard. A put of a small file then takes one round trip to
the bucket, the log entry, instead of two one after the other: the shard, then the log entry.

- Readers must be able to read `d` extents; writers use them only after adding the
  `inline-data` feature to `features.incompatible`.
- A `d` extent has the same ETag as a shard extent with the same bytes, so where the bytes live
  never shows to clients.
- Checkpoints never carry `d` extents: the checkpoint writer stores their bytes as shards and
  lists shard extents instead (**spilling**). Inline bytes are only ever in the log since the last
  checkpoint and in the server's memory of it, which bounds both.
- Garbage collection is unchanged: a `d` extent references nothing.

## Motivation

Every data write takes two bucket round trips one after the other, because a commit may only
reference shards that are already stored (format §7.4). For small files the shard upload is as
long as the log write, so the write takes twice as long as the bucket itself:

| Bucket 12 ms away ([rtt12-main-1, -2](../bench/results/health-metrics/)) | voidfs p50 | Bare bucket p50 | voidfs | SpaceFS |
|---|--:|--:|--:|--:|
| put 4 KiB, 8 at once | 32.8–37.2 ms | 12.4–12.6 ms | 2.6–3.0× slower | 2.0× slower |
| overwrite 4 KiB, 8 at once | 30.0–30.3 ms | 12.2–13.3 ms | 2.3–2.5× slower | 3.1× slower |
| fan-out put 1,000 × 4 KiB, 32 at once | 36.5–37.8 ms | 12.1 ms | 3.0–3.1× slower | 2.2× slower |
| fan-out put 1,000 × 4 KiB, 64 at once | 35.6–36.5 ms | 12.0–12.1 ms | 3.0× slower | 2.4× slower |

The server's own counters show the cost directly ([requests-rtt12](../bench/results/health-metrics/requests-rtt12.md)):
put 4 KiB makes 1.00 shard PUT and 0.31 log entries per operation (group commit shares entries),
and the fan-out puts 1.00 and 0.04–0.08. With `d` extents they would make only the log entries,
and take one round trip instead of two. That is the checklist's E9 ([RESEARCH_AND_PLAN.md](../docs/RESEARCH_AND_PLAN.md)),
and [step 3, item 3](../docs/step-3-performance.md) change 1.

Without it, these rows stay at two round trips, and three of the four behind SpaceFS's ratio.
Group commit (step 3, item 1) and taking checkpoints off the commit path (item 3, change 3) have
already taken what the server can do here without a format change.

## Design

### The extent

Format §5 gains a third extent kind:

```json
{ "extents": [ { "d": "aGVsbG8sIHdvcmxkCg==" } ] }
```

- `{ "d": b }` is the bytes whose standard base64 encoding (RFC 4648 §4, with padding) is `b`.
  Its length is the length of those bytes, which MUST be at least 1.
- The `d` extents of one descriptor MUST NOT hold more than 4,096 bytes in total.
- `d` extents MAY appear anywhere in an `extents` list, beside `s` and `z` extents. They MUST NOT
  appear in manifest pages (§5.1), where content is large enough to need a tree, nor in multipart
  staging records (§11), whose parts a completed upload joins into one descriptor that the limit
  would not fit.
- Readers MUST reject a descriptor with an invalid `d` (not base64, empty, or over the limit).

*(informative)* The first writer uses them only for whole files of at most 4 KiB: a descriptor of
one `d` extent, or none for the empty file. `d` extents beside shards would let an append to a
large file skip the upload of a new last shard, at the cost of chunk boundaries that are no longer
content-defined; that is a writer's choice, left for later.

### ETag

Format §7.7's ETag rule treats a `d` extent exactly as a shard extent holding the same bytes: it
contributes `s`, the 32-byte SHA-256 of its bytes, and their length as a u64. A file has the same
ETag whether its bytes are in a descriptor or in a shard, so spilling (below) changes no ETag, and
neither does a copy to a pool without the feature.

### The feature flag

`inline-data`, in `features.incompatible` (§3.1). A reader that does not implement it refuses the
pool, which is what it must do: it would fail to parse the new extent.

A writer adds the flag to `voidfs.json` before it writes the first `d` extent. Since servers read
`voidfs.json` when they start, the flag is added by an operator's action (for example a
`voidfs-server` subcommand, or a setting on a new pool), after every server that writes the pool
has been upgraded, never silently by a server that finds it missing.

### Commits

A transaction that sets content with `d` extents references no shard for those bytes, so the
§12.4 checks do not apply to them, and the commit is the only request. The bytes count towards the
log entry's size: a writer that caps entries (voidfs-server: 1 MiB) fits about 180 puts of 4 KiB
in one. Log entries also count towards the next checkpoint (§8.4: 16 MiB of log), so a stream of
4 KiB puts makes a checkpoint about every 3,000 puts rather than every 1,000 commits.

### Checkpoints: spilling

A checkpoint MUST NOT contain `d` extents. For each `objects` and `history` row whose content has
them, the checkpoint writer:

1. stores the bytes of each `d` extent as a shard (§4), with the §12.4 checks as for any shard;
2. lists the row with a descriptor in which each `d` extent is replaced by `{ "s": sha256(bytes),
   "n": length }`, merging nothing else.

The two descriptors are **equivalent**: the same bytes, the same ETag. Format §8.2's "carries each
version's content descriptor" becomes "an equivalent of it". A reader that loads the checkpoint and
then the log after it sees `s` extents for versions up to the checkpoint and possibly `d` extents
after. Nothing may depend on which: content is compared through ETags, not descriptors.

*(informative)* voidfs-server writes checkpoints in the background, after the commit that makes one
due, so the spilled uploads never delay a request. It also swaps the spilled descriptors into its
in-memory state once the checkpoint is written, which bounds the inline bytes it holds for a drive
by the log since its last checkpoint: at most 16 MiB of log, so about 12 MiB of content, while
checkpoints succeed (a failed one waits another interval). The number of shard uploads is the same
as today's, one per distinct small version; they move off the request path.

### Garbage collection

`d` extents reference no shard or page, so §12 is unchanged: the referenced set ignores them. The
spilled shards are referenced by the checkpoint that lists them, which is a root (§12), and are
checked when stored like any shard. A checkpoint that fails part way leaves spilled shards that
nothing references; they are collected as garbage, as a failed checkpoint's segments already are.

### Forks

A fork's first checkpoint (§9) follows the same rule: it spills whatever `d` extents the source's
state still has.

### The threshold: why 4 KiB

- SpaceFS's small-write rows, which this RFC exists for, are 4 KiB (4,096 bytes, so the limit is
  inclusive).
- Base64 adds a third: 4 KiB of content is about 5.5 KB of log. A larger limit makes log entries
  larger and checkpoints more frequent for the same number of puts; 16 KiB would mean a
  checkpoint every ~750 small puts, and groups of at most ~45 in one 1 MiB log entry.
- Memory: with spilling, a server holds at most the log since the last checkpoint in inline bytes,
  whatever the limit. Without spilling (see Alternatives) the limit would set how much a drive of
  small files costs in memory and in every checkpoint, and 4 KiB would already be too much.

A writer MAY use a lower limit, down to not using `d` extents at all. The format's 4,096 bounds
what a reader must accept.

## Compatibility

- **Protocol:** unchanged. No request, response, header or ETag differs.
- **Old readers** refuse a pool with `inline-data` in `features.incompatible` (§3.1), before
  reading any drive. Old servers already running when the flag is added would not see it until
  they restart, which is why the flag is added by an operator after upgrading them.
- **New readers** of pools without the flag see no change.
- **Format version:** stays 1; the flag is how format 1 adds features. `inline-data` is a new
  reserved name alongside those in §3.1.

## Conformance

The conformance suite is for the protocol, and a client cannot tell whether a server used `d`
extents. The cases below check that nothing leaks either way:

- `small-files-roundtrip`: put and get 0, 1, 4,095, 4,096 and 4,097 bytes; ranges within each;
  ETag and size on every response.
- `small-file-edits`: `write-at`, `splice` and truncate that take a 4 KiB file over the limit and
  back under it, checked byte for byte and against the version history.
- `small-file-history`: several puts to one 4 KiB key, then reads of an early version by version
  id and a restore of it: same bytes, same ETag as when it was written.

Reading a version back after a checkpoint has spilled it takes about 1,000 commits first, which a
case cannot express (the format has no repeated steps); that, and a test vector for the ETag
equivalence (the same bytes as a `d` extent and as a shard extent), go with the implementation's
unit tests.

## Alternatives

**Inline bytes kept verbatim in checkpoints.** Simpler: no spilling, a checkpoint is an image of the
state. But the server holds every drive's state in memory, and `history` keeps every retained
version's descriptor: 100,000 small files edited ten times each would hold up to 4 GB of inline
bytes in memory and in checkpoint segments, against about 300 MB of rows today. Segments are cut
by row count (256 to 8,192 rows, §8.3), so a history segment could reach 45 MB and be rewritten
whole for one changed row. Bounding that needs a per-drive budget and byte-bounded segments, which
is more format than spilling is.

**Packing small shards.** Several small files' bytes in one shard, with extents that name a range of
a shard. It saves requests and objects, but not the round trip: the packed shard must still be
stored before the commit that references it. It is also a larger change (extents with offsets, and
GC of partly referenced shards).

**Data in the log entry, referenced by position.** A commit could carry the bytes once in a section of
its own, with descriptors pointing at `(seq, index)`. Readers of an old version would then need its
log entry, which §8.5 lets a deployment delete, and log entries would become GC roots for content.

**Writing the shard and the log entry at once.** Not allowed by §7.4, for good reason: a commit that
lands before its shard would name missing content if the shard's upload failed.

## Open questions

Accepted as proposed (29 September 2026), which settles them so:
1. Checkpoints never carry `d` extents; every checkpoint spills. The implementation measures what
   that costs a checkpoint of a drive with many fresh small files, and a later RFC may relax it.
2. One format-wide maximum, 4,096 bytes; no `inline_max` in `voidfs.json`. Writers may use less.
3. How an operator turns the flag on is not part of the format; the implementation's pull request
   decides it, within what [The feature flag](#the-feature-flag) requires. It chose both:
   `--new-pool-feature inline-data` for a pool the server creates, and `voidfs-server pool enable
   inline-data` for an existing one, once every server that writes it is upgraded.
4. `d` extents may appear beside `s` and `z` extents, as the design says; the first writer uses
   them only for whole files of at most 4 KiB.

As they were asked:

1. **Spill in the checkpoint, or later?** Spilling makes a checkpoint of a drive with many fresh
   small files take longer (one upload per small version, in parallel). An alternative is to let a
   checkpoint carry `d` extents up to a bounded total and spill them at a later one. This RFC picks
   the simpler rule, "never in a checkpoint"; measurements from the implementation should confirm
   it.
2. **The limit per pool?** `voidfs.json` could name the limit (`"inline_max": 4096`), as it names
   the chunking parameters. This RFC keeps one format-wide maximum, with writers free to use less.
3. **How the flag is turned on.** A `voidfs-server pool enable inline-data` subcommand, a flag on
   new pools only, or both.
4. **`d` extents beside shards.** Allowed by the format here, unused by the first writer. Worth
   keeping, or should the first version restrict `d` to whole files of at most 4 KiB, and relax
   that later with another flag?
