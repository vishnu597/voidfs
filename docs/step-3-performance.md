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
- Rename at 8 at once took two round trips: after an entry landed, the requests it had answered
  waited for the next entry and then their own. **Done** with the hold below (30 September 2026).
- Planning the next batch while an entry is in flight saves only CPU, a small part of a round
  trip here, so it was not done.

**Follow-up (30 September 2026): a hold before each entry.** Measured in
[bench/results/group-commit-hold](../bench/results/group-commit-hold/README.md), relative to the
bare bucket in the same run:
- **At 12 ms, eight to 64 at once, a small write takes one round trip instead of two.** Put
  4 KiB 2.18× to 1.20×, overwrite 4 KiB 2.19× to 1.15×, the fan-out puts of 4 KiB at 32 and 64
  2.46× and 2.48× to 1.39× and 1.47× (focused runs, the median of four). Rename 64 MiB and the
  folder move take 13.9 and 13.8 ms, from 27.3 and 27.0. Put 4 KiB takes 14.0–14.3 ms at 2, 4 and
  8 at once, from 26.5–27.1, and 13.5 ms alone either way.
- **Over the 49 rows at 12 ms**, the geometric mean of the p50 ratios against `main` is 0.932
  (writes 0.830, metadata 0.747, edits 0.990, reads 1.002). 25–26 rows are at or ahead of
  SpaceFS's ratio (`main` 22 in the same session), and the geometric mean speed-up over the bare
  bucket is 2.9× (`main` 2.7×, SpaceFS 2.8×).
- **Nothing else moved that the focused runs confirm, but list 200 keys:** 1.0 to 1.1 ms at
  12 ms (+18% relative to bare in every pair, +11% with holding off and on in one binary), still
  31–38× faster than the bare bucket. Not explained: listing the drive's state takes the same
  30 µs however its puts were batched. Edits in 1 MiB files moved −3% to +6%, not the same way in
  each pair.
- **On loopback nothing is held**, since a log entry takes about 0.3 ms: over the 49 rows the
  geometric mean is 0.975 relative to bare, 33–35 rows are at or ahead of SpaceFS's ratio (`main`
  31–35), and the rows that looked slower moved both ways in a second focused set. Put 4 KiB was
  2–4% slower than `main`'s in every pair, but not with holding off and on in one binary.

What was built, in `Pool::drain` ([pool.rs](../crates/voidfs-server/src/pool.rs)):
- **The hold.** After an entry lands, the next waits until as many mutations have queued since as
  the entry answered, or a whole batch is waiting, for at most a quarter of the entry's write time
  and 2 ms. A client alone never waits: the entry after its last waits for its next write. The
  timer is set a tick early, since tokio's fire up to 1 ms late.
- **Not after an entry written in under 4 ms** (loopback, a bucket on the same machine): the
  bound would be under a timer tick. A spin of `yield_now` in its place caught nothing on
  loopback (log entries per put unchanged) and kept a worker busy.
- **Only while it pays.** The drive keeps a decayed tally of how many mutations queued within
  each entry's bound against how many it answered, held or not, and holds while that is at least
  a half. Edits, which upload a shard before they commit, and requests that arrive at their own
  pace, do not come back in time, so their entries are not held. In exploratory runs, holding
  every time made seven of the eight edits in 1 MiB files 8–19% slower, and deciding from the
  last entry alone −3% to +9%.
- **Not under the commit lock**, so forks, hard deletes and a checkpoint's swap get it between
  entries as before. The tally is on the drive, not in `Cadence` (which a checkpoint resets), since
  the draining task ends whenever the queue empties.
- `voidfs_commit_hold_seconds` reports each hold.

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

**Status (29 September 2026): done.** Change 1, small files held in the log, implements
[RFC 0003](../rfcs/0003-small-content-in-descriptors.md), measured in
[bench/results/small-content](../bench/results/small-content/README.md):
- **One at a time, a small write takes one round trip.** At 12 ms, put and overwrite 4 KiB take
  14.9 ms where `main` takes 29.0 (1.04× the bare bucket, from 2.03×). The server writes only its
  share of a log entry: 0.25 bucket requests per put 4 KiB (from 1.30), 0.06 and 0.03 per fan-out
  put (from 1.08 and 1.04).
