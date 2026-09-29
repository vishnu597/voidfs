# voidfs: research and build plan

*voidfs is an open-source alternative to Space (spacefs.com): the service layer, self-hosted, on
the user's own bucket.*

**Decided (2026-09-26):**
- Project name **voidfs**.
- License **Apache-2.0** for everything (code, SDKs, specs), with **DCO** sign-off.
- **No hosted offering**: voidfs is self-hosted only.
- **macOS first, on native FSKit**, then Windows.

See §11 for the full list. Protocol approach: §12. Phase 0 specs: [`spec/`](../spec/). Where voidfs
stands against SpaceFS today, and the step-by-step plan to parity: [PARITY.md](PARITY.md).

*Researched 2026-09-26 from spacefs.com, docs.spacefs.com (including their full `llms-full.txt`,
protocol spec, benchmarks), the changelog, terms, press coverage, and the SDK registry listings.*

---

## 0. TL;DR

- **What Space is:** a cloud filesystem, "one drive on every machine at once." People mount a
  drive as a folder (macOS and Linux, Windows coming), programs reach the same drive as an
  **S3-compatible bucket**, and agents **fork** it. Every write is a version. A fork of a whole
  drive is copy-on-write and costs the same at any size. Files stream by byte range, so
  terabytes take no local disk. Paid plans run $15–30 per user per month with 1 TB included.
- **How it works:** files are cut into content-defined shards (FastCDC, about 2 MiB, SHA-256
  named) and stored in an object bucket. A version is a manifest (a list of shards). One
  **authority per drive** orders commits, which gives read-after-write and precondition checks.
  Reads are cached at the edge and on the client. Every capability they advertise follows from
  those three choices. The placement hints and the "Cloudflare location nearest the caller" line
  strongly suggest they run on Cloudflare Workers and Durable Objects.
- **They already support "your own storage"** (S3-compatible, private cloud, on-prem), but only
  as a closed service, sold mainly as an enterprise feature.
- **The opening for an open-source version:** the same model, free to self-host, bring-your-own
  bucket as the default path, an **open on-bucket format** (no lock-in), plus things Space does
  not do today: zero-copy import of existing buckets, export to plain objects, file locking,
  offline pinning, an MCP server for agents, and Windows from day one.
- **Protocol:** voidfs publishes its own versioned protocol and on-bucket format specs under the
  `x-voidfs-*` namespace (§12). Space's SDKs are MIT-licensed and their protocol is public, so a
  compatibility shim that also accepts their headers stays possible later, but it is optional
  and off by default.
- **Suggested stack:** Rust core (engine, S3 gateway, daemon, CLI) built on Apache OpenDAL
  (storage backends), `s3s` (S3 server and SigV4) and `fastcdc`.
  - macOS first: a native FSKit module in a SwiftUI menu-bar app, with the Rust client core in a
    per-user launchd agent (decided; see the [FSKit spike](spikes/fskit.md)).
  - Windows later: WinFsp or the Cloud Files API.
  - Postgres or SQLite for the control plane.
- **The hardest parts** are not the S3 API. They are (1) POSIX and desktop-app semantics over a
  versioned store (atomic saves, locks, mmap, xattrs) with Premiere, Revit and Office, (2) the
  macOS mount story, (3) garbage collection that stays correct when forks share shards, and
  (4) keeping request and egress costs sane on users' own buckets.

---

## 1. What Space sells: the product surface

### 1.1 Positioning

- Tagline: "The infinite AI-native filesystem." Also: "Instant access to terabytes of data on
  any computer, using zero disk space."
- Company: Space Computer, Inc., founded 2025 by Matthew Ao, Arihant Bapna and Jason Zhao.
  Prototype built November 2025. $2.4M pre-seed led by a16z Speedrun in August 2026, with
  Golden Ventures and Northside. About 100 teams in private beta at announcement.
- Target industries:
  - Film, video and audio: scrub camera originals without proxies or downloads; onboard
    freelancers with a login instead of shipping drives; work on footage before its upload
    finishes. Named apps: Resolve, Premiere, Final Cut, After Effects, Pro Tools.
  - Architecture, engineering and construction (AEC): open large models without syncing the
    whole project; version history on drawings; office and field on the same files. Named apps:
    Revit, AutoCAD, Rhino, SketchUp.
  - Games and 3D: stream assets instead of syncing projects; share selected folders with
    partners; fork projects for experiments. Named apps: Unreal, Unity, Houdini, Blender, Maya.
  - Knowledge work and agents: agents share live files across machines and sandboxes; one
    forked drive per task. Named: Office formats, PDF, S3 clients, SDKs, Claude.

### 1.2 Surfaces (everything a user can touch)

| Surface | What it does | Status |
|---|---|---|
| **macOS app** | Mounts drives in Finder, manages transfers (pause and resume, speed limits), onboarding, billing, previews, "video reviews", Space Search | Shipping (v0.1.9xx, near-daily releases) |
| **Linux CLI and daemon** (`space`) | FUSE 3 mount; static x86_64 and aarch64 binaries; deb, rpm and tarball; `curl \| sh` installer; systemd user unit; self-update with SHA-256 check and rollback | Shipping, "refocused on command line" |
| **Windows** | none | "Coming soon" |
| **Web app** (app.spacefs.com) | Sign-in, settings, access-key management, billing, download | Shipping (no evidence of a full web file browser) |
| **S3-compatible endpoint** (`s3sdk.spacefs.com`) | Drives are buckets. SigV4, path or virtual-host addressing, a standard subset plus `x-s3sdk-*` extensions | Shipping, protocol v1 |
| **SDKs** | TypeScript, Python, Go, Rust. Wrap the official AWS SDK and add extension calls | v0.3.0, **MIT licensed** |
| **Agent docs** | `llms.txt`, `llms-full.txt`, `operations.json` (every op with samples in four languages), "Build with agents" guide | Shipping |
| **Space Search** | "10× faster than Spotlight, across your Mac and every Space drive"; "Ask Space to work with files that don't exist on disk" (AI search and agent actions) | Marketed on homepage |
| **Public and restricted links** | Share links usable without sign-in (Terms; Individual plan) | Shipping |

### 1.3 Feature inventory

**Filesystem and sync**
- Mount as a native drive. The full namespace appears instantly and data is fetched lazily by
  byte range.
- A local shard cache ("reads get faster as you work"). Because shards are content-addressed,
  the cache is never stale.
- A local write journal: saves return at local speed and publish in the background. There is an
  upload queue with status, pause and resume, speed controls, and low-disk handling.
- Changes from other machines or from S3 writers appear "within seconds," with nothing to
  refresh.
