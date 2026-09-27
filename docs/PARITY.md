# voidfs and SpaceFS: parity status and plan

*Stocktake of 2026-09-27. Sources: the voidfs code at commit `76c8b9a`, the parity checklist in
[§3 of the plan](RESEARCH_AND_PLAN.md#3-parity-checklist-everything-to-build), SpaceFS's
benchmark pages (runs of 20 and 23 September 2026) and changelog, and the SpaceFS macOS app
0.2.300 installed on the same Mac.*

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
  - 35 conformance cases passing on local disk and Cloudflare R2.
- **Almost everything a person touches is missing:** a writable mount, a CLI, accounts and a web
  app, SDKs, search and share links.
- **Performance parity is unmeasured.** SpaceFS publishes 49 benchmark scenarios; voidfs has never
  run them. Step 1 of the plan fixes that.
- **SpaceFS's Mac app is now understood** (§5). It is a native FSKit module with its core in Rust,
  running in a separate daemon, which is the architecture the FSKit spike chose for voidfs. It
  also shows that the FSKit entitlement can ship with Developer ID.

## 2. Decisions that shape the plan

| Decision | Choice | Notes |
|---|---|---|
| Platform order | **Full Mac parity first** (steps 1–8), then Linux (step 9) | Full Mac parity includes search, previews and video review. Linux servers use S3 and the SDKs until the Linux mount lands |
| Minimum macOS | **macOS 27** (decided 2026-09-27) | SpaceFS states macOS 26.4 as its minimum; its app bundles declare 26.0. voidfs gives up macOS 26 because only macOS 27 lets a module evict the kernel's caches when another machine changes a file ([spike §5](spikes/fskit.md#5-kernel-caching-and-coherence)) |
| Mac architecture | Per-user agent running the Rust core, thin FSKit extension | [Spike §4.1](spikes/fskit.md#41-where-should-the-rust-client-core-run). SpaceFS made the same split (§5) |
| Hosting | Self-hosted only | No billing, plans or trials to match. The hosted edge network is matched by deploying near users (step 10) |

## 3. Scorecard

Every item of the plan's checklist (§3, 71 items with the Finder integration item added today),
scored against the code:

| Area | Done | Partly | Missing | State |
|---|---|---|---|---|
| Engine (E1–E13) | 6 | 2 | 5 | Chunking, versions, point-in-time reads, restore, in-place edits, forks and checkpoints work. Missing: garbage collection, the small-file path, direct uploads, encryption, retention policies |
| Storage backends (B1–B8) | 0 | 1 | 7 | Local disk and R2 work. Not yet run on AWS S3 or MinIO. No capability probe, no short-lived storage credentials, no adopt or export |
| Server (S1–S9) | 2 | 4 | 3 | Full S3 subset, extensions and change feed, on one node. Missing: virtual-host addressing, a disk cache tier, several nodes, several regions, quotas |
| Accounts and web (C1–C10) | 0 | 1 | 9 | Static keys from command-line flags only |
| Clients (D1–D12) | 0 | 3 | 9 | A read-only macOS mount (the spike). No agent, journal, CLI, Finder integration, Linux or Windows |
| SDKs, agents, search (A1–A7) | 0 | 0 | 7 | Stock S3 SDKs and the AWS CLI work; nothing voidfs-specific |
| Operations (O1–O6) | 0 | 1 | 5 | One binary. No compose file, metrics or benchmark harness |
| **Total** | **8** | **12** | **51** | Of the 28 P0 items: 8 done, 10 partly, 10 missing |

"Partly" means:
- E1 has no compression, and E11 lacks content-defined segments.
- B1 is not yet run on AWS S3 or MinIO.
- S1 lacks virtual-host addressing; S3 lacks direct upload and credentials; S5 is memory-only;
  S8 has no operations catalogue.
- C3 keys can't be minted or revoked.
- D3 is read-only, D6 relies on the kernel's read-ahead only, and D11 is the spike's shell.
- O1 has no compose file.

## 4. Product by product

| SpaceFS | voidfs today |
|---|---|
| S3-compatible endpoint with extensions (protocol v1) | Protocol 1 with 35 conformance cases. Missing: virtual-host addressing, direct uploads, storage credentials |
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

**Near the bucket** (20 September 2026, build `s3sdk@fff9779`):
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

- Not measured on this workload.
- The only numbers so far are the FSKit spike's, over loopback: 2.3–2.5 GB/s sequential,
  1,000 files listed in 21–33 ms, random 4 KiB reads at 1.5–1.9 ms p50.
- Two predictions follow from the architecture, to be checked by step 1:
  - The metadata-only rows (rename, move, listing, edits in large files) should come close
    without tuning.
  - The small-write rows are where voidfs can beat SpaceFS (step 3).

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
2. **Finish the engine** (the Phase 1 exit criteria).
   - Garbage collection first. Without it, deleted and overwritten data is never reclaimed.
   - Content-defined checkpoint segments.
   - The bucket capability probe.
   - Runs on AWS S3 and MinIO, and rclone.
   - Virtual-host addressing.
   - A `docker compose` file, and health checks and metrics.
3. **Win the rows SpaceFS loses.**
   - A small-file path: tiny files stored inside their metadata.
   - Fewer bucket writes per commit, and batched commits.
   - A disk tier for the shard cache, and parallel shard fetch for large reads.
   - **Done when:** every one of the 49 rows is at least as fast as SpaceFS's.
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
  the maintainer.
- **Test data in the SpaceFS trial:** uploading the benchmark data into a trial drive needs the
  account owner's go-ahead each time.
