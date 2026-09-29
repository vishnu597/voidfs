# Step 3: win the rows SpaceFS loses

*Work items for step 3 of the [parity plan](PARITY.md#7-step-by-step-plan), written on
27 September 2026 from the step 1 measurements. Read this before starting step 3. The
measurements, their method and their caveats are in [bench/README.md](../bench/README.md); the
code references point at commit `c434fcb`.*

Step 3 is **done when** every one of SpaceFS's 49 rows is at least as fast, relative to the bare
bucket, as SpaceFS's is. That can only be judged by the real run in their setup
(`BENCH_TOPOLOGY=client-host bench/scripts/cloud-run.sh`, whose table has a "voidfs vs SpaceFS"
column). Until then, the working yardstick is the local run with the bucket 12 ms away.

## 1. What the measurements say

Three runs (27 September 2026, one Mac):
- all 49 scenarios on loopback ([table](../bench/results/local-versitygw-loopback.md));
- all 49 with the bucket 12 ms away ([table](../bench/results/local-versitygw-rtt12.md));
- 23 small-object scenarios against Cloudflare R2, about 200 ms per PUT away
  ([table](../bench/results/r2-small-objects.md)).

Plus probes that isolate two of the causes, in [bench/results/probes/](../bench/results/probes/).

- **Metadata and warm reads are already ahead** of SpaceFS's ratios: listing, `head`, small gets.
- **Every write is held back by one cause (item 1)**, which alone sets the floor of 37 rows.
- **Warm reads silently go to the bucket** once the shard cache fills (item 2).
- The rest are smaller costs on specific rows (items 3–6), and gaps in what can be measured
  (item 7).

## 2. Work items

In order of impact. Each says what is wrong, the evidence, where the code is, the change, and
how to check it. Expected effects are estimates, not measurements.

### Item 1. Group commit: one log write per batch, not per mutation

**Status (28 September 2026): done.** Measured in
[bench/results/group-commit](../bench/results/group-commit/README.md):
- The probe's p50 stays flat from 1 to 8 at once: rename 13.9 / 20.7 / 26.1 / 26.3 ms, put 4 KiB
  28.7 / 28.1 / 28.1 / 29.7 ms.
- Over the 49 rows at 12 ms, p50 is half of `main`'s (geometric mean of the ratios 0.504; writes
  0.325, edits 0.464). Fan-out puts went from 425 and 851 ms to 37 and 38 ms, renames and folder
  moves from about 100 ms to 25–26.
- Rows at or ahead of SpaceFS's ratio went from 6–7 to 17 in both runs.
- The four small-write rows that garbage collection had made 3.5–7% slower now take 30–38 ms,
  from 106–851; what is left of that difference can't be told apart from round trips (see the
  README).
- On loopback, p50 is 10% lower over the 49 rows (20% relative to the bare bucket in the same
  run), so the committer's own costs don't show.
- Multipart uploads at 8 at once have a 4–9% higher p50 and a 10% lower p90, with the same
  throughput: their completions now land in one log entry, and the next uploads start together
  and compete for connections. One at a time, they take the same as before.

What was built, in `Pool::commit` ([pool.rs](../crates/voidfs-server/src/pool.rs)):
- **Queue and committer.** A mutation queues its plan on the drive and starts a committer task.
  Whichever holds the commit lock takes everything waiting (up to 256 transactions and 1 MiB
  encoded, unless the first alone is larger) and plans it in order, each transaction against
  the state the ones before it leave. The task is spawned, so a request that goes away cannot
  stop a commit others wait on. The lock is taken per entry, so forks and hard deletes still get
  their turn.
- **Losing the race.** Plans may run again: after another authority wrote the sequence number
  (format §7.2), the committer catches up and plans the whole batch again, up to eight times,
  before handing `Retry` back. The alternative, handing `Retry` to every request, would turn one
  lost race into a 503 for the whole batch. Planning again against the caught-up state is what a
  client's retry would do, and a precondition that no longer holds fails then as it would have.
  `run_edit` still gets `Retry` from its own head check, because only the request can compute the
  edit's content again.
- **Answers.** A request is answered once the entry holding it is written (§7.4), with the
  drive's state just after its own transaction. A plan that fails against the drive as installed
  is answered at once. One that fails after an earlier transaction of the batch waits for the
  entry, because its failure may rest on a transaction that is never written; if the entry isn't
  written, it gets the write's error.
- **Garbage collection.** A mutation that has waited more than an hour for the log is handed back
  with `Retry` rather than committed, so every commit stays well within 12 hours of the checks
  it relies on (§12.4), of which a body may take 6.