- Bulk import through the daemon (`space upload … --detach`, `space uploads --watch`).
- Read-only mounts, `--allow-other`, foreground mode, mount by name or id, and remounting after
  a daemon restart.
- Works with pro apps "without plugins."
- Needs the internet. No offline mode is documented.

**Data model**
- Drives (bucket = drive, with an alias or a `d-<uuid>` id), grouped into workspaces.
- Real folders: `a/b/c` creates `a/` and `a/b/`. A file and a folder cannot share a path
  (`409 PathConflict`).
- Versioning is always on. Every mutation is exactly one version and history is append-only.
- Time travel: read by `versionId` or `as_of` a timestamp. Roll back by creating a new head
  equal to an old version, with no bytes copied. Subtree restore works from the CLI.
- In-place edits: `write_at` (pwrite), `patch` (up to 10,000 edits as one version), `splice`
  (insert or remove bytes with the rest shifting), and `truncate` or extend.
- Rename or move a file or folder of any size as one metadata-only version.
- Same-drive copy by reference.
- Copy-on-write drive **forks** in constant time, up to 8 deep, with lineage shown by
  `describe_drive`. Hard delete is refused while forks share content.
- Soft delete of drives with a retention window, then hard delete.
- Dedup across versions, files and forks.
- Preconditions: `if_version`, `If-Match`, `If-None-Match: *`. A mismatch returns `412` naming
  the winning version.
- Consistency: read-after-write through the endpoint. Cross-region reads may lag up to about 2 s
  unless the request sends `x-s3sdk-consistent: true`.
- Drive **placement** hints (`wnam enam sam weur eeur apac oc afr me`), fixed at creation.
- Direct uploads: the client chunks the file, asks which shards are missing, uploads only those
  to the bucket with presigned URLs, then commits.
- Limits: 64 MiB extension body, 5 GiB single PUT, 5 TiB multipart, 1024-byte keys.

**Identity, access and security**
- Passwordless email login with a six-digit code, in both the CLI and the web.
- Workspaces with members.
- Access keys: `s3sdk` plus 15 characters, a 40-character secret, and scope `read`, `write` or
  `admin`, optionally limited to an allowlist of drives. Minted in the web app, the CLI or over
  HTTPS. Keys can be listed with last-used time and revoked, which takes effect within about a
  minute.
- **Mount credentials:** an access key can be exchanged for short-lived storage credentials
  (about 1 h, sometimes 15 min) scoped to one drive's prefix in the bucket. They carry
  `canWrite`, an `accessGeneration` epoch, and a `storageBudget` quota for the client to enforce.
  The backend is visible in the response (for example `"backend": "r2"`), and a separate
  content bucket is optional.
- Enterprise: SSO and SAML, per-person, per-machine and per-agent access, central revocation,
  "every action is attributable" (audit), custom auditing and compliance reports, custom
  retention and version controls, private cloud, on-prem, dedicated infrastructure,
  bring-your-own S3-compatible storage, and white-glove migration.
- Encryption is claimed but not specified.

**Pricing** (all plans have a 7-day trial and no free tier)

| Plan | Price | Storage | Extras |
|---|---|---|---|
| Individual | $15/mo ($180/yr) | 1 TB | Multiple computers, public links, +500 GB for $6/mo |
| Teams | $30/member/mo ($360/yr) | 1 TB pooled per member | Shared workspaces, access controls, unlimited owned drives, +500 GB for $12/mo |
| Enterprise | Custom | Custom | See above |

---

## 2. How Space works under the hood

This section is taken from their Architecture, Protocol and Benchmarks pages. Items marked
*(inferred)* are my reading, not their statement.

```
             desktop (FUSE / macOS app)        programs & agents (S3 / SDKs)
                │  byte-range reads, local        │  SigV4, x-s3sdk-* extensions
                │  shard cache, write journal     │
                ▼                                 ▼
        ┌─────────────────────  edge (nearest PoP)  ─────────────────────┐
        │  S3 gateway: auth, key scopes, shard cache, range mapping       │
        └──────────────────────────────┬─────────────────────────────────┘
                                       │ commit / head lookup
                          ┌────────────▼────────────┐
                          │  per-drive AUTHORITY     │  orders commits, checks
                          │  (one region, placement  │  preconditions, holds head
                          │   hint) (inferred: CF    │  state, publishes changes
                          │   Durable Object)        │
                          └────────────┬────────────┘
                                       │ conditional writes
                          ┌────────────▼────────────┐
                          │  object bucket (R2 / S3) │  shards/<sha256>
                          │  shards + manifests +    │  txn, version locator,
                          │  commit records          │  record head per version
                          └──────────────────────────┘
```

1. **Shards.** Bytes are chunked with FastCDC v2020 (min 256 KiB, average 2 MiB, max 16 MiB)
   and each chunk is named by its SHA-256. Content-defined boundaries mean an insert changes only
   the shard it lands in, and the next boundary re-converges.
2. **A version is a manifest:** an ordered shard list, size, metadata and parent version.
   - A put writes the new shards and a manifest.
   - `write_at` or `patch` writes the touched shards and a manifest.
   - A splice writes one shard and a manifest with shifted offsets.
   - Truncate, rename and rollback write only a manifest.
3. **A commit** is atomic against the drive's authority, and preconditions are checked there.
   Each version writes a fixed set: the shard(s), the transaction, the version locator and the
   record head. Their benchmark bucket is "AWS S3 with native conditional writes," so the commit
   protocol almost certainly uses S3 `If-None-Match` / `If-Match` as a fence.
4. **The cost model is "new shards plus one fixed-price commit."** In their own benchmark, on
   the same bucket with and without the layer:
   - **Faster** on large-file edits (11–15×), ranged or warm reads (up to 34×), rename and move
     (8–18×), and listing (9×).
   - **Slower** on small puts and overwrites (2–3×), multipart (1.8–2.4×), and small-file edits.
   - Cold small reads are roughly even. Cold large reads are about 2× faster because shards are
     fetched in parallel.
5. **Forks** start from the parent's current manifests. A shard is reclaimed only when no
   version of any drive references it. A fork across different storage backends returns
   `ForkUnsupported`, or can run as an asynchronous full copy.
6. **Folders are first-class** (a namespace tree, not prefixes), which is what makes an O(1)
   folder rename possible.
7. **Multi-region:** each region serves reads from its own replica of drive state, and cross-region
   visibility is at most about 2 s. The authority lives in one region. Placement hint codes match
   Cloudflare Durable Object location hints exactly *(inferred: Workers plus Durable Objects,
   with R2 as the default backend)*.
