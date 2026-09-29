# voidfs and SpaceFS: parity status and plan

*Stocktake of 2026-09-28, brought up to date the same day after content-defined checkpoints, the
capability probe, group commit and the shard cache's admission, and on 2026-09-29 after
virtual-host addressing; the first was taken on 2026-09-27.*

Sources:
- the voidfs code on `main` at `0ff3f2d` (garbage collection, content-defined checkpoints, the
  capability probe, group commit and the shard cache's admission merged), with virtual-host
  addressing on top;
- the parity checklist in [§3 of the plan](RESEARCH_AND_PLAN.md#3-parity-checklist-everything-to-build);
- the benchmark results in [`bench/results/`](../bench/results/);
- SpaceFS's benchmark pages (runs of 20 and 23 September 2026) and changelog, read again on 28
  September: no release since 22 September, and the same benchmark runs;
- the SpaceFS macOS app 0.2.300, installed on the same Mac.

The goal is feature, functionality and performance parity with SpaceFS, as open source that people
host themselves on their own bucket. Where voidfs differs on purpose (no hosted offering, so no
billing or plans), this page says so.

## 1. Summary

- **The foundation is built.** The storage engine and the S3 layer are the parts that are hardest
  to get right, and they are largely there:
  - versioned, content-chunked storage;
  - in-place edits and O(1) renames;
  - forks, point-in-time reads and restore;
  - a change feed;
  - garbage collection, whose protocol is model-checked
    ([RFC 0002](../rfcs/0002-gc-safe-against-writers.md));
  - 35 conformance cases passing on local disk and Cloudflare R2.
- **Almost everything a person touches is missing:** a writable mount, a CLI, accounts and a web
  app, SDKs, search and share links.
- **Performance is measured locally, not yet in SpaceFS's setup.** SpaceFS publishes 49
  benchmark scenarios; [`bench/`](../bench/README.md) runs all of them against voidfs and the bare
  bucket underneath it.
  - In the closest local emulation (the bucket 12 ms away), voidfs is at least as far ahead of
    the bare bucket as SpaceFS on 21–22 of the 49 rows (§6), up from 16–19 with group commit
    alone and 7 before it.
  - Group commit removed the cause that held back 35 rows: a drive committed one mutation per
    bucket round trip. The shard cache now keeps what was written and read last, which put the
    fan-out gets far ahead. What holds back most of the other 27–28 is that a write still takes
    its shards, then its log entry, one after the other; then patch, and large reads that only
    the real run can judge.
- **SpaceFS's Mac app is now understood** (§5). It is a native FSKit module with its core in Rust,
  running in a separate daemon, which is the architecture the FSKit spike chose for voidfs. It
  also shows that the FSKit entitlement can ship with Developer ID.
- **The plan (§7):**
  - Step 1 has its harness, local results and CI. Still to do: the run in SpaceFS's setup
    (which waits on cloud accounts, §8), and the Mac comparison.
  - Step 2 has five of its six items done: garbage collection, content-defined checkpoint
    segments, the bucket capability probe, runs on AWS S3, MinIO and rclone, and virtual-host
    addressing. The Compose file is done too; a health endpoint and metrics are to do.
  - Step 3 has its first two items done: group commit and the shard cache's admission.
  - Steps 4–10 have not started.

## 2. Decisions that shape the plan

| Decision | Choice | Notes |
|---|---|---|
| Platform order | **Full Mac parity first** (steps 1–8), then Linux (step 9) | Full Mac parity includes search, previews and video review. Linux servers use S3 and the SDKs until the Linux mount lands |
| Minimum macOS | **macOS 27** (decided 2026-09-27) | SpaceFS states macOS 26.4 as its minimum; its app bundles declare 26.0. voidfs gives up macOS 26 because only macOS 27 lets a module evict the kernel's caches when another machine changes a file ([spike §5](spikes/fskit.md#5-kernel-caching-and-coherence)) |
| Mac architecture | Per-user agent running the Rust core, thin FSKit extension | [Spike §4.1](spikes/fskit.md#41-where-should-the-rust-client-core-run). SpaceFS made the same split (§5) |
| Hosting | Self-hosted only | No billing, plans or trials to match. The hosted edge network is matched by deploying near users (step 10) |

## 3. Scorecard

Every item of the plan's checklist (§3, 65 items, including the Finder integration item added on
2026-09-27), scored against the code:

| Area | Done | Partly | Missing | State |
|---|---|---|---|---|
| Engine (E1–E13) | 8 | 1 | 4 | Chunking, versions, point-in-time reads, restore, in-place edits, forks, checkpoints and garbage collection work. Missing: the small-file path, direct uploads, encryption, retention policies |
| Storage backends (B1–B8) | 1 | 1 | 6 | Local disk, R2 and versitygw (the local S3 server the benchmarks use) work. Not yet run on AWS S3 or MinIO. A capability probe checks the bucket at start. No short-lived storage credentials, no adopt or export |
| Server (S1–S9) | 3 | 3 | 3 | Full S3 subset, path and virtual-host addressing, extensions and change feed, on one node. Missing: a disk cache tier, several nodes, several regions, quotas |
| Accounts and web (C1–C10) | 0 | 1 | 9 | Static keys from command-line flags only |
| Clients (D1–D12) | 0 | 3 | 9 | A read-only macOS mount (the spike). No agent, journal, CLI, Finder integration, Linux or Windows |
| SDKs, agents, search (A1–A7) | 0 | 0 | 7 | Stock S3 SDKs and the AWS CLI work; nothing voidfs-specific |
| Operations (O1–O6) | 0 | 2 | 4 | One binary. A benchmark harness, not yet run in the cloud. No compose file or metrics |
| **Total** | **12** | **11** | **42** | Of the 28 P0 items: 12 done, 8 partly, 8 missing |

Earlier stocktakes counted 71 items and 6 more missing than the rows add up to; the checklist has
65. The P0 counts were right.

"Partly" means:
- E1 has no compression.
- B1 is not yet run on AWS S3 or MinIO.
- S3 lacks direct upload and credentials; S5 is memory-only; S8 has no operations catalogue.
- C3 keys can't be minted or revoked.
- D3 is read-only, D6 relies on the kernel's read-ahead only, and D11 is the spike's shell.
- O1 has no compose file. O5 has run locally and against R2, not yet in SpaceFS's setup.

The 28 P0 items, which a credible v1 needs:

| Status | Items |
|---|---|
| Done (12) | E2 format spec, E3 namespace, E4 versions, E5 edits, E6 commit protocol, E7 forks, E8 garbage collection, E11 checkpoints, B3 capability probe, S1 S3 server, S2 S3 subset, S4 per-drive authority and change feed |
| Partly (8) | E1 chunking, B1 backends, S3 extensions, S5 shard cache, S8 conformance and catalogue, C3 access keys, D3 macOS mount, O1 single binary and compose |
| Missing (8) | B5 stored bucket credentials, C1 sign-in, C2 workspaces, C4 bucket connections, D1 client daemon, D5 desktop semantics, D9 CLI, A1 Rust and TypeScript SDKs |

## 4. Product by product

| SpaceFS | voidfs today |
|---|---|
| S3-compatible endpoint with extensions (protocol v1) | Protocol 1 with 35 conformance cases, path or virtual-host addressing. Missing: direct uploads, storage credentials |
| macOS app: writable mount, transfers (pause, resume, speed limits), previews, video reviews, Space Search, Finder badges | Read-only FSKit mount and a menu-bar shell |
| `spacefs` CLI and mount daemon (macOS and Linux): login, whoami, drives, drive, workspace, use, keys, mount, unmount, mounts, uploads, upload, status, daemon, history, show, restore, version | None |
| Linux FUSE mount | None (step 9) |
| Web app: sign-in, settings, access keys, billing | None (billing does not apply) |
| SDKs: TypeScript, Python, Go, Rust (MIT) | None |
| Agent docs: `llms.txt`, `llms-full.txt`, `operations.json` | None |
| Public and restricted share links | None |
| Passwordless email sign-in, workspaces, scoped keys with drive allowlists, revocation within a minute | Static scoped keys; the allowlist exists in the code but cannot be set |
| Enterprise: SSO and SAML, audit, retention controls, private cloud, bring your own storage | Bring your own storage is the default. The rest is missing |
| Windows: "coming soon" | Neither has it |

## 5. What SpaceFS's Mac app is made of

Read from the installed app (`/Applications/Space/Space.app`, version 0.2.300) on 2026-09-27:
bundle metadata, entitlements and linked libraries only.

| Part | What it is |
|---|---|
| `Space` | The SwiftUI menu-bar app (`com.spacefs.launcher`). It links FSKit, ServiceManagement, SQLite, QuickLook thumbnailing, AVKit and Vision |
| `Extensions/SpaceFSExtension.appex` | **An FSKit module** (`com.apple.fskit.fsmodule`, short name `SpaceFS`). It uses `FSPathURLResource` with security-scoped URLs, not a generic URL. It is sandboxed with `network.client` and the App Group `group.com.spacefs.launcher` |
| `MacOS/spacefs-fskitd` (and `spacefs`, a symlink to it) | **A Rust binary** (tokio, hyper, `aws-sdk-s3`) that is both the CLI and the mount daemon |
| `MacOS/SpaceMountHelper` + `Library/LaunchDaemons/…MountHelper.plist` | A privileged LaunchDaemon (registered through ServiceManagement) with an XPC service. It has no FSKit mount entitlement, so it presumably mounts through `mount(8)` as root, into `/Volumes` |
| `PlugIns/SpaceFinderSync.appex` | A Finder Sync extension: badges and context menus in Finder |
| `Resources/space-launcher-search` | A separate search process, reached through a Unix socket |
| Signing | Developer ID, notarized, arm64 only. The extension carries a **Developer ID provisioning profile** ("Spacebar FSExt DevID-Dist", all devices) that grants `com.apple.developer.fskit.fsmodule` |

What voidfs takes from this:
- Distributing an FSKit module with Developer ID works, which removes the spike's main
  distribution risk.
- `/Volumes` does not need the `fskit.mount` entitlement: a privileged helper can do the mount.
- Finder Sync, not File Provider, gives the Finder badges. It is now checklist item D12.
- The split between a thin extension and a Rust core in a separate process is confirmed by a
  shipping product.

## 6. Performance

### What SpaceFS publishes

**Near the bucket** (20 September 2026, run `20260920T055107Z`):
- Setup: client on a Google Cloud n2-standard-8 in us-east4; bucket on AWS S3 us-east-1 with
  native conditional writes.
- Method: Rust `aws-sdk-s3`, 8 operations at a time, 3 warm-ups, 2 rounds, median of each.
- Every scenario runs once through SpaceFS and once against the bare bucket.
- SpaceFS wins 31 of 49 scenarios.

| Scenarios | Rows | SpaceFS against the bare bucket | Example: SpaceFS / bare, p50 |
|---|---|---|---|
| Small, ranged and cached reads, `head`, fan-out gets | 7 | 2.6–34× faster | 64 KiB range of a 64 MiB file: 1.3 / 44.1 ms |
| Large gets and streams (32–256 MiB) | 4 | 12–16× faster | get 64 MiB: 47.2 / 779 ms |
| Edits inside 32 and 64 MiB files (append, insert, delete, truncate, write, patch) | 16 | 1.4–15× faster | append 4 KiB to 64 MiB: 111 / 1,668 ms |
| Rename and folder move | 2 | 7.9–18× faster | move a folder of 200 files: 99 / 1,774 ms |
| Listing | 1 | 9.1× faster | 200 keys: 3.0 / 27.2 ms |
| Edits inside 1 MiB files | 8 | parity to 2.1× slower | write 4 KiB in 1 MiB: 157 / 101 ms |
| Whole-object puts and overwrites, fan-out puts | 9 | 1.1–3.1× slower | overwrite 4 KiB: 84.6 / 27.7 ms |
| Multipart uploads | 2 | 1.8–2.4× slower | 64 MiB in 8 MiB parts: 1,613 / 660 ms |

**Around the world** (23 September 2026): the same workload from six regions, as a geometric mean
against S3 us-east-1.

| Region | Own drive | Drive in Virginia |
|---|---|---|
| Sydney | 9.7× | 5.5× |
| Singapore | 5.7× | 4.8× |
| Frankfurt | 2.2× | 1.8× |
| Oregon | 1.4× | 1.4× |
| Virginia | 0.8× | 0.6× |
| São Paulo | 0.8× | 0.8× |

SpaceFS publishes no numbers for its mount.

### Where voidfs stands

*First measurements, 27 September 2026: [bench/README.md](../bench/README.md#results-so-far).*
All 49 scenarios ran on one Mac, against a local S3 server (versitygw), once over loopback and
once with the bucket 12 ms away; 23 of them also ran against Cloudflare R2. **These are not comparable with SpaceFS's cloud figures**; the
real run in their setup is still to do (§8).

| Scenarios | Rows | Loopback | Bucket 12 ms away | 12 ms, group commit (28 Sep) | 12 ms, shard cache (28 Sep) | SpaceFS |
|---|--:|---|---|---|---|---|
| Small, ranged and cached reads, `head`, fan-out gets | 7 | 1.1× slower to 4.6× faster | 1.1× slower to 48× faster | 1.1× slower to 51× faster | 12–48× faster | 2.6–34× faster |
| Large gets and streams | 4 | 1.1–1.7× slower | 1.1–3.0× faster | 1.1× slower to 3.1× faster | 2.3–3.1× faster | 12–17× faster |
| Edits inside 32 and 64 MiB files | 16 | parity to 24× faster | 1.1× slower to 3.3× faster | 1.1× slower to 8.2× faster | 1.5–8.3× faster | 1.4–15× faster |
| Rename and folder move | 2 | 44–138× faster | 1.5–4.8× faster | 5.8–18× faster | 5.2–18× faster | 7.9–18× faster |
| Listing | 1 | 31× faster | 34× faster | 37× faster | 36× faster | 9.1× faster |
| Edits inside 1 MiB files | 8 | 1.6–4.7× slower | 3.4–3.6× slower | 1.3–1.7× slower | 1.3–1.7× slower | 2.1× slower to parity |
| Whole-object puts and overwrites, fan-out puts | 9 | 1.7–4.5× slower | 2.1–70× slower | 2.1–3.1× slower | 2.2–3.0× slower | 1.1–3.1× slower |
| Multipart uploads | 2 | parity to 1.2× faster | 1.6–1.9× slower | 1.6–1.9× slower | 1.5–1.8× slower | 1.8–2.4× slower |
| **All 49**: faster in / geometric mean | | 26 / 2.1× | 27 / 1.0× | 24–26 / 2.0× | 30 / 2.6× | 31 / 2.8× |

The group-commit and shard-cache columns are the median of each row over two runs
([bench/results/group-commit](../bench/results/group-commit/README.md),
[bench/results/shard-cache](../bench/results/shard-cache/README.md)).

What the runs show:
- **The prediction held for metadata:** listing, `head` and small warm reads are far ahead of
  the bare bucket at any distance, and further ahead than SpaceFS.
- **Against Cloudflare R2** (23 small-object scenarios, voidfs-server beside the harness on the
  Mac, the bucket about 200 ms per PUT away): reads and metadata ran 15–324× faster than the
  bare bucket, and every write ran at its concurrency times one PUT, up to 13 s. Run again on
  29 September with group commit and the shard cache (a 64 MiB cache, so that it fills): writes
  took 0.6–1.0 s, the fan-out gets 0.6–2.2 ms instead of `main`'s 75–116, and 16 of the 23 rows
  were at or ahead of SpaceFS's ratio against the earlier run's bare bucket
  ([bench/results/shard-cache](../bench/results/shard-cache/README.md#against-cloudflare-r2)).
- **Everything that wrote was held back by one thing:** a drive committed one mutation per
  bucket round trip, because the commit lock was held across the log's conditional PUT. At 8
  operations at once, every write cost 8 round trips; at 64, 64. That is why renames and
  large-file edits lost most of their lead once the bucket was far away, and why fan-out puts were
  up to 70× slower. Group commit (step 3, item 1) fixed it: mutations that wait while a log entry
  is written share the next one, and a write's p50 now stays at two or three round trips from 1
  to 64 at once.
- **The shard cache stopped admitting new shards** once shards read often earlier filled it
  (moka's TinyLFU admission), so warm reads silently went to the bucket. It now keeps what was
  used last (step 3, item 2): the three fan-out gets take 0.5–1.1 ms instead of 12–15, get
  32 MiB 25 ms instead of 56–88, and 12 of the 16 edits inside 32 and 64 MiB files, which read
  the shard they rewrite, are 11–57% faster.
- Also found: patch re-chunks once per edit, multipart completion is a chain of round trips, and
  a write takes at least two round trips where SpaceFS takes one. Details and code references are
  in [bench/README.md](../bench/README.md#findings-where-voidfs-is-far-from-parity-and-why).
- The FSKit spike's mount numbers (loopback: 2.3–2.5 GB/s sequential, 1,000 files listed in
  21–33 ms, random 4 KiB reads at 1.5–1.9 ms p50) are still the only ones for the mount.

### Against SpaceFS's ratios

Step 3 is done when every row is at least as fast, relative to the bare bucket, as SpaceFS's.
Scored on the local runs, which are not SpaceFS's setup:

| Run | Rows at or ahead of SpaceFS | Edits (24) | Writes (11) | Reads (10) | Metadata (4) |
|---|--:|--:|--:|--:|--:|
| Bucket 12 ms away, 28 September, with the shard cache's admission (two runs) | 21–22 | 9–12 | 2–3 | 6 | 2–3 |
| Bucket 12 ms away, 28 September, `main` with group commit, in the same session (two runs) | 16–19 | 8–10 | 3 | 3 | 2–3 |
| Bucket 12 ms away, 28 September, with group commit (two runs) | 17 | 9–10 | 2 | 3 | 2–3 |
| Bucket 12 ms away, 28 September, before group commit (two runs) | 6–7 | 0 | 1–2 | 3 | 2 |
| Bucket 12 ms away, 28 September, with checkpoints and the capability probe | 7 | 0 | 2 | 3 | 2 |
| Bucket 12 ms away, 28 September, with garbage collection | 7 | 0 | 2 | 3 | 2 |
| Bucket 12 ms away, 27 September | 6 | 0 | 1 | 3 | 2 |
| Loopback, 27 September | 20 | 13 | 4 | 0 | 3 |

With the shard cache's admission, at 12 ms:
- 19 rows are ahead in both runs, against 15 for `main` in the same session:
  - `head`, listing, get 4 KiB and 1 MiB, the range read, and the three fan-out gets;
  - nine edits: five of the eight in 1 MiB files, delete and insert 4 KiB at the start of
    64 MiB, write 4 KiB in 64 MiB, and patch in 32 MiB;
  - overwrite 4 KiB and multipart put 64 MiB.

  Five more are ahead in one run of the two: delete 4 KiB in the middle of 1 MiB, insert 4 KiB
  at the start of 32 MiB, patch in 64 MiB, the folder move and multipart put 256 MiB.
- The geometric mean speed-up over the bare bucket went from 2.0× to 2.6× (SpaceFS's: 2.8×), and
  voidfs beats the bare bucket on 30 rows (SpaceFS does on 31).
- Group commit before it took the geometric mean from 1.0× to 2.0×: every edit, rename, folder
  move, fan-out put, and put or overwrite of up to 1 MiB took half the time or less. Large puts
  and multipart uploads, which spend their time taking in the body, did not change.
- Multipart put 256 MiB sits at SpaceFS's 0.56× of the bare bucket and crosses it from run to
  run, mostly because the bare bucket's own time varies.
- Earlier, neither content-defined checkpoints nor the probe changed speed
  ([bench/results/checkpoints](../bench/results/checkpoints/README.md),
  [bench/results/capability-probe](../bench/results/capability-probe/README.md)).

What holds back the 27–28 rows voidfs does not yet win at 12 ms
([bench/results/shard-cache](../bench/results/shard-cache/README.md)):
- **A write takes its shards, then its log entry, one after the other (8 of the 9 writes, and
  most of the 15 edits).** Fan-out put 200 × 256 KiB is within 3% of SpaceFS's ratio; put 4 KiB,
  overwrite 1 MiB and fan-out put 1,000 × 4 KiB at 64 within 13–26%; put 1 MiB and the fan-out
  put at 32 within 35–38%. Edits inside 32 and 64 MiB files, now that the shard they rewrite is
  cached, take 34–44 ms where SpaceFS's ratio needs 17–41. Puts of 32 and 64 MiB are about half
  as far ahead as SpaceFS, which is the cost of taking in the body. Step 3, item 3.
- **Patch rewrites a shard once per edit (patch in 1 MiB, and in 64 MiB in one run).** Item 4.
- **Large reads can't be judged locally (get 32 and 64 MiB, and streams of 64 and 256 MiB):**
  the emulated bucket adds latency but no bandwidth limit. The bare bucket's 64 MiB get takes
  110–130 ms here, against S3's 779 ms in SpaceFS's run. Get 32 MiB now takes the same time per
  byte as get 64 MiB. Edits inside 32 and 64 MiB files are understated the same way. Only the
  real run can judge these rows.
- **Rename at 8 at once takes two round trips** (24–26 ms, where SpaceFS's ratio needs 15–17):
  after a log entry lands, the first new request starts the next alone and the rest wait for it.
  A short hold before an entry might bring this to one round trip; not tried.

Garbage collection left four small-write rows 3.5–7% slower, for a reason not found. With group
commit they take 30–38 ms instead of 106–851, and what is left of that difference can't be told
apart from round trips ([bench/results/gc](../bench/results/gc/README.md),
[bench/results/group-commit](../bench/results/group-commit/README.md)).

## 7. Step-by-step plan

Each step lists what it delivers and when it counts as done. Later steps depend on earlier ones.

1. **Make performance measurable.**
   - Port SpaceFS's 49 scenarios exactly into `bench/`: same sizes, concurrency, warm-ups,
     rounds and p50. It must run against any endpoint and against the bare bucket.
   - Run it in CI against MinIO.
   - Run it for real in their setup: a GCP n2-standard-8 in us-east4, S3 us-east-1, the voidfs
     server in us-east-1.
   - Benchmark the two Mac apps head to head, with the spike's mount measurements (listing,
     sequential and random reads, `semantics.c`, Finder behaviour) on the same test data. One run
     goes through SpaceFS's app on a trial drive; the other through voidfs on a bucket in the same
     region.
   - **Done when:** a table in the same format as theirs is published for both the S3 layer and
     the mount.
   - **Status (2026-09-29):** the scenarios are ported and have run locally and against R2 (§6).
     CI (GitHub Actions) runs them on every push to `main` at a tenth of the operations, with
     versitygw 12 ms away as in the local runs, and keeps the results; MinIO no longer publishes
     binaries, so the benchmark doesn't use it. The run in SpaceFS's setup and the Mac comparison
     are still to do.
2. **Finish the engine** (the Phase 1 exit criteria).
   - Garbage collection first. **Done** (E8): two phases per format §12 as amended by
     [RFC 0002](../rfcs/0002-gc-safe-against-writers.md), which closes a race in draft 1 that
     could delete a shard a new commit referenced. The protocol is model-checked and the
     implementation simulated (`crates/voidfs-server/src/gc/`). It reclaims deleted drives and
     abandoned uploads; old versions in live drives wait for retention policies (E13).
   - Content-defined checkpoint segments. **Done** (E11): a segment ends where a row's key hash
     has 12 low zero bits, within 256–8,192 rows (format §8.3), so a checkpoint stores only the
     segments that changed and a fork shares its source's. A checkpoint comes every 1,000
     commits or 16 MiB of log, counted from the last one and across restarts (§8.4). Loading a
     checkpoint fetches its segments 32 at a time.
   - The bucket capability probe. **Done** (B3): at start, a server creates `voidfs.json` again
     with its own bytes, which the bucket must refuse (format §7.2). OpenDAL's capability flags
     describe its driver, not the endpoint, so only trying tells. A bucket that ignores or
     fails the check can't open a pool for writing, unless the pool was created with
     `--commit-guard external` (§7.3: one server, which checks before each write). Lifecycle
     rules that would delete or archive the pool's objects also stop a server, and versioning
     without expiry of old versions is a warning. `voidfs-server probe` reports these and more
     (presigned URLs, CORS, object lock, modification times, the bucket's clock) and writes
     nothing new. Temporary credentials are reported as not probed, until short-lived
     credentials (B4) need them. Checked against versitygw 1.8.0 and Cloudflare R2.
   - Runs on AWS S3 and MinIO, and rclone.
     - MinIO: **done**. MinIO no longer publishes binaries or images, and its repository is
       archived, so CI builds its last release (`RELEASE.2025-10-15T17-29-55Z`) from source and
       runs the conformance suite and the clients on it. versitygw stands in for it locally.
     - rclone: **done** ([`tests/interop/rclone_smoke.sh`](../tests/interop/rclone_smoke.sh)):
       copy with multipart, `check --download`, sync, server-side copy and move, `rcat`. `rclone
       purge` fails: on a versioned bucket it deletes every version, and voidfs keeps history
       (protocol §3). `rclone delete` then `rclone rmdir` removes a drive.
     - AWS S3: **done** (29 September, from this Mac over home internet to us-east-1). The
       conformance suite passes (35/35), rclone does (10/10), and the probe reads every setting
       it checks. Of the 23 small-object scenarios, 20 are at or ahead of SpaceFS's ratio to the
       bare bucket and 16 faster than it; put 4 KiB and the two fan-out puts of 4 KiB are behind
       ([results](../bench/results/aws-small-objects.md)).
   - Virtual-host addressing. **Done** (S1): with `--virtual-host-domain <domain>` (repeatable),
     `<drive>.<domain>/<key>` reaches the same drive and key as `/<drive>/<key>`, which keeps
     working. The domain itself, and any other host, stay path-style, and without the flag
     nothing changes. The signature is checked on the request as sent, and must cover `host` in
     either style, as S3 requires (`403 AccessDenied` otherwise). Locations name the path on
     the host the request used (`/<key>` rather than `/<drive>/<key>`). The conformance runner
     runs every case either way (`--virtual-host <domain>`): 35/35 both ways, and boto3, the
     Rust SDK, curl's SigV4 and all three `aws-chunked` forms work.
   - A `docker compose` file, and health checks and metrics. The Compose file is **done**
     ([`deploy/compose/`](../deploy/compose/)): the server's image, its pool in a volume, in a
     bucket of yours, or in versitygw beside it, which CI brings up and tests. Health is only
     "answers HTTP" for now; a health endpoint and metrics on a port of their own are to do.
   - **Status (2026-09-29):** 5 of 6 done, and the Compose file.
3. **Win the rows SpaceFS loses.** The work items, with the step 1 evidence and a row-by-row
   baseline, are in [step-3-performance.md](step-3-performance.md). In order of impact:
   - Group commit: one log write per batch of mutations, not per mutation (37 rows). **Done**:
     rows at or ahead of SpaceFS's ratio at 12 ms went from 7 to 17, and write p50 stays at two
     or three round trips from 1 to 64 at once.
   - A shard cache that admits new shards. **Done**: it keeps what was used last, and holds its
     own copy of each shard. Rows at or ahead of SpaceFS's ratio at 12 ms went from 16–19 to
     21–22, and the fan-out gets take about a millisecond. A disk tier (S5) comes later.
   - Fewer sequential round trips per write: a small-file path with tiny files stored inside
     their metadata (a format change, so an RFC first), pipelined ingest, and checkpoints off
     the commit path.
   - Patch that rewrites each touched shard once, and a multipart completion without a chain
     of round trips.
   - Parallel and coalesced shard fetch for cold and large reads, once the harness can measure
     cold reads.
   - **Done when:** every one of the 49 rows is at least as fast as SpaceFS's.
   - **Status (2026-09-28):** group commit and the shard cache's admission are done; 21–22 of the
     49 rows are there in the local 12 ms runs.
4. **Client core, CLI and Rust SDK.**
   - `crates/client`: cache, journal, upload queue, change-feed client.
   - Direct uploads (§4.11), and short-lived storage credentials: R2, AWS STS, and presigned URLs
     as the fallback.
   - A `voidfs` CLI matching every `spacefs` command that needs no account: drives, drive, mount,
     unmount, mounts, uploads, upload, status, daemon, history, show, restore, version.
   - The Rust SDK.
5. **Writable macOS drive** (Phase 2).
   - The design the spike chose: the per-user agent and a thin extension.
   - Mac file semantics (xattrs, no `._` files, atomic saves) and snapshot-at-open reads.
   - A connectivity state that fails fast when offline, and read-ahead for video.
   - A privileged helper that mounts into `/Volumes`, as SpaceFS does.
   - Finder Sync badges.
   - A menu-bar transfer queue (pause, resume, speed limits).
   - A notarized Developer ID build.
   - **Done when:** the Phase 2 criteria are met. A 50 GB video project and a code repo can be
     edited from two Macs, changes show within 5 s, and the app-compatibility matrix is green.
6. **Accounts and web app** (Phase 3).
   - Passwordless email sign-in, workspaces and roles.
   - Keys minted and revoked within a minute, with drive allowlists.
   - Bucket connections, with their credentials stored encrypted.
   - A web file browser, share links, an audit log and quotas.
   - The account commands of the CLI: login, logout, whoami, workspace, use, keys.
7. **SDKs and the agent surface.**
   - TypeScript, Python and Go SDKs.
   - `operations.json`, `llms.txt` and `llms-full.txt`.
   - An MCP server (SpaceFS has none).
8. **Search, previews and video review**, the last parts of SpaceFS's Mac app.
   - A filename index across drives and the Mac.
   - Content search.
   - Previews for the Mac app, the web and share links.
   - Video review with timecoded comments.
   - **Done when:** everything SpaceFS's Mac app offers, voidfs's does too.
9. **Linux CLI and FUSE mount**, with self-update and deb, rpm and Homebrew packages.
10. **Enterprise and scale.**
    - SSO, SAML and SCIM; admin tools; encryption; retention policies.
    - Several nodes with fenced leases.
    - Regional presence: several regions, or the Cloudflare deploy target.
    - **Done when:** the global benchmark matches in all six regions.

**After parity**, where voidfs can lead: Windows, adopting an existing bucket in place, export to
plain objects, file locking, offline pinning.

## 8. Open items

- **Cloud accounts for step 1:** an AWS bucket in us-east-1 and a GCP VM in us-east4, billed to
  the maintainer. The commands and scripts are in
  [bench/README.md](../bench/README.md#the-real-run). SpaceFS ran their layer on the client VM,
  so that topology is the like-for-like one; the plan's server in us-east-1 is a second run.
- **Test data in the SpaceFS trial:** uploading the benchmark data into a trial drive needs the
  account owner's go-ahead each time.
- **CI** ([`.github/workflows/ci.yml`](../.github/workflows/ci.yml), added 2026-09-29): tests,
  clippy and the spec's cases; the conformance suite, boto3 and rclone over memory, local disk,
  versitygw and MinIO; the Compose files; and on `main`, the release GC model and a small
  benchmark. Still to add: the bucket checks the capability probe can't make without new
  objects (that create-if-absent holds when writers race, and that reads and listings see
  writes at once).
- **Tools:** rclone and Docker (with Colima) are installed on the development Mac; the AWS CLI is
  not. MinIO's Homebrew build crashes on this Mac, and MinIO no longer publishes binaries or
  images, so versitygw is the local S3 server and CI builds MinIO from source.
- **An AWS S3 bucket** in us-east-1, with a key scoped to it, exists since 2026-09-29 (made as
  in [bench/README.md](../bench/README.md#the-real-run); the key is in `.env.aws`, which git
  ignores). Step 2's runs used it; step 1's real run can too. Runs against it are asked for
  first, and `voidfs-bench/` is emptied after each.
- **Order of the next steps (agreed 2026-09-28):** the shard cache's admission (step 3, item 2),
  then the rest of step 2, then fewer sequential round trips per write (step 3, item 3).
  - Content-defined checkpoint segments came first, because they change the checkpoint writer
    that step 3, item 3 moves off the commit path. The capability probe and group commit came
    next. All three are done.
  - Item 2 is done. Item 3's small-file path is a format change and needs an RFC first; with
    group commit done, it is what most of the remaining write and edit rows wait on.