- **Change feed.** Each transaction's feed changes now come from the state just before and after
  it, on commit and on replay. `feed_for` used the state after the whole commit, which named the
  wrong keys once a commit held a put and then a rename of the same file.
- **Unchanged:** the checkpoint cadence counts log entries and their bytes (§8.4), and a
  checkpoint is still written under the lock after the entry that makes it due.
- **DeleteObjects** queues its keys together and in order (`Pool::commit_all`), so they share log
  entries: 200 keys at 12 ms went from 2.6 s to 16 ms.

What was left for later:
- Rename at 8 at once takes two round trips: after an entry lands, the first new request starts
  the next alone. A short hold before an entry, while the previous one had company, might bring
  it to one; not tried.
- Planning the next batch while an entry is in flight saves only CPU, a small part of a round
  trip here, so it was not done.

**Problem.** A drive commits one mutation per bucket round trip. `Pool::commit`
([pool.rs:619](../crates/voidfs-server/src/pool.rs#L619)) takes the drive's `commit_lock`
(line 624) and holds it while it writes the log entry with a conditional PUT (line 632).
Mutations on one drive queue behind each other, so any write row's p50 is its concurrency
times one conditional PUT.

**Evidence.**

| | Loopback | Bucket 12 ms away | R2, about 205 ms per PUT |
|---|--:|--:|--:|
| rename 64 MiB, at 1 / 2 / 4 / 8 at once | – | 12.8 / 24.5 / 48.4 / 99.2 ms | – |
| put 4 KiB, at 1 / 2 / 4 / 8 at once | – | 27.3 / 25.0 / 50.0 / 101 ms | – |
| edits in 1 MiB files, 8 at once | 6–8 ms | 91–97 ms | 1.5–1.7 s |
| fan-out put 1000 × 4 KiB, 32 / 64 at once | 12 / 23 ms | 412 / 859 ms | 6.7 / 13.0 s |
| Commits per second, one drive | about 2,500 | about 78 | about 5 |

Probes: [commit-concurrency-1](../bench/results/probes/commit-concurrency-1.md),
[-2](../bench/results/probes/commit-concurrency-2.md),
[-4](../bench/results/probes/commit-concurrency-4.md),
[-8](../bench/results/probes/commit-concurrency-8.md). SpaceFS's corresponding rows are 60–270 ms.

**Rows it moves:** 37. Every edit (24), put and overwrite (6), fan-out put (3), multipart
upload (2), rename and folder move (2).

**Change.** Batch every transaction that is waiting when the log becomes free into one commit.
The format already allows it: a commit holds one or more transactions, and transaction `i` of
commit `seq` is version `<seq>.<i>` ([format §7.1](../spec/format.md#71-commits)). The change
feed already walks `commit.txns` (`feed_for`, pool.rs:59), and `VersionId::new(seq, idx)`
already carries the index. No RFC is needed.

A shape that fits the current code:
1. Each mutation enqueues its `plan` closure and a oneshot reply on the drive, instead of
   taking the lock.
2. One committer per drive (a task, or whichever waiter wins the lock) drains the queue.
3. It plans each transaction in order against the running state: the snapshot, plus the
   transactions already planned in this batch. Preconditions and `run_edit`'s head check
   (object.rs:551) stay inside the closures and see the right state.
4. A closure that fails gets its error; the others stay in the batch.
5. The committer applies the batch, writes one log entry, installs the state, and replies to
   each request with its version.
6. If the conditional PUT loses (another authority wrote `seq`), every transaction in the batch
   is discarded and re-planned after catching up, never reported as done
   ([format §7.2](../spec/format.md#72-claiming-a-sequence-number-create-if-absent)).
7. Cap a batch by transactions and by encoded bytes, so log entries stay quick to replay.

**Do not** write commit `n + 1` before commit `n` is confirmed. If `n` loses the race, an
`n + 1` built on it could still win its own conditional PUT, and the log would mix two
authorities' histories. Planning and encoding the next batch while `n` is in flight is safe.

**Expected effect.** Write p50 of about 2–3 round trips at any concurrency, instead of
concurrency × 1. At 12 ms: renames and edits from about 95 ms to 25–40 ms, fan-out puts from
412–859 ms to 40–60 ms.

**Check.**
- Re-run the probes: the p50 should stay nearly flat from 1 to 8 at once.
  ```bash
  BENCH_ONE_WAY_MS=4 bench/scripts/local.sh --targets voidfs --scenario rename-64m --scenario put-4k --concurrency 8 --ops-scale 0.5
  ```
- Then the full 12 ms run against [its baseline](../bench/results/local-versitygw-rtt12.md).
- Tests in `pool.rs`:
  - a batch with preconditions that depend on each other in order;
  - a failed transaction inside a batch;
  - a batch that loses the race (extend `a_second_authority_cannot_fork_history`);
  - the change feed and history of a batched commit.
- Garbage collection left four small-write rows 3.5–7% slower at 12 ms, for a reason not yet
  found (overwrite 4 KiB and the three fan-out puts;
  [bench/results/gc](../bench/results/gc/README.md)). Compare them with `main-*` there once
  commits are batched.

### Item 2. Shard cache admission

**Status (28 September 2026): done.** Measured in
[bench/results/shard-cache](../bench/results/shard-cache/README.md):
- The probe sequence below: get 32 MiB 26.0–27.3 ms (`main` 54–79), the fan-out get 0.47–0.57 ms
  (`main` 13.1–13.7).
- Over the 49 rows at 12 ms, p50 is 0.77 of `main`'s (geometric mean of the ratios; reads 0.38,
  edits 0.88). The three fan-out gets take 0.5–1.1 ms instead of 12.4–15.1, get 32 MiB 25 ms
  instead of 56–88, and get 1 MiB's p90 is 1.6 ms instead of 15–16.
- 12 of the 16 edits inside 32 and 64 MiB files are 11–57% faster: an edit reads the shard it
  rewrites, which `main` had turned away since the file was written. The appends and the inserts
  at the start moved within the spread between runs.
- Rows at or ahead of SpaceFS's ratio: 21 and 22, against 16 and 19 for `main` in the same
  session. The geometric mean speed-up over the bare bucket went from 2.0× to 2.6×.
- Against Cloudflare R2, with a 64 MiB cache so that the small-object scenarios fill it: the
  fan-out gets took 0.6–2.2 ms instead of 75–116, get 1 MiB's p90 2.5 ms instead of 357, and six
  of the eight edits in 1 MiB files were 19–26% faster, one 1 MiB GET from R2 fewer.
- Writes: with the cache full, fan-out put 1,000 × 4 KiB at 64 at once is 1–3% slower in p50
  and 1.6–2.6% in round time; one at a time and at 8 at once, nothing shows. Inserting from a
  blocking thread did not help.

What was built, in `Pool::open_as` and the shard and page functions
([pool.rs](../crates/voidfs-server/src/pool.rs)):
- **Least recently used first**, for shards and pages (`EvictionPolicy::lru()`). Shards and pages
  never change, so what was written or read last is what the next read wants: the file just
  written, the one being edited. moka's TinyLFU admits a new entry only if it has been used more
  often than all the entries it would evict put together, so a shard just written (used no times)
  never got in once the cache was full, and a 2 MiB shard, which displaces many small ones,
  hardly ever.
- **Not a recency window in front of TinyLFU.** moka has none (Caffeine's W-TinyLFU does); it
  would take a second cache and moving entries between the two, and moka's comparison against
  the sum of the victims would still keep large shards out of the main part. What LRU gives up is
  scan resistance: a read of more than the cache evicts everything before it. The harness has no
  such scenario, so this is not measured. Where it matters, the disk tier (S5) is the place for a
  scan-resistant policy, such as a probation segment for entries read once.
- **Pages the same way.** The page cache (an eighth of the bytes) holds manifest pages and
  checkpoint segments, just as immutable. Only files of more than 1,024 extents (about 2 GiB)
  have manifest pages, and none of the benchmark's do, so this is decided by the argument and
  the tests, not measured.
- **The caches keep their own copy** of each shard and page (`cached`). A shard cut from an
  upload is a slice of the chunker's buffer, and kept that buffer (about 33 MB) alive while the
  cache counted the shard alone. After 48 puts of 16 MiB of fresh data, 1.1–1.2 GB was allocated
  for the 512 MiB cache (`main` included); with the copies, 0.63 GB. For 96 versions of one
  16 MiB file, which share all but one shard, allocation grew by a buffer a put, to 3.0 GB; with
  the copies, 0.25 GB. The copy is one memcpy of what the cache takes in, less than the hashing
  the write already does.
- **`Pool::extents`** flattens a manifest tree from the pages it fetched, not from the cache. If
  moka's upkeep ran between the fetch and the flatten, a page the cache had evicted or turned
  away (with `--cache-mib 0` the page cache holds one byte) failed the read with "manifest page …
  is missing".
- Unchanged: holding a shard is not a garbage-collection check (the reuse set in `gc/guard.rs`
  decides what a commit may reference), `Pool::forget` drops deleted shards and pages from both
  caches, a miss checks the shard's hash, and the caches stay within `--cache-mib`.

What was left for later:
- Scan resistance, with the disk tier (above).
- Inserts stay on the request path: the one row where they showed did not get faster with them
  on a blocking thread, and moka 0.12's async cache does its upkeep in the calling task too.
- The process's footprint also counts freed memory the allocator keeps: 0.84 GB after 48 puts of
  16 MiB with the cache off and nothing allocated. Not the cache, and not changed.

**Problem.** The shard cache is a moka cache with moka's default TinyLFU admission
([pool.rs:269](../crates/voidfs-server/src/pool.rs#L269)). TinyLFU lets a new entry in only if
it looks more popular than the entry it would evict. Shards that were read many times earlier
keep winning, even long after their drives are deleted, so newly written or newly read shards
are never admitted. Warm reads then go to the bucket, and nothing reports it.

**Evidence** (bucket 12 ms away, the same eleven scenarios in the full run's order):

| | TinyLFU (as shipped) | LRU (one-line change) |
|---|--:|--:|
| get 32 MiB | 75 ms | 27 ms |
| fan-out get 1000 × 4 KiB, 32 at once | 12.8 ms | 0.54 ms |

Probes: [cache-tinylfu](../bench/results/probes/cache-tinylfu.md) and
[cache-lru](../bench/results/probes/cache-lru.md).

In the full 12 ms run the second round of that fan-out read had a *fastest* read of 9.6 ms: not
one hit. Loopback cannot show this, because a miss costs about 1 ms there.

**Rows it moves:** the warm read rows after the cache fills. In the 12 ms run these were get
32 MiB, get 1 MiB (p90), and the three fan-out gets: 4–5 rows, all rows SpaceFS wins by 2.6–12×.

**Change.** The validated change is one line:

```diff
-            shards: moka::sync::Cache::builder().weigher(weigh).max_capacity(cache_bytes).build(),
+            shards: moka::sync::Cache::builder().weigher(weigh).max_capacity(cache_bytes).eviction_policy(moka::policy::EvictionPolicy::lru()).build(),
```

Shards are immutable and content-addressed, so recency is the right signal. Consider the same
for the `pages` cache (line 270). If scan resistance matters later, the alternative is a
recency window in front of the frequency filter, not TinyLFU alone. The disk tier (checklist
S5) comes after this.

*Added with garbage collection (28 September 2026):* moka's sync cache does its housekeeping
inside inserts, on whichever async worker calls it. A second moka cache on the write path (the
first version of the garbage-collection reuse set) made small-write rows 2–4% slower at 12 ms,
and replacing it with a plain set removed that part
([bench/results/gc](../bench/results/gc/README.md)). Whether the shard cache's own inserts cost
the same way is worth measuring when this item changes it, for example with an async cache or
with inserts moved off the request path.

**Check.** Re-run the sequence in the probes (its `--scenario` list is in the probe's JSON),
and the full 12 ms run: the fan-out gets should be under 1 ms, and get 32 MiB near get 64 MiB's
per-byte time.

### Item 3. Fewer sequential round trips per write

**Status (29 September 2026): changes 2 and 3 done; change 1 is
[RFC 0003](../rfcs/0003-small-content-in-descriptors.md), accepted, not yet implemented.** Measured in
[bench/results/write-round-trips](../bench/results/write-round-trips/README.md):
- **Measured first.** Put 64 MiB on loopback, one at a time, took 300 ms: about 150 ms of CPU in
  the request's own task and 150 ms waiting for seven batches of four shard uploads. The largest
  CPU cost was neither FastCDC nor SHA-256 but the upload checksum SDKs send (CRC32): the `crc`
  crate's default single table runs at 0.5 GB/s, 93–110 ms for 64 MiB. A checkpoint under the
  commit lock showed at p99.9, not p99: over 20,000 puts of 4 KiB at 12 ms, 32–37 took over
  100 ms (p50 29–31 ms).
- **Checkpoints in the background.** In the same probe, p99.9 went from 118–135 ms to 49–56, the
  slowest write from 145–166 ms to 55–88, and none took over 100 ms; p50 and p99 did not move. A
  checkpoint of that drive takes 79–87 ms, now off every request's path.
- **Ingest.** At 12 ms, put 64 MiB went from 371–376 ms (2.3× the bare bucket) to 204–221 ms
  (1.2–1.4×) eight at once, and from 345 ms (2.5×) to 122 ms (0.9×) one at a time; put 32 MiB
  from 2.1–2.2× to 1.3× eight at once and 1.05× alone. Of that, the checksum was about 100 ms of
  64 MiB's 160, and the window of uploads most of the rest. Eight large puts at once are now
  bound by this one machine's CPU, which the harness, versitygw and the server share.
- **Over the 49 rows**, relative to the bare bucket in the same run: at 12 ms the geometric mean of
  the p50 ratios against `main` is 0.987 (writes 0.922), on loopback 0.965 (writes 0.822). At
  12 ms no row crossed SpaceFS's ratio (20–23 rows are at or ahead, `main` 22–24 in the same
  session): the large puts are at 1.3×, and SpaceFS's at 1.1–1.2×. On loopback, put 32 and 64 MiB
  and overwrite 1 MiB crossed: 34–35 rows, `main` 31–32.

What was built:
- **Checkpoints in the background** (`Pool::commit_batch`, `Pool::checkpoint`). The commit that
  makes one due starts a task with the state it has just installed, and is answered at once. The
  commit lock still guards the count towards the next checkpoint; the last checkpoint, whose
  pages the next one lists without storing them, moved to a lock of its own, which the committer
  takes with `try_lock` while it holds the commit lock and hands to the task. So there is one
  checkpoint at a time per drive; if one is still being written when the next is due, the next
  commit tries again; a failure still waits a whole interval.
  - Forks and hard deletes take the commit lock and then the checkpoint lock, so they wait for
    one being written: a fork starts from it, and nothing of a deleted drive is written after it
    has gone.
  - The rows are copied, encoded and hashed on a blocking thread.
  - The index is written within 12 hours of when its state was the drive's, as §12.4 option 1
    requires of anything that references content through a root. Before, the state was the
    drive's throughout; now it may not be. Nothing expires versions yet, so a later state still
    references everything an earlier one did, but a retention policy would change that. A fork's
    deadline now counts its snapshot too.
  - A server that stops waits for checkpoints in flight (one cut short would be harmless).
  - `voidfs_checkpoint_write_seconds` reports how long they take.
- **Checksums with sixteen tables** (`s3/chunked.rs`): 5.6 GB/s instead of 0.5, the same crate.
- **Ingest that keeps reading** (`ingest` in `s3/object.rs`): the body is read and cut while up to
  16 shards or 32 MiB (one larger shard alone) upload; a failure fails the request at once and
  drops the uploads in flight; nothing is committed until the whole body matched its signature,
  and `MAX_INGEST` still applies. Shards of 256 KiB or more are hashed on blocking threads, at
  most one per CPU across requests, so that one body's shards hash in parallel.
- **The chunker's copy** (`StreamChunker::push`): each shard is copied out and the buffer advanced,
  instead of split off, which made the next input move the rest of the buffer, up to 16 MiB per
  shard of about 2 MiB. Each byte is now copied out about 1.4–1.8 times, and the buffer stays at
  32 MiB with the default chunking and body frames of 16–64 KiB.

What was left for later:
- Change 1, small files inside their metadata: [RFC 0003](../rfcs/0003-small-content-in-descriptors.md),
  accepted on 29 September, specifies a `d` extent with up to 4 KiB of content, an `inline-data`
  feature flag, and checkpoints that store those bytes as shards ("spilling") so that neither
  checkpoints nor memory grow with them. Its spec text, conformance cases and code come next.
- Multipart uploads of 8 and 16 MiB parts gain little from the window: their time at 12 ms is
  completion's chain of round trips (item 5).
- Put 1 MiB and the fan-out puts of 4 KiB, 8 to 64 at once at 12 ms, are 1.5–3.5% slower in p50.
  Nothing on their path costs more: one at a time, put 4 KiB takes the same time and put 1 MiB
  2–3% less. Closed-loop clients fall into step with group commit a little differently when each
  request's own work takes less time.
- SHA-256 runs twice over a body whose payload hash is signed (the signature's, then the
  shards'); nothing can share them.

**Problem.** Even alone, a data write takes two bucket round trips one after another: the new
shards, then the log entry. A 4 KiB put at concurrency 1 took 27.3 ms where a rename, one round
trip, took 12.8 ms. SpaceFS says it commits in "one wave". The format forbids simply doing the same: a commit
may only reference shards that are already durable
([format §7.4](../spec/format.md#74-durability-order)).

Three more costs sit on the same path:
- `ingest` ([object.rs:310](../crates/voidfs-server/src/s3/object.rs#L310)) stops reading the
  request body while each batch of four shards uploads (line 325). With chunking and hashing on
  the server, 32 and 64 MiB puts run 2× slower than the bare bucket even on loopback. Which of
  these dominates has not been measured.
- `StreamChunker::push` ([chunk.rs](../crates/voidfs-core/src/chunk.rs)) cuts each shard off its
  buffer with `split_to`, so the next `extend_from_slice` finds the buffer shared and moves the
  rest of it, up to 16 MiB, to a new allocation. In a large upload that is a copy of up to 16 MiB
  for each shard of about 2 MiB; not timed. Found with item 2, where the shards kept those
  buffers alive in the cache. Cutting each shard out as a copy and advancing the buffer would
  copy each byte once.
- The commit that makes a checkpoint due (1,000 commits or 16 MiB of log since the last one)
  writes it *while holding the commit lock* (`Pool::checkpoint`). Every writer of that drive
  waits several round trips behind it. Since content-defined segments (E11), a checkpoint stores
  only the segments that changed, but it still encodes and hashes every row: for 600,000 rows,
  about 600 ms of CPU in a release build, of which 280–420 ms is copying the state out
  (`DriveState::rows`) and about 50 ms is hashing keys to cut segments. Its effect on commit
  latency has not been measured separately; it shows in p99, not p50.

**Rows it moves:** puts, overwrites and fan-out puts (9), multipart uploads (2), and every
edit's shard wave (24).

**Changes.**
1. **Small-file path** (checklist E9): store content under a threshold, say 4 KiB, inside the
   content descriptor instead of as a shard. A put of a tiny file is then one round trip. This
   **is a format change**:
   - a new extent kind carrying the bytes, beside `s` and `z`
     ([format §5](../spec/format.md#5-content-and-manifests));
   - an `incompatible` feature flag ([format §3.1](../spec/format.md#31-feature-flags));
   - rules for checkpoints and GC.

   It needs an RFC first ([CONTRIBUTING.md](../CONTRIBUTING.md)).
2. **Pipelined ingest:** keep reading and chunking while a bounded window of shard uploads (for
   example 16 shards or 32 MiB) is in flight. Profile `put 64 MiB` on loopback first, to see
   whether CPU (FastCDC and SHA-256) or waiting dominates.
3. **Checkpoints off the commit path:** write them from the installed `Arc<DriveState>` in a
   background task after releasing the lock. The format does not tie a checkpoint to the commit
   that triggered it ([format §8](../spec/format.md#8-checkpoints)). The commit lock also guards
   the drive's `Cadence`: the count towards the next checkpoint, and the last checkpoint's pages,
   which the next one lists without storing them again. A background writer needs those pages,
   and the rule that comes with them: the new index is written within 12 hours of the old one
   being seen (format §12.4, option 1).

**Expected effect.** With item 1, tiny puts at about 1–2 round trips (SpaceFS: 2× the bare
bucket); large puts within 1.1–1.2× of the bare bucket, as SpaceFS's are.

### Item 4. Patch: rewrite each touched shard once

**Problem.** `content::apply_edits` ([content.rs:273](../crates/voidfs-core/src/content.rs#L273))
applies a patch's edits one at a time as full `write_at`s (line 283). Each one re-chunks and
re-hashes the shard it lands in. Sixteen edits inside one 1 MiB file hash about 16 MiB instead
of 1 MiB.

**Evidence.** On loopback, patch 16 × 4 KiB in 1 MiB took 20–27 ms, against about 7 ms for a
single edit in the same file. That is 4.7–6.5× the bare bucket's download, change and upload.

**Rows it moves:** the three patch rows.

**Change.**
1. Group the edits by the extent range they touch, merging ranges that overlap after
   re-chunking.
2. Apply every edit to the fetched bytes of each range, then re-chunk each range once.
3. The existing property tests in `content.rs` compare against an in-memory model; add one that
   checks the grouped version gives the same extents as the edit-by-edit one.

### Item 5. Multipart completion

**Problem.** `complete_upload` ([object.rs:870](../crates/voidfs-server/src/s3/object.rs#L870))
makes a chain of round trips before it answers:
- it reads the upload record;
- it lists the parts and reads each part's record one after another (`parts_of`, line 856);
- it commits;
- it deletes the staging records (line 904).

That is about twenty sequential round trips for 16 parts.

**Evidence.** On par with the bare bucket on loopback. 1.6–1.9× slower at 12 ms (SpaceFS:
1.8–2.4× slower).

**Change.** Read the part records concurrently, or keep the part list in one record. Delete the
staging prefix after answering: [format §11](../spec/format.md#11-multipart-staging) only needs
it gone eventually, and it stays a GC root until then.

### Item 6. The read path for cold and large reads

**Problem.** These are observations, not yet measured, because the harness cannot yet measure
cold reads (item 7):
- A GET streams at most 8 shards at a time (object.rs:298). A cold 64 MiB read is four waves of
  2 MiB shard fetches.
- `Pool::shard` (pool.rs:436) fetches on every miss. Eight readers of the same cold object make
  eight fetches of each shard; nothing coalesces them.
- There is no read-ahead across requests, which the mount will need (checklist D6), and no disk
  tier (S5).

**Evidence.** SpaceFS's cache-cleared figures are the target:
- get 64 MiB cold: 299 ms against 769 bare;
- get 32 MiB cold: 179 ms;
- small cold reads at parity with the bucket.

With a warm cache voidfs already reads 64 MiB in 45–55 ms, 8 at once, as SpaceFS's warm 47 ms.

**Changes, after item 7 measures cold reads:**
- coalesce concurrent misses (moka's async cache has `try_get_with`, or a small in-flight map);
- raise or adapt the per-request shard window;
- add read-ahead;
- add the disk tier.

### Item 7. Measurement gaps to close first

1. **Cold reads.** voidfs-server cannot drop its caches, so the harness only measures warm
   reads. Add an operator-only way to drop them, for example on `SIGUSR1`. Something outside the
   S3 surface avoids a protocol change. Then give the harness a `--cold` mode that drops caches
   between setup and each round.
2. **The real run** in SpaceFS's setup (`client-host`), for the numbers the done-criterion is
   judged on. Everything above is local or on R2.
3. **CI against MinIO** (step 1's remaining bullet). MinIO runs on Linux, and
   `BENCH_S3=minio bench/scripts/local.sh --ops-scale 0.1` works there. A small emulated-distance
   run in CI (`BENCH_ONE_WAY_MS=4`) would catch regressions in items 1 and 2, which loopback
   hides.

## 3. How to compare a change

- **Build the candidate server, then run the same command twice for each build.** Between two
  identical runs the median row moved 8% in speed-up; rows near 10 ms moved by up to half.
  ```bash
  BENCH_SERVER_BIN=<path to the candidate voidfs-server> BENCH_ONE_WAY_MS=4 bench/scripts/local.sh
  ```
- **Loopback** hides every cost of talking to a bucket. Use it only for CPU-bound work: item 4
  and item 3's ingest.
- **Record the result** in `bench/results/` under a name that says what changed, and put the
  before and after in the pull request.

## 4. Baseline, row by row

The 12 ms run and the loopback run, against SpaceFS's published result. "Items" says which work
items above are expected to move the row.

| Scenario | voidfs, 12 ms (ms) | Bare, 12 ms (ms) | 12 ms result | Loopback result | SpaceFS result | Items |
|---|--:|--:|---|---|---|---|
| range 64 KiB of 64 MiB | 0.28 | 11.9 | 42× faster | 4.6× faster | 34× faster | 6 |
| get 4 KiB | 0.36 | 12.2 | 34× faster | 3.7× faster | 23× faster | 6 |
| move dir 200 × 64 KiB | 95.3 | 460 | 4.8× faster | 138× faster | 18× faster | 1 |
| get 64 MiB | 53.5 | 122 | 2.3× faster | 1.1× slower | 17× faster | 6 |
| stream get 256 MiB | 193 | 583 | 3.0× faster | 1.1× slower | 16× faster | 6 |
| stream get 64 MiB | 49.7 | 131 | 2.6× faster | 1.1× slower | 15× faster | 6 |
| append 4 KiB to 64 MiB | 92.4 | 296 | 3.2× faster | 22× faster | 15× faster | 1, 3 |
| head | 0.25 | 12.1 | 48× faster | 4.0× faster | 13× faster | – |
| truncate 4 KiB, end of 64 MiB | 91.7 | 287 | 3.1× faster | 22× faster | 13× faster | 1, 3 |
| get 32 MiB | 58.7 | 66.6 | 1.1× faster | 1.7× slower | 12× faster | 2, 6 |
| delete 4 KiB, middle of 64 MiB | 94.9 | 291 | 3.1× faster | 11× faster | 11× faster | 1, 3 |
| insert 4 KiB, middle of 64 MiB | 91.7 | 277 | 3.0× faster | 9.4× faster | 11× faster | 1, 3 |
| list 200 keys | 1.1 | 38.8 | 34× faster | 31× faster | 9.1× faster | – |
| append 4 KiB to 32 MiB | 91.2 | 146 | 1.6× faster | 12× faster | 8.0× faster | 1, 3 |
| rename 64 MiB | 95.9 | 141 | 1.5× faster | 44× faster | 7.9× faster | 1 |
| delete 4 KiB, start of 64 MiB | 89.9 | 266 | 3.0× faster | 13× faster | 6.9× faster | 1, 3 |
| insert 4 KiB, start of 64 MiB | 90.7 | 298 | 3.3× faster | 24× faster | 6.8× faster | 1, 3 |
| write at 4 KiB in 64 MiB | 97.9 | 288 | 2.9× faster | 17× faster | 6.2× faster | 1, 3 |
| fanout get 200 × 256 KiB, 32 at once | 13.4 | 12.5 | 1.1× slower | 1.1× slower | 6.2× faster | 2, 6 |
| truncate 4 KiB, end of 32 MiB | 91.5 | 138 | 1.5× faster | 8.2× faster | 5.9× faster | 1, 3 |
| fanout get 1000 × 4 KiB, 32 at once | 12.4 | 12.5 | 1.0× faster | 1.1× faster | 5.3× faster | 2, 6 |
| write at 4 KiB in 32 MiB | 95.3 | 142 | 1.5× faster | 7.2× faster | 5.1× faster | 1, 3 |
| delete 4 KiB, middle of 32 MiB | 94.9 | 149 | 1.6× faster | 6.2× faster | 4.8× faster | 1, 3 |
| delete 4 KiB, start of 32 MiB | 89.3 | 147 | 1.7× faster | 6.0× faster | 4.6× faster | 1, 3 |
| get 1 MiB | 1.5 | 13.4 | 9.1× faster | 1.0× faster | 4.5× faster | 2, 6 |
| insert 4 KiB, middle of 32 MiB | 91.5 | 141 | 1.5× faster | 6.1× faster | 3.8× faster | 1, 3 |
| insert 4 KiB, start of 32 MiB | 96.4 | 138 | 1.4× faster | 10× faster | 3.5× faster | 1, 3 |
| fanout get 1000 × 4 KiB, 64 at once | 12.4 | 12.0 | 1.0× slower | 1.0× faster | 2.6× faster | 2, 6 |
| patch 16 × 4 KiB in 64 MiB | 192 | 287 | 1.5× faster | 1.5× faster | 2.1× faster | 1, 3, 4 |
| patch 16 × 4 KiB in 32 MiB | 145 | 135 | 1.1× slower | 1.0× slower | 1.4× faster | 1, 3, 4 |
| append 4 KiB to 1 MiB | 92.6 | 27.3 | 3.4× slower | 1.7× slower | 1.0× faster | 1, 3 |
| put 64 MiB | 378 | 168 | 2.2× slower | 2.0× slower | 1.1× slower | 1, 3 |
| put 32 MiB | 188 | 90.3 | 2.1× slower | 1.9× slower | 1.2× slower | 1, 3 |
| delete 4 KiB, middle of 1 MiB | 93.4 | 26.8 | 3.5× slower | 1.7× slower | 1.4× slower | 1, 3 |
| insert 4 KiB, middle of 1 MiB | 92.5 | 27.5 | 3.4× slower | 2.2× slower | 1.4× slower | 1, 3 |
| patch 16 × 4 KiB in 1 MiB | 91.2 | 26.4 | 3.5× slower | 4.7× slower | 1.5× slower | 1, 3, 4 |
| delete 4 KiB, start of 1 MiB | 93.4 | 26.4 | 3.5× slower | 1.6× slower | 1.5× slower | 1, 3 |
| write at 4 KiB in 1 MiB | 93.5 | 26.3 | 3.6× slower | 1.6× slower | 1.6× slower | 1, 3 |
| insert 4 KiB, start of 1 MiB | 96.8 | 27.0 | 3.6× slower | 1.7× slower | 1.7× slower | 1, 3 |
| put 1 MiB | 85.4 | 13.4 | 6.4× slower | 2.6× slower | 1.7× slower | 1, 3 |
| multipart put 256 MiB × 16 MiB | 1,212 | 625 | 1.9× slower | 1.2× faster | 1.8× slower | 1, 3, 5 |
| overwrite 1 MiB | 85.9 | 13.5 | 6.3× slower | 1.7× slower | 1.9× slower | 1, 3 |
| put 4 KiB | 104 | 12.7 | 8.2× slower | 3.0× slower | 2.0× slower | 1, 3 |
| truncate 4 KiB, end of 1 MiB | 93.6 | 27.2 | 3.4× slower | 1.6× slower | 2.1× slower | 1, 3 |
| fanout put 1000 × 4 KiB, 32 at once | 412 | 12.1 | 34× slower | 4.2× slower | 2.2× slower | 1, 3 |
| fanout put 1000 × 4 KiB, 64 at once | 859 | 12.3 | 70× slower | 4.0× slower | 2.4× slower | 1, 3 |
| multipart put 64 MiB × 8 MiB | 408 | 254 | 1.6× slower | 1.0× faster | 2.4× slower | 1, 3, 5 |
| fanout put 200 × 256 KiB, 32 at once | 368 | 13.0 | 28× slower | 4.5× slower | 2.8× slower | 1, 3 |
| overwrite 4 KiB | 101 | 11.8 | 8.6× slower | 2.5× slower | 3.1× slower | 1, 3 |

The emulated distance adds latency but no bandwidth limit, so the bare bucket's large transfers
are far faster here than S3's. The large-read rows (get and stream get 32–256 MiB) will only be
judged fairly by the real run.