8. **Mount path:** the client can bypass the API and read shards directly from the bucket with
   scoped short-lived credentials (the `?x-s3sdk-mount` exchange). That keeps bulk bytes off the
   service.
9. **macOS implementation:** not in their docs, but visible in their app (0.2.300, checked
   2026-09-27). It is a native FSKit module with its core in a separate Rust daemon, a privileged
   mount helper and a Finder Sync extension, signed with Developer ID ([PARITY.md
   §5](PARITY.md#5-what-spacefss-mac-app-is-made-of)). Linux uses FUSE 3.

**Weak spots they admit to:** small-object writes, multipart, first read of a small object far
from the bucket, no South America or Africa authorities, and no offline mode.

---

## 3. Parity checklist: everything to build

Priority: **P0** = required for a credible v1, **P1** = needed to match paid Space, **P2** =
enterprise or later.

### 3.1 Storage engine (the core library)

| # | Item | Pri |
|---|---|---|
| E1 | FastCDC chunker (256K/2M/16M), SHA-256 shard naming, optional zstd per shard | P0 |
| E2 | On-bucket format spec: shard, manifest, commit log and head layout, versioned and documented | P0 |
| E3 | Namespace tree with real folders, path conflict rules, O(1) subtree rename | P0 |
| E4 | Version-per-mutation, append-only history, `as_of` resolution, restore-as-new-head | P0 |
| E5 | Mutations: put, write_at, patch (binary format), splice (insert or remove), truncate or extend, copy-by-reference | P0 |
| E6 | Commit protocol: single writer per drive, preconditions, fenced by conditional writes where the backend supports them | P0 |
| E7 | Copy-on-write forks, lineage, depth limit, cross-backend fallback to asynchronous copy | P0 |
| E8 | Garbage collection: mark-and-sweep across all drives and forks sharing a shard store, soft-delete retention, safe against in-flight uploads | P0 |
| E9 | Small-object path (inline tiny files in the manifest, or pack small shards) to avoid Space's 2–3× small-write penalty | P1 |
| E10 | Direct-upload protocol: plan missing shards, upload via presigned URLs, commit with verification | P1 |
| E11 | Metadata checkpoints and compaction, so opening a drive does not replay the whole history | P0 |
| E12 | Encryption: server-side per-drive keys; optional client-side or end-to-end mode (which trades away cross-user dedup) | P1 |
| E13 | Version retention policies (keep N, keep for T, legal hold) | P2 |

### 3.2 Storage backends (bring your own bucket)

| # | Item | Pri |
|---|---|---|
| B1 | Backend abstraction (OpenDAL): AWS S3, Cloudflare R2, MinIO, local disk (dev) | P0 |
| B2 | GCS, Azure Blob, Backblaze B2, Wasabi, Tigris, DigitalOcean Spaces, Hetzner, Ceph RGW, Garage | P1 |
| B3 | Capability probe at bucket-connect time: conditional PUT, presign, STS or temporary credentials, lifecycle, CORS | P0 |
| B4 | Scoped short-lived credential vending per backend: AWS STS AssumeRole with a session policy on the prefix; R2 temporary credentials; GCS downscoped tokens; Azure user-delegation SAS; MinIO STS. Fallback: per-shard presigned URLs | P1 |
| B5 | Secure storage of users' bucket credentials (envelope encryption, KMS option); prefer role assumption over static keys | P0 |
| B6 | **Adopt an existing bucket in place**: index current objects as external single-extent manifests (zero copy), and re-chunk lazily on first edit | P1 (differentiator) |
| B7 | **Export to plain objects** (drive or version to a normal bucket layout): no lock-in | P1 (differentiator) |
| B8 | Split metadata and content buckets (Space supports a separate content bucket) | P2 |

### 3.3 Service: authority and S3 gateway

| # | Item | Pri |
|---|---|---|
| S1 | S3 server: SigV4 (header, presigned, `UNSIGNED-PAYLOAD`, the three `aws-chunked` variants, trailing checksums crc32, crc32c, crc64nvme, sha1, sha256), path and virtual-host addressing, S3 XML errors, `501` for anything unsupported | P0 |
| S2 | S3 subset: ListBuckets, Head/Create/DeleteBucket, GetBucketVersioning, ListObjectsV2, ListObjectVersions, Head/Get (single Range, versionId, conditionals), Put (conditionals), Copy, Delete/DeleteObjects, the full multipart set | P0 |
| S3 | Extension API (write, patch, splice, rename, versions, restore, as-of, consistent read, fork, describe, mount credentials, direct plan and commit) | P0 |
| S4 | Per-drive authority: in-memory head state, ordered commits, change feed (WebSocket or SSE) so mounts refresh within seconds | P0 |
| S5 | Shard cache in the gateway (memory plus disk, content-addressed, never stale) | P0 |
| S6 | Horizontal scale: drive-to-node assignment with leases and fencing tokens; stateless gateways | P1 |
| S7 | Multi-region: regional read replicas of drive state, a consistent-read flag, placement | P2 |
| S8 | Protocol version header, conformance suite (language-neutral JSON cases), `operations.json` catalogue | P0 |
| S9 | Quotas and storage budgets per workspace | P1 |

### 3.4 Control plane and web app

| # | Item | Pri |
|---|---|---|
| C1 | Users and auth: email one-time code (passwordless), sessions, CLI device login | P0 |
| C2 | Workspaces, members, roles (owner, admin, member, guest), per-drive ACLs | P0 (single-user) / P1 (teams) |
| C3 | Access keys: read, write or admin scope, drive allowlist, label, last-used, revoke within about a minute; mint over HTTPS | P0 |
| C4 | Bucket connections UI (add S3, R2 and others, test the connection, show capability report) | P0 |
| C5 | Web file browser: upload, download, rename, move, history, restore, fork | P1 |
| C6 | Public and restricted share links (permission, expiry, password, download or preview only) | P1 |
| C7 | Previews: images, PDF, Office, video (thumbnails, HLS proxies) for the web and for share links; "video review" with comments at timecodes | P1–P2 |
| C8 | Audit log (every action attributed to a user, key or agent), exportable | P1 |
| C9 | SSO: OIDC first, SAML, and SCIM provisioning | P2 |
| C10 | Admin: usage, quotas, retention policy, key and device inventory, central revocation | P2 |

### 3.5 Clients: daemon, mounts, CLI, desktop

| # | Item | Pri |
|---|---|---|
| D1 | Client daemon: mount lifecycle, remount at start, local shard cache (LRU, size cap, low-disk behavior), write journal with background publish, upload queue (pause and resume, bandwidth limits), change-feed subscription and invalidation | P0 |
| D2 | Linux FUSE 3 mount (`fuser`): read-only, allow-other, foreground; systemd user unit | P2 (after macOS; Linux servers use S3 and the SDKs until then) |
| D3 | macOS mount: native FSKit module (see §5.3), launchd agent, notarized and signed builds | P0 |
| D4 | Windows mount (WinFsp or the Cloud Files API), service, signed MSI | P1 (after macOS) |
| D5 | Desktop semantics: atomic-save patterns (temp file plus rename, exchangedata/renamex_np), xattrs and resource forks, AppleDouble `._` files, `.DS_Store` policy, mtime, mode bits, symlinks, sparse files, mmap, `fsync` meaning durably published or journaled | P0–P1 |
| D6 | Read-ahead and prefetch tuned for video scrubbing and large CAD files; parallel shard fetch | P1 |
| D7 | File locking: advisory `flock`/`fcntl` propagated through the authority as leases; `.dwl`/`.lck` awareness for CAD | P2 (differentiator) |
| D8 | Offline pinning ("keep this folder local"), with a conflict policy that uses versions, never loss | P2 (differentiator) |
| D9 | CLI parity: login, whoami, workspace list/use, drives, drive create/delete, mount/unmount/mounts, status, daemon install/start/stop/restart/info, upload/uploads --watch, history/show/restore, keys create/list/revoke (`--format env`), fork, update --check (JSON output everywhere) | P0 |
| D10 | Self-update: checksum-verified, atomic swap, rollback; package repos (deb, rpm, Homebrew, winget) | P1 |
| D11 | Desktop app (tray or menu bar): onboarding, drives, transfers, settings, updates, notifications | P1 |
| D12 | Finder integration: a Finder Sync extension for status badges and context menus (SpaceFS ships one) | P1 |

### 3.6 SDKs, agents and search

| # | Item | Pri |
|---|---|---|
| A1 | SDKs in TypeScript, Python, Go and Rust, each wrapping the official AWS SDK and adding the extensions; one error type; retry rules (never retry an unguarded insert or remove) | P0 (Rust, TS) / P1 (Py, Go) |
| A2 | Docs generated from source, `llms.txt` and `llms-full.txt`, and an operations catalogue | P1 |
| A3 | **MCP server**: fork drive, scoped key for the fork, read, write, patch, search, diff between versions, restore. Space documents agent patterns but ships no MCP server | P1 (differentiator) |
| A4 | Search: instant filename index across drives (and local disk on the desktop) | P1 |
| A5 | Content search: text extraction (Office, PDF, code) plus full-text index (Tantivy) | P1 |
| A6 | Semantic search and "ask about files": embeddings (pluggable, including local models) in a vector index stored next to the drive (for example LanceDB in the bucket) | P2 |
| A7 | Version diff API (which shards or byte ranges changed; text diff for text files) | P2 |

### 3.7 Distribution and operations

| # | Item | Pri |
|---|---|---|
| O1 | Single binary with `docker compose` (service, Postgres or SQLite, optional MinIO) | P0 |
| O2 | Helm chart; Terraform examples for AWS and GCP | P1 |
| O3 | Optional self-host target on the user's own Cloudflare account: Workers plus Durable Objects plus R2 (mirrors Space's own architecture; zero egress; "deploy to Cloudflare" button) | P2 |
| O4 | Metrics (Prometheus), tracing (OpenTelemetry), structured logs, health checks | P1 |
| O5 | Benchmark harness in the same shape as theirs (same bucket with and without the layer) | P1 |
| O6 | Security: threat model, fuzzing of the patch and SigV4 parsers, external audit before 1.0 | P1 |

---

## 4. Target architecture for voidfs

```
 ┌──────────── clients ────────────┐     ┌──────────── voidfs service (self-hosted) ─────────────────┐
 │ osfs daemon (Rust)              │     │  gateway (Rust, s3s + axum), stateless, N replicas         │
 │  ├ FUSE (Linux)                 │◀───▶│   SigV4 · key scopes · S3 subset · extensions · shard cache│
 │  ├ FSKit / File Provider (mac)  │ API │                     │                                      │
 │  ├ WinFsp / CfAPI (Windows)     │     │  authority (per-drive actor; leased to one node)           │
 │  ├ shard cache + write journal  │     │   ordering · preconditions · head state · change feed      │
 │  └ change feed subscriber       │     │                     │                                      │
 │ voidfs CLI · desktop · SDK · MCP│     │  control plane (axum + Postgres | SQLite)                  │
 └───────────────┬─────────────────┘     │   users · workspaces · keys · buckets · links · audit      │
                 │ direct shard GET/PUT  │  workers: GC · indexer/search · previews · import/export   │
                 │ (scoped temp creds    └─────────────────────┬─────────────────────────────────────┘
                 │  or presigned URLs)                         │
                 ▼                                             ▼
        ┌──────────────── USER'S BUCKET(S): S3 · R2 · GCS · Azure · B2 · MinIO … ────────────────┐
        │ /<drive-base>/shards/ab/cd/<sha256>   /manifests/…   /log/<seq>   /checkpoints/…       │
        │ (the bucket is the source of truth for drive data AND drive metadata)                   │
        └──────────────────────────────────────────────────────────────────────────────────────────┘
```

### Key design decisions (with recommendations)

1. **The bucket is the source of truth for drive state.** Recommended. Store the commit log,
   checkpoints and manifests in the user's bucket, as Space does (their commit writes the txn,
   locator and head into the bucket). The service then only holds accounts, keys and caches.
   Consequences:
   - Losing the service loses nothing.
   - A drive can be re-attached to a new deployment.
   - "Your data, your bucket" is literally true.

   Keep Postgres or SQLite for the control plane only. For per-drive metadata, evaluate
   **SlateDB** (an embedded LSM on object storage, single writer with fencing) against a
   purpose-built log plus checkpoints.
