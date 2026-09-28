# RFC 0002: Garbage collection that is safe against writers

| | |
|---|---|
| Status | Proposed |
| Author(s) | voidfs maintainers |
| Created | 2026-09-27 |
| Affects | format |
| Implemented by | [`spec/format.md`](../spec/format.md) §7.4, §10, §12 (draft 1, revision 2) |

## Summary

Draft 1's two-phase garbage collection (format §12) can delete a shard that a new commit
references. This RFC closes that race and pins down what draft 1 left open:

1. `gc/pending.json` gets a **phase** (`marking`, `waiting`, `deleting`). It is created with a
   create-if-absent write when a run starts, so runs cannot overlap, and its Last-Modified time
   is `t1`, so `t1` comes from the bucket's clock.
2. Before phase 2 deletes anything, the collector marks the run `deleting`. A writer that saved a
   candidate by rewriting it checks for that mark before it commits.
3. Every shard and page a commit references needs one of three checks, **including objects the
   writer has just uploaded**.
4. Time limits: the grace period is at least 24 hours (except when collecting offline), phase 1
   finishes within half of it, and a writer commits within 12 hours of the check it relies on.

## Motivation

[RFC 0001](0001-metadata-in-the-bucket.md) (open question 2) made a model check of the GC
protocol a prerequisite for a Stable format. Working through the protocol for the first
implementation found a race, and several gaps that the safety argument depends on.

### The rewrite race

Draft 1 lets a writer keep a candidate alive by rewriting it with the same bytes (§12.3, third
option), and asks phase 2 to delete only candidates "still last modified before `t1`". Between
the collector's check of the last-modified time and its DELETE there is a window:

| Time | Collector (phase 2) | Writer |
|---|---|---|
| 1 | Recomputes references: X is unreferenced | |
| 2 | Checks X: last modified before `t1` | |
| 3 | | Finds X in `pending.json`; rewrites X |
| 4 | | Commits a version that references X |
| 5 | Deletes X | |

The new version now references a missing shard. The same happens to a writer that uploads bytes
identical to a candidate without knowing it is one: an upload *is* a rewrite. Draft 1's §7.4 let
a writer rely on "having written" an object, so every upload was exposed.

Draft 1 asks for a conditional delete "where the backend supports one". That does not help:
- **ETag.** The rewrite has the same bytes, so it has the same ETag.
- **Last-modified.** Among the target backends, only Amazon S3 directory buckets document a
  last-modified condition on DELETE (`x-amz-if-match-last-modified-time`). General-purpose S3
  buckets, R2 and MinIO don't.

### Gaps

- **Whose clock is `t1`?** Candidates are chosen by comparing `t1` with modification times,
  which the bucket assigns. A collector whose clock runs ahead would treat fresh shards as old.
- **How long can phase 1 take?** The writer's rule "commit within grace/2 of the check" is only
  sound if phase 1 publishes its candidates quickly. Nothing bounded it.
- **Two runs at once.** Nothing stopped a second collector from overwriting `pending.json`.
- **What counts as the check.** The second writer option ("confirm it exists and is not listed")
  is only sound if the list is read *before* the existence check.