- **Eight to 64 at once, still two round trips.** At 12 ms, relative to the bare bucket: put
  4 KiB 2.37× to 2.12× (SpaceFS 2.01×), overwrite 2.57× to 2.16× (SpaceFS 3.05×), the fan-out
  puts at 32 and 64 3.05× to 2.24× and 2.28× (SpaceFS 2.23× and 2.41×). Group commit, not the
  write: while a log entry is in flight, the requests the last one answered wait for it and then
  for their own, and two at once already take two round trips. A diagnostic build that holds an
  entry until those requests are back (at most 2 ms) took the four rows to 1.1–1.4× and rename
  64 MiB from 26 ms to 13; it is not in this change, but in item 1's follow-up.
- **On loopback**, where round trips cost little: put 4 KiB 1.98× to 1.15×, overwrite 1.72× to
  0.91×, the fan-out puts 1.70× and 1.84× to 0.65× and 0.57×.
- **Over the 49 rows**, relative to the bare bucket: at 12 ms the geometric mean of the p50 ratios
  against `main` is 0.957 (writes 0.883), and 24–26 rows are at or ahead of SpaceFS's ratio (`main`
  22–23 in the same session); on loopback 0.938 (writes 0.756), 34–35 rows (`main` 34–36), and 31–32
  rows faster than the bare bucket (`main` 28).
- **Checkpoints store the small files as shards** in the background. Over 20,000 puts of 4 KiB
  at 12 ms, one comes every ~2,870 puts (16 MiB of log) and takes 1.25 s, uploading ~2,870 shards
  32 at a time; foreground writes did better than `main`'s: p99 34.5–34.8 ms (48.5–49.1), p99.9
  41–43 (51–52), slowest 44–46 (57–64).
- **Found on the way:** every mutation started a task to drain the drive's queue, each of which
  wrote a batch when its turn at the commit lock came, so the line of them grew under steady
  writes. Anything else that needed the lock (a fork, a hard delete, a checkpoint's swap) waited
  1.5 s, then 9, then 56 as it went on. One task drains a drive's queue now.

Changes 2 and 3, measured in [bench/results/write-round-trips](../bench/results/write-round-trips/README.md):
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
  - Forks and hard deletes take the checkpoint lock and then the commit lock (the commit lock
    first until change 1, whose checkpoints take it at their end), so they wait for one being
    written: a fork starts from it, and nothing of a deleted drive is written after it has gone.
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

What change 1 built:
- **Data extents** (`Extent::Data` in `voidfs-core`, format §5): up to 4,096 bytes in a
  descriptor, parsed and checked wherever descriptors are (never in manifest pages or part
  records), with the ETag of the shard extent holding the same bytes. `Extent` is no longer
  `Copy`; its bytes are a `Bytes`, so a clone is a reference count. Edits treat a data extent as a
  shard whose bytes are in hand.
- **The writer**, only in a pool that lists `inline-data`: a put reads up to 4,097 bytes before it
  cuts anything, and a body of at most 4,096 is held in one data extent, checked against its
  signature and checksums before the commit, with no shard. An edit whose result is at most
  4,096 bytes is held the same way; a larger result, and any result in a pool without the
  feature, keeps no data extents. Copies keep the descriptor; multipart parts never hold one.
- **Turning it on:** `--new-pool-feature inline-data` for a pool the server creates, and
  `voidfs-server pool enable inline-data` for an existing one, once every server that writes it
  is upgraded. Servers read the flag when they start, as the RFC requires.
- **Spilling** (`Pool::write_checkpoint`): each distinct small content goes to a shard through
  the garbage-collection guard, which now keeps 32 uploads in flight, not all of them at once;
  rows list shard extents. Once the index is written, the drive's state takes the spilled
  descriptors in place of those it still holds (`DriveState::with_spilled`), under the commit
  lock for 14 ms on average for ~2,870 rows, so the state holds no more bytes in data extents
  than its log since the last checkpoint: without that, memory grew about 5 KB a small file
  more. A fork's first checkpoint spills too, and only the fork's state takes the result: only a
  drive's own checkpoints vouch for its content.

What was left for later:
- A short hold before a log entry that follows another, until the requests the last one
  answered are back, would take small writes, fan-out puts and renames from two round trips to
  one at 8 to 64 at once (the diagnostic above). **Done** (30 September 2026, item 1's
  follow-up, [results](../bench/results/group-commit-hold/README.md)): at 12 ms, put and overwrite
  4 KiB take 1.20× and 1.15× the bare bucket's time eight at once, the fan-out puts 1.39× and
  1.47×, all ahead of SpaceFS's ratios, and put 4 KiB takes one round trip from 2 to 8 at once.
  On loopback, where a hold would cost more than an entry, entries are not held.
- A drive's state takes about 26 KB of memory per small file, on `main` too (100,000 files:
  2.6–2.8 GB with a 16 MiB shard cache). Not investigated.
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