2. **One writer per drive.** A per-drive authority actor serializes commits. Across nodes, a
   lease names which node owns which drive, and conditional writes on the log act as a fencing
   token, so two owners can never both commit. On backends without conditional PUT, require
   single-node deployment or an external lock (Postgres advisory lock or etcd).
3. **Keep bytes off the service.** Mounts and SDKs fetch and upload shards straight to the bucket
   with scoped temporary credentials or presigned URLs. The gateway proxies only for plain S3
   clients. This matters more for self-hosters than for Space: their egress bill and throughput
   ceiling are their own.
4. **An open, versioned on-bucket format spec from day one**, plus `export` and `adopt`. This is
   the project's main differentiator and its promise of no lock-in.
5. **Protocol:** stock S3 for everything S3 can express, and `x-voidfs-*` extensions for the
   rest. The spec is published in-repo with a conformance suite (see §12).
6. **Rust for everything performance-critical.** Engine, gateway, daemon and CLI share one
   crate graph. That means one implementation of chunking, manifests and SigV4 for client and
   server.

---

## 5. Build-vs-reuse map

### 5.1 Open-source pieces worth reusing

| Need | Candidate | License | Notes |
|---|---|---|---|
| Multi-cloud storage access | **Apache OpenDAL** | Apache-2.0 | S3, R2, GCS, Azure, B2, MinIO and more; one Rust API |
| S3 server protocol and SigV4 | **s3s** (Rust) | Apache-2.0 | Implements the S3 service trait and auth; add extensions around it |
| Content-defined chunking | `fastcdc` crate (v2020) | MIT | Matches Space's parameters |
| LSM metadata on object storage | **SlateDB** | Apache-2.0 | Single writer with fencing via manifest conditional writes |
| Linux FUSE | `fuser` | MIT | Pure Rust FUSE |
| macOS kext-free FUSE | macFUSE 5 FSKit backend; FUSE-T | Mixed (see below) | macFUSE 5 FSKit supports non-local volumes on macOS 26 |
| Windows | WinFsp; Cloud Files API | GPLv3 with FLOSS exception / OS API | WinFsp is FUSE-compatible, which eases porting |
| Full-text search | Tantivy | MIT | |
| Vector search | LanceDB / Lance | Apache-2.0 | Can live in object storage |
| Desktop shell | Tauri | MIT/Apache | |
| Auth: SSO, SAML, SCIM | Zitadel / Authentik / Ory / BoxyHQ SAML Jackson | Various | Or integrate as an external IdP instead of building it |
| Local test backends | MinIO, Garage, LocalStack | AGPL / AGPL / Apache | For CI conformance across backends |