- **References from other drives** (a copy from another drive, a fork's first checkpoint) and
  from multipart staging were not covered by "referenced by the state it is committing against".
- **Soft-deleted drives after their window.** §10 said they are roots *during* the window, not
  what happens after it.

## Design

### `gc/pending.json`

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
| `run` | Identifies the run |
| `phase` | `marking`, `waiting` or `deleting` |
| `t1` | The object's Last-Modified time when the run was created, truncated to whole seconds. Absent in `marking` |
| `grace` | The run's grace period, in seconds |
| `candidates` | Hashes proposed for deletion. A hash stands for the shard and the page of that name. Absent in `marking` |

### Phase 1: mark and propose

1. Create `gc/pending.json` as `{ "format": 1, "run": …, "phase": "marking" }` with a
   create-if-absent write. If it already exists, a run is in progress and no new run starts.
   Read the object's Last-Modified time: that is `t1`.
2. Compute the referenced set across the pool (the roots are unchanged from draft 1).
3. List `shards/` and `pages/`. A candidate is an object that is not referenced and was last
   modified before `t1 - grace`.
4. If there are no candidates, delete `pending.json`: the run is over.
5. If more than `grace / 2` has passed since `t1` on the collector's clock, delete
   `pending.json`: the run is abandoned.
6. Otherwise overwrite `pending.json` with phase `waiting`, `t1`, `grace` and the candidates.

### Phase 2: confirm and delete

1. When `t1 + grace` has passed on the collector's clock, overwrite `pending.json` with phase
   `deleting`. Read its Last-Modified time; if it is earlier than `t1 + grace`, write phase
   `waiting` back and try later.
2. Recompute the referenced set. This MUST start after step 1 completes.
3. Observe each candidate's Last-Modified time after step 1 (list, or HEAD each). Delete each
   candidate that is still unreferenced and was last modified before `t1`.
4. Delete `pending.json`.

A run in `waiting` or `deleting` whose collector stopped MAY be continued by the next collector.
A run left in `marking` MAY be deleted once it is more than `grace` old. At most one collector
works on a pool at a time. Create-if-absent stops two runs from starting at once, but the
deployment must ensure that no two collectors continue the same run.

### The grace period

`grace` MUST be at least 24 hours. When no writer is active in the pool (every authority is
stopped), a collector MAY use any grace, including zero. That is for tests, development and
cleanup after benchmarks.

### Writers

Before committing, the writer MUST have done one of the following for **every** shard and page
the commit references. This applies whether or not it uploaded the object in this request.

1. **Referenced.** It saw the object referenced by a root (§12: a drive's state, a readable
   checkpoint, an open multipart staging record) as that root stood at time `r`. This covers
   edits that keep shards, restores, copies, forks, and completing a multipart upload.
2. **Not a candidate, then present.** It read `pending.json` at time `r` and found no run, or a
   run that does not list the object. Then it confirmed the object exists with a request that
   started after `r`: an upload (PUT), a GET or a HEAD.
3. **Rescued.** It read `pending.json` at time `r` and found the object listed by a run in phase
   `waiting`. It rewrote the object with its bytes after `r`. It then read `pending.json` again
   and found the same run, still not `deleting`.
   - If the second read finds the run `deleting`, the writer MUST NOT commit. It waits for the
     run to end, then uses the second option.
   - If the second read finds a different run, or no run, it starts again.

The writer MUST NOT commit more than 12 hours (half the minimum grace) after the `r` it relies
on.

**Renewal.** A writer MAY cache `pending.json` and re-read it at least once an hour. A check
under the second option then stays valid while every re-read omits the object, with `r` taken as
the latest re-read. It lapses if a re-read lists the object, or if an hour passes without a
successful re-read. Likewise, an object rescued from a run stays rescued for as long as that run
is the one in `pending.json`.

### Deleted drives

A soft-deleted drive stays a GC root until it is hard-deleted. Once its window has passed
(default 30 days), the authority or the collector MAY hard-delete it.

A hard delete starts with `drive.json`, because the drive stops being a root when that object
goes. Before that, the authority stops starting operations on the drive and lets those in
progress finish: a copy from the drive, or a fork of it, references content through the drive
under option 1. For the same reason, a server whose hard delete fails part way must not keep
serving the drive: a later copy from it, or a fork of it, could reference shards that have
already been collected.

## Why this is enough

*(informative)* The model in `crates/voidfs-server/src/gc/model.rs` checks these rules; this is
the argument in brief. A run deletes an object X only if all three hold:
- X is one of its candidates;
- X is unreferenced in the recompute, which starts after the `deleting` mark;
- X is observed, after that mark, to be last modified before `t1`.

Then, for each option:

- **Option 2.** A run that deletes X did not list it at `r`, so it published its candidates
  after `r`. Phase 1 publishes within `grace / 2` of `t1`, so the recompute starts after
  `t1 + grace`, which is more than `r + 12 h`. The writer committed before then, so the
  recompute sees the commit. X also still existed at the commit, because the run deletes only
  after recomputing.
- **Option 3.** The second read saw no `deleting` mark, so the rewrite finished before the mark.
  The collector therefore sees a Last-Modified time after `r`, and `r` is after `t1`, so it keeps
  X. Later runs start after this one ends, and their phase 1 sees the commit.
- **Option 1.** This follows by induction over commits. A root that references X at `r` either
  still references it at the recompute (so X is kept), or dropped it after `r`. If it dropped it,
  any run that lists X marks after the drop, and its recompute comes more than 12 hours after
  `r`.
- **Uploads.** An upload of a candidate's bytes is a rewrite, so the writer uses option 3 for
  it, or option 2 when the object is not listed.

### What was checked

- **The model** explores every interleaving of a collector, one or two writers and a drive
  deletion, with writers that re-upload, read and then re-upload, and copy between drives.
  - The rules above are safe in all 16 configurations, the largest with 56 million states.
  - Dropping any one rule produces a counterexample: the `deleting` mark, the recheck after a
    rescue, the checks on uploads, or the bound on phase 1. So do draft 1 as written, and a
    writer that trusts its cache.
- **The implementation** runs on a simulated bucket with a manual clock. Several writers and a
  collector in another process run at once, while requests are reordered and fail before or
  after taking effect.
  - 24,000 seeds kept every reference.
  - Removing the `deleting` mark, the recheck, or the cache check makes it lose data within 256
    seeds.

## Compatibility

- Readers are unaffected: nothing a reader loads changes.
- No collector implemented draft 1's §12, so there are no runs to migrate.
- The format version stays 1. The specs are drafts, and [`spec/CHANGELOG.md`](../spec/CHANGELOG.md)
  records the revision.

## Conformance

The protocol suite cannot observe garbage collection. The checks are:
- the model of the protocol, which explores interleavings of a collector and writers;
- simulation tests of the implementation on a simulated bucket.

The format-level suite that RFC 0001 promises will add fixtures for `pending.json` and the GC
roots.

## Alternatives

| Option | Why not |
|---|---|
| Conditional delete on the ETag | A rewrite has the same bytes, so the same ETag |
| Conditional delete on the last-modified time | Only S3 directory buckets document it |
| Writers remove the objects they rescue from `pending.json` with a conditional PUT | Needs `If-Match` on every backend, and all writers of the pool would contend on one object |
| A lock between the server's writers and its own collector | Sound only when a single process does all the writing and collecting |
| Writers wait out any run that lists an object they need | Re-uploading a file deleted a few days earlier could wait up to a day |

## Open questions

1. Is a 24-hour minimum grace acceptable for every deployment? It only delays reclamation; the
   offline exception covers tests and cleanup.
2. **Undelete racing expiry.** A collector that hard-deletes an expired drive can race a server
   that is undeleting it. The server checks that `drive.json` still exists, but without a
   conditional delete the race cannot be closed completely. Undeleting a drive at the very end
   of its window while a collector runs is therefore not supported.
3. Draft 1 let large candidate sets be split into pages. This revision keeps them inline, about
   70 bytes each, so a million candidates make a 70 MB object. Splitting can come later, as an
   additive change.