**Status (30 September 2026): done.** Measured in
[bench/results/patch-once](../bench/results/patch-once/README.md):
- **Patch in 1 MiB takes what one edit takes.** On loopback, 5.1 ms where `main` takes 18.2
  (1.33× the bare bucket, from 4.46×; write at 4 KiB in the same file: 5.0 ms). At 12 ms, 1.34×
  from 1.71× (SpaceFS 1.45×), ahead of SpaceFS's ratio in all four focused runs.
- **Patch in 32 MiB** at 12 ms: 0.69× from 0.81× (SpaceFS 0.70×), at or ahead in all four
  focused runs. On loopback, where it uploads 12 shards to a local disk, it moved within the
  spread.
- **Patch in 64 MiB** did not move in the focused runs (0.51×, SpaceFS 0.47×): its edits, 4 MiB
  apart, rarely share a shard. Its CPU is now each touched shard chunked and hashed once, about
  29 ms a patch.
- The profile of `main` found 98% of the server's CPU in this scenario in `apply_edits`: SHA-256
  and FastCDC in equal parts. The requests to the bucket did not change: `main` already fetched a
  patch's shards in one go, and uploaded only those the result keeps.
- Over the 49 rows, relative to the bare bucket, the geometric mean against `main` is 0.987 at
  12 ms and 0.967 on loopback. The rows that looked slower in the full runs do not patch. Run
  focused again, and in one diagnostic binary switched between the old patch and the new, they
  moved as much with no change at all.

What was built, in `content::apply_edits` ([content.rs](../crates/voidfs-core/src/content.rs)):
- **Groups.** Each edit's range is widened to the whole shards and data extents it touches (in a
  zero run, only its own bytes). Edits whose widened ranges share a byte are one group; ranges
  that only meet at a boundary stay apart, as they would edit by edit.
- **Once per group.** A group's bytes are read once, every edit in it is applied in order (later
  ones win), and they are chunked once. The new shards are hashed once, at the end.
- **The edges as `splice` has them.** A group shorter than the minimum absorbs the extent before
  it; while its last shard is short it pulls in up to two following extents, or the next group
  whole, which may then pull in two of its own. A pull re-cuts only from the short shard's start.
- **The same fetch.** Every shard this reads is one `needed_shards` names for an edit's range,
  which `run_edit` fetches in one go before it (a property test reads only from those).
- **The same bytes and size.** The extents, and so the ETag, are those edit by edit gives when no
  edit's neighbourhood overlaps another's, and were in every in-file patch tried (565 in process).
  Pages written beside each other into a zero run can come out with other boundaries, since edit
  by edit they depend on the order the pages come in: 1–5% of such patches tried, each with one
  shard fewer.

Left for later:
- Hashing a patch's new shards in parallel, and off the async workers: in 64 MiB, about 16 shards
  of about 2 MiB are chunked and hashed one after another on the request's task.

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

**Status (30 September 2026): done.** Measured in
[bench/results/multipart-complete](../bench/results/multipart-complete/README.md):
- **Completion answers in two round trips.** At 12 ms, 32 ms at the median, where it took 157 ms
  for 8 parts and 258 ms for 16: one round trip reads the upload's record and every part's
  together, one commits. The staging records are deleted after answering.