### 5.2 Nearest existing projects (learn from, compete with, or build on)

| Project | Overlap | Why not just use it |
|---|---|---|
| **JuiceFS** (Apache-2.0) | POSIX FS over S3, FUSE, S3 gateway, clones, trash, metadata engines (Redis, Postgres, TiKV, SQLite) | Fixed-size blocks (no CDC dedup), metadata lives in an external DB, not per-write versioning, no S3-first extension API or agent model. It is the best reference for POSIX edge cases and a fair "fork it" fallback |
| **lakeFS** | Git-like zero-copy branches over object storage | Built for data lakes, not mounts or desktop apps |
| **Mountpoint for S3** (AWS, Rust) | High-throughput FUSE reads | No rename, weak writes; good reference for prefetch |
| **rclone mount** (VFS cache) | Mount anything | Whole-object semantics, no versioned model |
| **ZeroFS** | NFS and 9P over S3 on SlateDB | Proves the SlateDB approach; no collaboration layer |
| **xet-core** (Hugging Face) | CDC dedup storage in Rust | Built for ML repos; useful for chunking and dedup ideas |
| **Kopia / restic / casync** | CDC plus content-addressed manifests | Backup tools, but the same storage math and GC lessons |
| **LucidLink, Suite Studios** (commercial) | Streaming filesystems for media | Closed; the real competitors in video and AEC |

### 5.3 The macOS mount decision (the riskiest platform choice)

| Option | Byte-range streaming | Install friction | Notes |
|---|---|---|---|
| macFUSE (kext) | Yes | High: reduced-security boot on Apple Silicon | Avoid for consumers |
| **macFUSE 5 with `-o backend=fskit`** | Yes | Low on macOS 26 (non-local volumes supported there; local volumes from 15.4) | Fastest route that reuses the Linux FUSE code |
| Native FSKit extension | Yes | Low (signed app extension) | Most "native," most Swift work, newest API |
| FUSE-T (NFS loopback) | Yes | Low | NFS semantics quirks (locking, xattrs) |
| **File Provider** + `NSFileProviderPartialContentFetching` | Partial fetch is macOS-only | Low; best Finder integration (badges, eviction) | Dropbox- and iCloud-style; strict sync model, harder for "live" shared state |

**Decision (2026-09-26): a native FSKit module, macOS first.** No macFUSE or FUSE-T dependency.
File Provider stays in reserve for Finder-integration features (badges, "keep downloaded").

What that implies (updated 2026-09-27 from the [FSKit spike](spikes/fskit.md)):