- **The uploads' own times fell** in every focused pair at 12 ms: multipart put 64 MiB × 8 MiB
  from 343 to 254 ms (−26%), 256 MiB × 16 MiB from 1,147 to 1,008 ms (−12%). Relative to the bare
  bucket, whose multipart times varied 2× within the session, the full runs put them at 0.74× and
  1.50× the bare bucket's time (64 MiB) and 1.50× and 0.80× (256 MiB), against `main`'s 1.45× and
  1.70×, and 1.58× and 1.25×. Both builds are ahead of SpaceFS's 2.44× and 1.79× in every run.
- In one binary switched between the two paths, multipart put 64 MiB took 23% less time, and
  256 MiB only 2%: in those runs the local disk had slowed down, and eight uploads of 256 MiB at
  once waited for it, not for completion's round trips.
- On loopback, where the round trips cost little, nothing moved beyond the spread.
- Over the 49 rows, relative to the bare bucket, the geometric mean against `main` is 0.967 at
  12 ms and 1.015 on loopback. The rows that looked slower in the full runs do not run this code,
  and run focused again were not.
- **LIST per upload: 1 to 0.** GETs per upload are unchanged (17 and 33): each part still reads
  `upload.json`, now while its body is read, and completion still reads each part's record, now all
  at once.

What was built, in [object.rs](../crates/voidfs-server/src/s3/object.rs):
- Completion reads `upload.json` and the listed parts' records by name, together (at most 32 at
  a time), and lists nothing. The errors keep their order: the upload's, the body's, then the
  parts' as listed.
- It commits, answers, and then deletes the staging prefix, retrying for about 40 seconds; at
  shutdown, what is left is deleted once more.
- A claim per upload, in the server's memory, says it is completed until its records are gone:
  completions and aborts of one upload take turns, so racing or retried completions commit once,
  and ListMultipartUploads, ListParts, UploadPart and aborts see the upload as gone. The completion
  runs in a task of its own, so a client that goes away cannot leave it committed and still open.
- UploadPart looks its upload up while it reads the part, and answers NoSuchUpload as soon as the
  lookup does. A part whose upload is completed meanwhile is refused before its record is written.
- The on-bucket format, and what GC counts as roots, are unchanged.

Left for later:
- The claim is per server: two servers completing one upload at once can still both commit, and
  another server sees a completed upload open until its records are deleted. Closing that across
  servers needs the upload's id in the commit, a format change.
- A part whose record lands after its upload's staging prefix was deleted leaves an orphan record
  (now only when it lands within one round trip of the completion). Records with no `upload.json`
  are never aborted by GC, and stay roots.
- The GETs per part and per record: keeping the part list in one record, or `upload.json` in
  memory, would remove them, but the first changes the format and the second would miss aborts by
  another server or by `voidfs-server gc`.

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

**Status (1 October 2026): coalescing and a shared read-ahead budget done; ranged shard reads
designed, not built.** Measured in [bench/results/shard-fetch](../bench/results/shard-fetch/README.md):
- **Concurrent misses of a shard make one bucket GET** (`Pool::read`): the fetch runs on a task of
  its own that every reader waits for, so a reader that goes away leaves it to the others; a
  failure reaches every reader and is not kept; the hash check and the guard's view stay with the
  fetch. A wave of eight cold 64 MiB reads makes 3.6 GETs per read instead of 24.
- **A GET reads up to 32 shards ahead, borrowing past 8 from a budget of 32 that all GETs share.**
  Alone, a cold 64 MiB read with the cap takes 110 ms instead of 215; many at once read as with 8
  each, at most 15% more memory. A fixed window of 32 cost many concurrent reads 17–40% more memory
  and, without the cap's total, up to 2.6× the time.
- **Cold, against SpaceFS's cache-cleared figures,** as a fraction of the bare bucket's time,
  `main` → this branch, A B B A B A A B:

  | Row | Capped (`s3`) | Capped, no total | No cap | SpaceFS |
  |---|--:|--:|--:|--:|
  | get 32 MiB | 0.71–0.76 → 0.19–0.31 | 0.31–0.36 → 0.24–0.30 | 1.18–1.32 → 0.66–0.68 | 0.50 |
  | get 64 MiB | 0.71–0.73 → 0.18–0.24 | 0.27–0.31 → 0.14–0.15 | 1.17–1.24 → 0.55–0.56 | 0.38 |

  Both are ahead of SpaceFS's in every run with the cap, and without its total. Without a cap the
  bare bucket reads 64 MiB in 134 ms rather than S3's 779, so the ratio does not compare. The small
  cold rows are as they were, and ahead.