- **Minimum macOS 27** (decided 2026-09-27, on the spike's evidence). FSKit mounts network
  ("non-local") volumes from macOS 26, but a drive that other machines change needs
  `FSVolume.setCacheState` to evict what the kernel cached, and that exists only from macOS 27.
  SpaceFS supports macOS 26.4 and later, so voidfs gives up macOS 26 users. So do the `FSVolume.*Handler` protocols (the `*Operations` ones are
  deprecated in 27), `FSClient.mountSingleVolume` and `openFileSystemExtensionsSettings`.
- **Shape.** `voidfs.app` is a SwiftUI menu-bar app. It is the host FSKit requires. It contains:
  - the FSKit module (a thin Swift app extension);
  - a per-user launchd agent that runs the Rust client core (cache, write journal, upload queue,
    change-feed client), linked as an XCFramework with UniFFI bindings;
  - the `voidfs` CLI, installed on the user's `PATH`.

  Users enable the extension once in System Settings (General → Login Items & Extensions → File
  System Extensions). The app can open that pane and check the setting through `FSClient`.
- **The Rust core runs in the agent, not the extension.** The spike found one extension process
  per mounted volume, which exits on unmount and, if it crashes, takes the volume with it (a
  forced unmount, no relaunch). The sandbox lets the extension reach an agent whose XPC service
  name starts with the App Group (62 µs per round trip). The extension keeps a metadata memo and
  reads cached chunks straight from files in the App Group container, so warm operations do not
  pay the hop.
- **Mount flow.** An `FSGenericURLResource` for `voidfs://host[:port]/drive`, mounted with
  `mount -F -t voidfs <url> <folder>` as the logged-in user, the way Apple's own FTP module works.
  Mounting in `/Volumes` through `FSClient.mountSingleVolume` needs the
  `com.apple.developer.fskit.mount` entitlement, which the team's profile does not grant.
  SpaceFS mounts in `/Volumes` through a privileged LaunchDaemon helper instead, and voidfs will do
  the same.
- **Build requirements.**
  - Full Xcode, not just the Command Line Tools.
  - An Apple Developer Program membership. `com.apple.developer.fskit.fsmodule` is a restricted
    entitlement: without a provisioning profile that grants it, AMFI kills the extension at
    launch. Contributors need their own team to run a build.
  - Developer ID signing and notarization. A Developer ID provisioning profile can carry the
    FSKit Module entitlement: SpaceFS ships one, notarized.
- **Risk, as measured by the spike.** Over loopback, the read-only mount streamed a 1 GiB file at
  2.3–2.5 GB/s and listed 1,000 files in 21–33 ms. What remains hard:
  - Kernel caches can only be revoked, not updated. Phase 2 reads open files at the version
    current when they were opened (snapshot-at-open).
  - `RENAME_SWAP` loses data on Apple's FSKit FAT module (it reports success and overwrites).
  - Locks never reach the module.
  - Finder looks up an AppleDouble `._` file for every file.
  - A hung connection blocks callers for the whole request timeout.

  The spike write-up lists each finding with its evidence.

---

## 6. Storage-provider compatibility (bring your own bucket)

What the design needs from a bucket, and where each provider stands. Entries marked "verify"
still need confirming with the capability probe (`voidfs-server probe`) and conformance CI.

| Provider | Conditional PUT (fencing) | Temp scoped creds for direct I/O | Egress | Notes |
|---|---|---|---|---|
| AWS S3 | Yes (`If-None-Match` since 2024-08, `If-Match` since 2024-11) | STS AssumeRole plus session policy | ~$0.09/GB | Every version costs several PUTs: watch request costs |
| Cloudflare R2 | Yes (`If-Match`, `If-None-Match`, wildcard); the probe confirmed a second create-if-absent PUT is refused (2026-09-28) | R2 temporary access credentials (minted with a Cloudflare API token); presigned URLs work | **$0** | Best default for media-heavy users; Space itself appears to default to R2. An S3 token scoped to objects gets AccessDenied reading lifecycle rules, versioning, object lock and CORS, so a server can't check them |
| MinIO / Ceph RGW / Garage | Mostly (verify per version) | MinIO STS; others vary | Self-hosted | On-prem story |
| versitygw 1.8.0 (the local S3 server the benchmarks use) | Yes: a second create-if-absent PUT is refused with 412 (probe, 2026-09-28) | Presigned URLs work | Self-hosted | Lifecycle rules not supported; versioning only with `--versioning-dir` |
| GCS | Generation preconditions (native API; verify in S3-interop mode) | Downscoped tokens (native API) | Paid | Likely needs the native API through OpenDAL |
| Azure Blob | ETag `If-Match` (native) | User-delegation SAS | Paid | Not S3-compatible; OpenDAL covers it |
| Backblaze B2 | Not documented for S3 PUT (verify) | Presigned URLs | Low or free with a CDN | May need single-node mode or an external lock |
| Wasabi, DO Spaces, Tigris, Hetzner, Storj | Verify each | Presigned URLs at minimum | Varies | Run the capability probe and the conformance suite in CI |

Fallback rule: without conditional PUT, the authority must be the only writer, either with a
single-node deployment or with a lease held in Postgres or etcd. A pool created with
`--commit-guard external` says so (format §7.3), and a server won't write a pool that relies on
create-if-absent in a bucket that fails the probe. Without temporary credentials, use per-shard
presigned URLs.

---

## 7. Where an open-source version can beat Space

1. **Free self-hosting and BYO bucket by default.** Space gates "your own storage" behind
   enterprise sales.
2. **An open on-bucket format**, with `export` to plain objects and `adopt` of an existing bucket
   without copying. Space's shard format makes existing buckets unreadable as plain files, and
   moving in means migrating.
3. **A small-file fast path** (inline or packed shards). This is Space's clearest weakness
   (2–3× slower small writes).
4. **An MCP server and agent tooling as first-class,** not just docs.
5. **File locking and offline pinning** for AEC and field work. Neither is offered today.
6. **Windows on day one** via WinFsp, reusing the FUSE code.
7. **Transparent security:** documented encryption, an optional end-to-end mode, and
   customer-managed KMS keys.
8. **A Cloudflare deploy target** for people who want the edge architecture without running
   servers.

---

## 8. Hard problems and risks

1. **Desktop-app semantics.** Premiere, Resolve, Revit, AutoCAD, Office and Adobe apps save
   through temp files plus rename or exchange, keep lock files, mmap, and depend on xattrs and
   resource forks. Each app needs a compatibility test matrix. Plan a dedicated "app compat" test
   rig early.
2. **Concurrent editors on one file.** Last-writer-wins plus version history avoids data loss,
   but two people saving a Revit model will still clobber each other. That needs locks (D7) or
   clear conflict UX.
3. **GC correctness with forks.** Never delete a shard that a version, a fork or an in-flight
   upload still references. Use a grace period for uploads, generation-based mark-and-sweep, and
   extensive property tests. A GC bug is data loss.
4. **Request-cost amplification.** Roughly four PUTs per version on S3 ($0.005 per 1,000 PUTs;
   R2 Class A is $4.50 per million). An app that saves every few seconds creates many versions.
   Mitigations:
   - Coalesce journal writes into a single version per save window.
   - Batch log records into segments.
   - Offer version-retention policies.
5. **Egress on AWS** for streaming media. Steer media users to R2 or put a CDN in front, and make
   costs visible in the UI.
6. **Consistency across mounts.** Change-feed latency, cache invalidation, and what an open file
   handle sees when another machine writes (a snapshot-at-open or live policy).
7. **Credential custody.** Holding users' bucket keys is a large security responsibility. Prefer
   role assumption and short-lived credentials, encrypt at rest, and support a mode where
   credentials never leave the self-hosted service.
8. **macOS distribution:** notarization, FSKit maturity, and macFUSE licensing (macFUSE is not
   fully open source, so check redistribution terms).
9. **Scope creep.** The previews, video review, search and SSO surfaces are each a product in
   themselves. Keep them as separate, optional services.
10. **Naming and trademark.** Resolved: **voidfs**. As of 2026-09-26 the name is free on
    crates.io, npm and PyPI. On GitHub there are only a few tiny unrelated repos (0–1 stars).
    voidfs.com, .dev and .io don't resolve, which does not prove they are unregistered, so check
    with a registrar. Apache-2.0 grants no trademark rights (§6 of the license), so add a short
    name policy (see §12).

---

## 9. Phased roadmap

The current stocktake against SpaceFS and the step-by-step plan to parity are in
[PARITY.md](PARITY.md). It orders the work more finely than this table, and it is newer.

| Phase | Goal | Deliverables | Exit criteria |
|---|---|---|---|
| **0: Specs** | Lock the foundations | On-bucket format spec; wire protocol; conformance case format and cases; `LICENSE`, `NOTICE`, `DCO`, `TRADEMARKS.md`, `SECURITY.md`; `rfcs/` process | **Drafts done 2026-09-26** (35 cases, validator passing). Exit: specs reviewed, RFC 0001 accepted (done 2026-09-27) |
| **1: Engine and S3 gateway** (+ FSKit spike in parallel) | "It's a better bucket" | E1–E8, E11; B1, B3; S1–S5, S8; CLI basics; conformance runner; `docker compose` with SQLite; S3, R2 and MinIO in CI. **Spike:** a minimal FSKit module serving a read-only drive | Stock AWS CLI, boto3 and rclone work; all conformance cases pass on S3, R2 and MinIO; benchmark harness runs. The spike answers §5.3's open questions. **Progress (2026-09-26):** engine, server, SigV4 (including `aws-chunked` and checksums), checkpoints, forks, change feed and conformance runner done. 35/35 conformance cases pass on memory and local disk, including across a restart; boto3 and the AWS CLI work. **2026-09-27:** Cloudflare R2 passes too (35/35 conformance, boto3, restart, exclusive conditional writes under 32-way races). **2026-09-27:** FSKit spike done ([write-up](spikes/fskit.md)): a read-only native mount works, and §5.3's questions are answered. **2026-09-27:** benchmark harness done ([bench/](../bench/README.md)). **2026-09-27:** GC (E8) done, with the protocol amended by [RFC 0002](../rfcs/0002-gc-safe-against-writers.md) after a model check. **2026-09-28:** content-defined checkpoint segments (E11) and the capability probe (B3) done. **2026-09-29:** CI (GitHub Actions) runs the conformance suite, boto3 and rclone over memory, disk, versitygw and MinIO, whose last release it builds from source because MinIO no longer publishes binaries; rclone works except `purge`, which deletes versions; a `docker compose` file; Amazon S3 passes too (35/35 conformance, rclone); virtual-host addressing (`--virtual-host-domain`), with the 35 conformance cases passing in either addressing style. Still to do: a health endpoint and metrics |
| **2: macOS drive** | "It's a drive on my Mac" | D1, D3, D5, D6, D9, D11 (SwiftUI menu-bar host app); change feed; journal and uploads; notarized build | Edit a 50 GB video project and a code repo from two Macs; changes visible within 5 s; app-compat matrix (Finder, Premiere, Resolve, Final Cut, Blender, Office) green |
| **3: Control plane and web** | Multi-user | C1–C4, C5, C6, C8; B4, B5; S6, S9; access keys and scoped storage credentials | A team of 3 on one BYO R2 bucket with scoped keys, share links and an audit trail |
| **4: Agents and search** | "AI-native" | A1 (all four), A2, A3 (MCP), A4, A5; E10; B6 adopt and B7 export | Agent forks, edits and restores through MCP; search across drives |
| **5: Windows (and Linux mount)** | Beyond the Mac | D4 (WinFsp or the Cloud Files API), D2 (Linux FUSE), D10; Windows app-compat matrix (Revit, AutoCAD, Office) | Signed MSI; Windows app matrix green |
| **6: Enterprise and scale** | Enterprise parity | C9, C10, E12, E13, S7, O2, O3, O6; D7 locking; D8 offline | SSO, SCIM, multi-node authority with fencing proven by chaos tests; external security audit |

Phases 1 and 2 carry most of the engineering risk: the engine, and FSKit semantics. Phases 3
onward reuse existing components heavily.

---

## 10. Suggested repository layout

```
voidfs/
  LICENSE  NOTICE  TRADEMARKS.md  CONTRIBUTING.md (DCO)
  spec/                 protocol.md, format.md, CHANGELOG.md, conformance/cases.json
  rfcs/                 numbered proposals for spec changes
  crates/
    core/               chunking, manifests, namespace, versions, forks, GC
    backends/           OpenDAL wrapper, capability probe, credential vending
    authority/          per-drive actor, commit log, leases, change feed
    gateway/            s3s-based S3 server + extensions
    control/            users, workspaces, keys, links, audit (axum + sqlx)
    client/             cache, journal, upload queue, change-feed client (used by every mount)
    client-ffi/         UniFFI bindings → XCFramework for the macOS app
    mount-linux/        (Phase 5) fuser adapter
    cli/                `voidfs` command
    sdk-rust/
  sdks/ts  sdks/python  sdks/go
  apps/macos/           Xcode project: SwiftUI menu-bar host app + FSKit module extension
  apps/windows/         (Phase 5)
  apps/web
  services/search  services/previews  services/mcp
  deploy/compose  deploy/helm  deploy/cloudflare
  bench/                same-bucket-with-and-without harness
```

---

## 11. Decisions

### Made

| Decision | Choice | Consequences |
|---|---|---|
| Name | **voidfs** | CLI `voidfs`; crates `voidfs-*`; npm `@voidfs/*`; PyPI `voidfs`; headers `x-voidfs-*` |
| License | **Apache-2.0** for code, SDKs and specs | Explicit patent grant; compatible with OpenDAL, s3s, SlateDB and Tantivy. Does not stop anyone from reselling voidfs as a service, which is accepted |
| Hosted offering | **None for now** | Billing, abuse handling, a managed control plane and multi-tenant hardening are out of scope. Design for one organization per deployment, and keep workspaces so a team can still split drives |
| Contribution sign-off | **DCO** | `git commit -s` on every commit; no CLA. Relicensing later would need every contributor's consent, which is accepted |
| macOS mount | **Native FSKit module**, **minimum macOS 27** (2026-09-27) | 27 rather than 26 because only 27 lets a module evict kernel caches when another machine changes a file ([FSKit spike](spikes/fskit.md)). SpaceFS supports 26.4. Needs full Xcode and a provisioning profile with the FSKit Module capability; see §5.3 |
| macOS client core | **A per-user launchd agent, reached over XPC** (2026-09-27, from the [FSKit spike](spikes/fskit.md)) | The FSKit extension stays thin: a metadata memo, cached reads from App Group files, everything else through the agent. The agent serves an App-Group-prefixed XPC service and owns the cache, journal, uploads and change feed |
| Metadata engine | **Follow Space's architecture for now** ([RFC 0001](../rfcs/0001-metadata-in-the-bucket.md), accepted 2026-09-27) | Metadata as a commit log plus checkpoints in the user's bucket, in the same shape as Space's documented design, with fewer writes per version. Revisit when benchmarks exist |
| Platform order | **macOS → (control plane, agents) → Windows and Linux mounts**; full Mac parity before the Linux mount (2026-09-27, [PARITY.md](PARITY.md)) | Linux servers and agents use S3 and the SDKs until the Linux mount lands. The desktop host app is SwiftUI on macOS; a cross-platform shell (such as Tauri) is revisited when Windows starts |

### Still open

1. **Space compatibility shim.** Whether and when to also accept `x-s3sdk-*` (see §12.8).

---

## 12. Publishing the protocol: standard practice, applied to voidfs

voidfs has two public contracts, and both need a spec. The **wire protocol** is what clients send.
The **on-bucket format** is what sits in the user's bucket. The format matters more for the "no
lock-in" promise: anyone must be able to read a drive without the voidfs server.

### 12.1 Use your own namespace, and speak standard S3 for everything else

This is what every S3-compatible store does. They implement AWS's `x-amz-*` headers exactly,
because clients depend on them, and put their own extensions under their own prefix: MinIO
uses `x-minio-*`, Google Cloud Storage uses `x-goog-*`, and Cloudflare R2 uses `cf-*` headers.

For voidfs:
- **Standard S3 unchanged:** `x-amz-version-id`, `If-Match`, multipart, and so on.
- **Extensions:** query parameters `?x-voidfs-write`, `?x-voidfs-patch`, and so on, plus headers
  `x-voidfs-if-version`, `x-voidfs-offset`, `x-voidfs-as-of`, and so on. They are included in
  the SigV4 signed headers.
- **Errors:** the standard S3 XML body, with voidfs-specific codes documented.

Do not use `x-s3sdk-*` as the primary namespace. It belongs to another vendor's product, and
their future changes would then constrain yours.

### 12.2 Keep specs in the repo, normative and separate from the docs

- `spec/protocol.md` and `spec/format.md` are written as normative documents with RFC 2119 /
  RFC 8174 keywords (MUST, SHOULD, MAY).
- Each spec has a changelog (`spec/CHANGELOG.md`).
- User docs and guides live elsewhere and link to the spec. The spec does not explain how to use
  things; it defines what is correct.
- Apache-2.0 covers the spec text too. This is the same choice OCI (the image and distribution
  specs), OpenAPI and CloudEvents made, and it is simpler than a separate CC-BY license.

### 12.3 Version the protocol separately from the software

- **Wire protocol:** an integer version, advertised on every response as
  `x-voidfs-protocol: 1`. Additive changes (a new optional header, a new operation) keep the
  number. Breaking changes need a new major version and a deprecation window in which the
  server supports both.
- **On-bucket format:** every object carries a magic value and a version (like the patch body's
  `S3SP`/`1` in Space). Each drive has a root `format.json` that lists `format_version` plus
  feature flags in two sets:
  - `compatible`: readers may ignore these.
  - `incompatible`: a reader that doesn't know one MUST refuse to open the drive.

  This is the ext4 feature-flag and Git `extensions.*` pattern. It lets the format grow without
  silent corruption by old clients.
- **Stability:** label the specs **draft** and keep crates at `0.x` until the Phase 1
  conformance suite is green on S3, R2 and MinIO. Then declare `protocol 1` and `format 1`
  stable.

### 12.4 Ship a conformance suite as part of the spec

- Store language-neutral cases in `spec/conformance/cases.json`. Each case is a sequence of
  requests with expected status codes, headers and bodies. Any server can run them, and every
  voidfs SDK runs them in CI. Space runs 16 cases; aim for broader coverage, including
  path-conflict rules, forks, GC safety and consistency.
- Run the de facto S3 compatibility suite, `ceph/s3-tests`, for the standard-S3 subset, and
  publish which tests pass. Third parties can then check claims.
- Rule: **no spec change merges without a conformance case** that exercises it.

### 12.5 A lightweight change process

- Changes that affect the spec go through a numbered proposal in `rfcs/`: motivation, wire and
  format changes, compatibility impact, conformance cases.
- Small or editorial fixes go straight to a pull request.
- Keep it light. The Rust RFC style is the model, without the ceremony. A foundation-style
  process can come later if other implementations appear.

### 12.6 Publish a trademark and name policy

Apache-2.0 §6 grants no rights to the project name. Add a short `TRADEMARKS.md`:
- Forks must rename.
- Third-party implementations may say "compatible with the voidfs protocol v1" if they pass
  the conformance suite.
- The name may not be used to imply endorsement.

This is the norm for Apache-licensed projects that are not under a foundation.

### 12.7 Machine-readable surfaces from day one

- `spec/operations.json`: every operation, its request shape, its cost, and examples.
- `llms.txt`: a plain-text index of the docs for AI agents.
- Generate SDK method stubs and the docs from these files, so the spec, the SDKs and the docs
  cannot drift apart. Space does this well; copy the practice, not their files.

### 12.8 Space compatibility (optional, later)

Interoperating with a published API is common: every S3 clone does it with AWS. Space's SDKs are
MIT-licensed. If migration from Space becomes a real user need, add a compatibility mode that
also accepts the `x-s3sdk-*` spellings and maps them to voidfs operations. Keep it:
- off by default,
- documented as a compatibility layer,
- free of Space's names or marks in voidfs's own branding.

Keeping voidfs semantics close to Space's (version per mutation, `if_version`, `as_of`, splice,
rename, fork) makes that shim cheap. This is not legal advice; if the shim ships, have someone
check it.