- **Pages are checked against their hashes** when fetched, as shards are, and in garbage
  collection's marking: a checkpoint segment that parsed but was not what had been written loaded a
  drive in a state it never had, and a manifest page that lied would have hidden live shards from
  the marking.
- **Warm, the three large reads behind SpaceFS with the cap are where they were** (0.78–0.94 of
  their ratio): served from memory, at this Mac's loopback limit. versitygw, serving the same reads
  from the page cache, is 6–12% faster than voidfs, and the server's CPU is the kernel's
  copy into the sockets. Nothing cheap was found to fix; the real run will judge them.
- With the cap, 45 of the 49 rows are at or ahead of SpaceFS's ratio in both runs (46 before):
  the same three, and the range read, which sits on SpaceFS's ratio within a tenth of a millisecond.
  Warm without the cap, the geometric mean against `main` is 0.980.

**What is left:** ranged shard reads, which need a decision first (they either weaken the hash
check or need block hashes, a format change; the options and their costs are in the results);
read-ahead across requests for the mount (D6); the disk tier (S5); and the real run, cold too, to
settle S3's download total.

**Problem.** Measured cold since 1 October (item 7.1), with the bucket 12 ms away and capped as
S3 was in SpaceFS's run (item 7.4):
- A GET streams at most 8 shards at a time (`.buffered(8)` in object.rs). A cold 64 MiB read is
  about 25 shard fetches of 2–3 MiB, 8 in flight.
- `Pool::shard` fetches on every miss. Eight readers of the same cold object make eight fetches
  of each shard; nothing coalesces them.
- A cold range fetches the whole shard it falls in.
- There is no read-ahead across requests, which the mount will need (checklist D6), and no disk
  tier (S5).

**Evidence** ([bench/results/cold-reads](../bench/results/cold-reads/README.md)), voidfs's time
as a fraction of the bare bucket's, cold, two runs, against SpaceFS's cache-cleared figures:

| Row | Capped (`s3`) | Capped, no total | No cap | SpaceFS |
|---|--:|--:|--:|--:|
| get 4 KiB (`inline-data`; without it) | 0.02 (0.96–0.98) | 0.02 | 0.02 | 1.08 |
| get 1 MiB | 1.03–1.04 | 1.03–1.04 | 1.04–1.05 | 1.18 |
| get 32 MiB | 0.72–0.74 | 0.32–0.39 | 1.16–1.18 | 0.50 |
| get 64 MiB | 0.69–0.71 | 0.29–0.30 | 1.12–1.16 | 0.38 |
| range 64 KiB of 64 MiB | 2.77–2.98 | 3.04–3.32 | 1.25–1.29 | – |

- **Small cold reads are ahead of SpaceFS's:** one shard GET, or none for a file in the log.
- **Large cold reads are behind with the cap and ahead without its total.** Eight parallel shard
  fetches beat S3's per-stream limit, but each of a wave's eight readers fetches every shard,
  8 × 64 MiB = 537 MB, and at the cap's 1,000 MB/s total that takes the 506–517 ms measured. The
  total is the cap's assumption for downloads (item 7.4).
- **A cold range is about 3× the bare bucket's time:** about 2.5 MiB fetched for 64 KiB.
- With a warm cache voidfs reads 64 MiB in 45–55 ms, 8 at once, as SpaceFS's warm 47 ms.

**Changes, in this order:**
- coalesce concurrent misses (moka's async cache has `try_get_with`, or a small in-flight map):
  537 MB per wave of get 64 MiB becomes 67, and the estimate is about 0.15× the bare bucket's
  time with the cap. **Done:** an in-flight map, 0.18–0.24× with the window below;
- fetch only the bytes a range needs from its shard (a ranged GET of the shard object). **Designed,
  not built:** it weakens the hash check or needs a format change;
- raise or adapt the per-request shard window, which bounds one read once reads are coalesced.
  **Done:** adapted, from a budget all GETs share;
- add read-ahead;
- add the disk tier.

### Item 7. Measurement gaps to close first

**Status (1 October 2026): 7.1 and 7.4 done.** Measured in
[bench/results/cold-reads](../bench/results/cold-reads/README.md):
- **Cold reads** (7.1): `SIGUSR1` empties voidfs-server's shard and page caches, and the
  harness's `--cold` (`BENCH_COLD=1`) drops them before every wave of operations, so that every
  measured read starts cold. The four rows SpaceFS gives cold figures for: get 4 KiB and 1 MiB
  ahead of SpaceFS's ratio, get 32 and 64 MiB behind with the cap (0.69–0.74× the bare bucket's
  time, SpaceFS 0.38× and 0.50×) and ahead without its total (0.29–0.39×). Item 6 has the detail.