---

## Sources

- Space homepage, pricing and FAQ: https://spacefs.com/
- Enterprise: https://spacefs.com/enterprise/
- Industries: https://spacefs.com/industries/
- Changelog: https://spacefs.com/changelog/
- Terms: https://spacefs.com/terms/
- Developer docs (index, full text, operations catalogue): https://docs.spacefs.com/ ·
  https://docs.spacefs.com/llms-full.txt · https://docs.spacefs.com/operations.json
- Architecture: https://docs.spacefs.com/how-it-works/architecture/
- Protocol v1: https://docs.spacefs.com/protocol/model/ (and the sibling sections)
- Benchmarks: https://docs.spacefs.com/benchmarks/ · https://docs.spacefs.com/benchmarks/global/
- Funding announcement: https://www.globenewswire.com/news-release/2026/08/18/3347063/0/en/space-raises-2-4m-led-by-a16z-speedrun-to-build-the-ai-native-filesystem-for-humans-and-agents.html
- SDK licenses: npm `@spacefs/s3sdk`, PyPI `spacefs-s3sdk`, crates.io `spacefs-s3sdk` (all MIT)
- macFUSE FSKit backend: https://github.com/macfuse/macfuse/wiki/FUSE-Backends · https://macfuse.github.io/2026/09/07/macfuse-5.4.0.html
- File Provider partial fetch: https://developer.apple.com/documentation/fileprovider/nsfileproviderpartialcontentfetching
- R2 conditional writes: https://developers.cloudflare.com/r2/api/s3/extensions/ · https://developers.cloudflare.com/r2/platform/release-notes/