- **A bandwidth cap** (7.4): `BENCH_BANDWIDTH=s3` caps the relay at 95 MB/s down and 68 MB/s up
  per connection, and 1,000 MB/s in all each way, fitted to SpaceFS's bare bucket: within −7% to
  +13% on the rows that move the most data. With it, warm, **46 of the 49 rows are at or ahead
  of SpaceFS's ratio** in both runs, where 27–28 are without it. The three behind are warm large
  reads (get 64 MiB, stream get 64 and 256 MiB: 0.79–0.91 of SpaceFS's ratio), which voidfs
  serves from memory in 50–57 ms for 64 MiB, eight at once, against SpaceFS's 45–47.
- The server's change costs nothing warm: over the 49 rows at 12 ms without the cap, the
  geometric mean against `main` is 0.999, and the rows that looked slower were not when run
  focused again.

1. **Cold reads. Done.** voidfs-server could not drop its caches, so the harness only measured
   warm reads.
   - `SIGUSR1` calls `Pool::drop_caches`, which empties the shard cache and the page cache and
     counts it in `voidfs_cache_drops_total`. It is outside the S3 surface and the
     unauthenticated admin port: only someone who may signal the process can drop the caches.
     Each drive's state (with `inline-data`, small files' bytes too), the guard's record of
     stored shards and the connections to the bucket stay.
   - `--cold` runs each round in waves of one operation per worker, and drops voidfs's caches
     before each of its waves, confirmed from the server's metrics; the bare target runs the same
     waves. A get row's eight readers share one object, so dropping once per round would have
     left all but the first eight reads warm. SpaceFS does not say when it cleared its cache.
   - Cold runs are scored against SpaceFS's cache-cleared figures (`Published::layer_cold`).
2. **The real run** in SpaceFS's setup (`client-host`), for the numbers the done-criterion is
   judged on. Everything above is local or on R2. A cold run there
   (`BENCH_COLD=1 BENCH_TOPOLOGY=client-host bench/scripts/cloud-run.sh`) would also settle the
   download total that 7.4 could only assume.
3. **CI against MinIO** (step 1's remaining bullet). MinIO runs on Linux, and
   `BENCH_S3=minio bench/scripts/local.sh --ops-scale 0.1` works there. A small emulated-distance
   run in CI (`BENCH_ONE_WAY_MS=4`) would catch regressions in items 1 and 2, which loopback
   hides.
4. **A bucket with S3's bandwidth. Done.** The relay that emulates the distance added no
   bandwidth limit, so the bare bucket read 64 MiB in about 120 ms where S3 took 779 ms in
   SpaceFS's run, and a 64 MiB edit's download and upload took about 290 ms where they took
   1,690. The large reads and the edits in 32 and 64 MiB files could not be judged.
   - `voidfs-bench delay --bandwidth` (`BENCH_BANDWIDTH` in `local.sh`, off by default) sends
     each chunk once a link at the rate would have carried it, on top of the delay: a link per
     connection each way, and one shared by all connections each way, for voidfs-server's
     traffic and the bare target's alike.
   - `s3` is 95 MB/s down and 68 MB/s up per connection, and 1,000 MB/s in all each way, fitted
     to SpaceFS's bare-bucket figures for get and stream get, put 32 and 64 MiB, and the
     multipart uploads; the edits in 32 and 64 MiB files, not used to fit it, land within −1% to
     +5%. Not confirmed: the total for downloads, which no bare row of theirs reaches, and what
     limited the multipart uploads.
   - It does not emulate S3's time per request: small requests and server-side copies stay
     faster here than on S3.

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
