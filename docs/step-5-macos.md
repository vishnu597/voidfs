# Step 5: a writable macOS drive

*Proposed 4 October 2026, after step 4; architecture, integrity and local-save decisions accepted
5 October; moved-snapshot resolution updated 6 October. Item 1 has begun with the Rust mount
namespace and read-only snapshot handles. The later deliverables remain an
implementation plan; no writable adapter, platform service installation, new protocol field or
format feature is delivered by this slice.*

Step 5 is **done when** the daemon mounts a writable drive from the same durable client core the
CLI uses, Finder and the target applications can save through it, and a signed, notarized app can
bring the drive back at login. A 50 GB video project and a code repo must work across two Macs:
changes appear within 5 seconds of a successful server commit, new opens see the new version,
and existing opens keep a consistent version. Local saves survive a crash before cloud publish.
The application matrix must pass, with limitations stated rather than hidden.

The scope follows [PARITY §7](PARITY.md#7-step-by-step-plan),
[the FSKit spike §4–§8](spikes/fskit.md#4-answers-to-53s-open-questions),
[the architecture plan §5.3](RESEARCH_AND_PLAN.md#53-the-macos-mount-decision-the-riskiest-platform-choice)
and [step 4's Mac observations §1.3](step-4-client.md#13-the-mac-app-observed-through-computer-use-1-october-02333).
Those observations of SpaceFS are dated evidence already in the repository. This plan does not
require another SpaceFS session, account or mount.

## 1. Starting point and decisions

Step 4 supplies `voidfs-client`'s versioned cache, durable operation journal, upload queue,
change-feed watcher and connectivity state, plus `voidfs-daemon` and its HTTP/JSON Unix socket.
`void mount`, `unmount` and `mounts` already use the daemon's live and remembered mount tables.
The extension in `apps/macos` is a read-only spike using its own Swift HTTP client.

The adapter boundary already exists in
[`crates/voidfs-daemon/src/mounts.rs`](../crates/voidfs-daemon/src/mounts.rs):
`Adapter::mount(MountSpec, Core)` returns a `Mounted`, which receives invalidations and unmounts.
`Core` contains the SDK client, cache, queue and connectivity. Builds supply adapters through
`DaemonConfig.adapters`; an empty list returns `NoAdapter`. The first adapter is the current
default. A remembered mount names its adapter explicitly.

The core now supplies a persistent namespace and read-only snapshot handles. A complete local
overlay and a read path combining unpublished writes with a remote base remain to be built in
the shared Rust core before writable FSKit or SMB callbacks are added.

| Decision | Choice or remaining recommendation | Alternative and consequence |
|---|---|---|
| FSKit, SMB or both | **Accepted 5 October: FSKit first, keep the adapter boundary, evaluate SMB only if the compatibility gate fails.** Preserve the native-module decision and the existing spike. | SMB first could avoid FSKit-specific cache and entitlement constraints, but needs a reusable SMB server, a license review and proof of Mac semantics. Shipping both now doubles adapter and coherence validation. SpaceFS's default SMB is evidence to evaluate, not evidence that voidfs's SMB adapter already exists. |
| Minimum OS | **Keep macOS 27 for FSKit.** Recheck the final SDK/runtime before release. | Any lower minimum requires a separate coherence strategy and an explicit change to the recorded platform decision. An SMB adapter would need its own measured minimum; it does not automatically lower the app's minimum. |
| Rust/Swift boundary | **Accepted 5 October: keep the Rust daemon and socket; add a thin Swift XPC bridge for the sandboxed extension.** | A Swift agent embedding Rust through UniFFI follows the original spike, but replaces the now-built daemon host and adds bindings/build work. A direct extension-to-socket path is smaller only if a signed sandbox test proves access and notifications. |
| Cache and journal ownership | **One daemon owns mutable state; shared cache files are read through leases.** | An extension-owned core repeats state per drive and loses its process on eject. Neither a bridge nor an extension may independently open the writable state database. |
| Partial-shard integrity | **Accepted 5 October: keep verified whole-shard reads; authenticated pieces require a later RFC.** | Length-only ranged reads fit today's format but weaken content-address verification. They require a separate policy decision and must not silently replace the verified path. |
| `/Volumes` | **First prove a user-owned mountpoint; add a narrowly scoped privileged mount helper later.** | A release may use user-owned folders if the helper is not ready, but that does not complete the `/Volumes` deliverable in PARITY. Do not make all client I/O privileged. |

The adapter, bridge and integrity choices are settled. The other recommendations preserve the
existing plan or describe implementation requirements. The user also accepted these item 1
policies on 5 October; writes and conflict handling arrive in later slices.

| Decision | Accepted policy |
|---|---|
| Names | New writes store NFC names, with equivalent NFD lookups, using the approved `unicode-normalization` dependency. Existing remote names stay byte-exact. If multiple names normalize alike, lookup reports ambiguity rather than choosing one. Names remain case-sensitive. |
| Conflicts | Preserve the local saved data and remote version locally until the user resolves the conflict. Do not automatically publish a conflict sibling visible to other clients. |
| Write acknowledgement | `write` returns after staging bytes that survive a daemon or extension crash. `fsync`, `F_FULLFSYNC` and `close` flush bytes and metadata to disk. Cloud publication has separate status. |

### Platform evidence, checked 4 October

Apple's [FSKit updates](https://developer.apple.com/documentation/updates/fskit) describe the
new handler protocols and kernel data caching. Its
[`setCacheState` reference](https://developer.apple.com/documentation/fskit/fsvolume/setcachestate(for:cachemode:coherencytype:action:))
describes module-initiated cache changes and requires calls without module locks held, because
the kernel may call back. The retrieved reference still carries a beta label. The macOS 27
minimum and the observed revoke/negative-cache behavior come from the spike's SDK and runtime
tests; do not treat that test of one seed as proof of every release.

Apple documents an [FSKit module entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.fskit.fsmodule),
[`NSXPCConnection` to a launchd agent](https://developer.apple.com/documentation/foundation/nsxpcconnection/init(machservicename:options:))
and [SMAppService](https://developer.apple.com/documentation/servicemanagement/smappservice) for
bundled login items, agents and daemons. The App Group service name working from the sandbox,
and profile enforcement, are spike observations. Verify the actual signed voidfs bundle and
team profile early; the entitlement documentation alone does not prove distribution readiness.

SMB leasing and lease breaks are part of Microsoft's
[SMB2 protocol](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/c367fad4-c00f-4778-913d-c0560ead1360).
This makes a loopback server an option for mapping feed changes to cache invalidation, but is
not proof of macOS client behavior or a suitable Rust implementation. That proof is an SMB gate.

## 2. Ordered deliverables

### Item 1. The mount core and local filesystem contract

Build the adapter-independent mount session in `voidfs-client`, reused by `voidfs-daemon`.

- Stable local inode identity, a persisted namespace overlay, explicit directories, NFC names
  for new writes with equivalent NFD lookups, and case-sensitive collision rules. Keep existing
  remote names byte-exact and reject ambiguous normalized lookups. Do not derive an open handle's
  identity solely from a path: rename and open-unlink must preserve it.
- Open, close, lookup, enumeration, getattr, read, write, truncate, rename-over, remove, mkdir
  and attrs/xattrs operations with one error mapping contract. Unsupported hard links,
  exchange, cloning and cross-machine locks must be advertised and tested explicitly.
- A read-only open binds object identity, version id, size and attributes together. It reads
  that version through `Cache::reader`/`Content`, not a succession of current-version GETs.
  A writable open uses that remote base plus its local staged generation; same-Mac reads see
  local edits immediately while external committed versions wait for a new open.
- Apply every acknowledged local mutation to durable namespace/data state as well as the
  journal. Retain data needed by open handles after publish, rename or unlink; lease release
  controls cleanup. Reconstruct the overlay from disk after a daemon restart.
- Admit local writes only when bytes and metadata can be made durable within the configured
  disk budget/reserve. Return a storage error on full disk; do not acknowledge an unrecorded
  write. Mark pending, saving, conflict and error states for the UI.
- Reuse queue guards and the existing `412` policy. Keep both versions locally on a conflict
  until the user resolves it, and prevent a later unguarded publish from silently overwriting
  either version. Conflict UI remains a later slice; no automatic remote conflict copy is planned.

**Done when:** local operations against a temporary store pass save/rename/open-unlink and
crash-recovery cases without an OS mount, and reads agree with the durable overlay while upload
is paused. Kill points cover staged bytes, recorded metadata, publish and cleanup, especially a
published entry still held open.

### Item 2. Daemon sessions and the Swift transport bridge

Extend the daemon with bounded, versioned local filesystem RPCs. The existing `/v1/` status,
uploads and mounts remain the app and CLI's control plane; the adapter must not create a second
upload queue or remembered-mount store.

- Map an adapter session to a drive, read-only policy and lifetime. Use opaque session/handle
  ids, daemon-generation handshakes and structured errors. Reconnect after interruption must
  detect stale handles, rather than reuse ids from the former process.
- Bound request sizes, concurrent calls and read-ahead; carry binary read/write data without
  JSON expansion. Keep controls responsive during transfers and support cancellation safely.
- Add a per-drive feed subscription shared across mounts and observers where practical. The
  current mount table starts a watcher per mount; avoid extra watchers in the Swift bridge.
  Send monotonic metadata generations and a full-resync signal after a feed gap/reconnect.
- Add a Swift launchd agent with the tested App Group Mach-service prefix. It forwards to the
  Rust daemon's user-only socket and relays invalidations. It owns no journal or publisher.
  Authenticate peers and validate allowed RPCs and paths; a bridge is not an arbitrary proxy
  for every local socket operation.
- Keep a generation-tagged metadata memo in the extension. Initially return read bytes over
  the bridge. Add direct App Group cache-file reads only after a cache lease API prevents
  eviction/replacement while an extension reads. Never expose arbitrary daemon file paths.
- Define a migration for existing CLI daemon state to the app's shared container, without
  losing queued bytes or remembered mounts and without two daemons owning separate stores.

**Done when:** a signed sandboxed extension can create a session, read/write bounded payloads
and receive a generation update through XPC → socket → daemon. Restarting the bridge and daemon
has known outcomes and leaves the journal recoverable. Record warm metadata and cache-hit hop
costs against the spike before optimizing direct file access.

### Item 3. Read-only FSKit on the daemon, then coherence

Replace the spike's Swift HTTP/cache/feed ownership with the session from items 1–2. Keep
`FSGenericURLResource` and the secret-free resource URL for mounting in a user-owned folder.
Implement an `Adapter` named `fskit`; remove spike-only lookup names and echo instrumentation.

- Return every required attribute in handler results. Recheck runtime selectors and protocol
  availability against the actual Xcode/macOS combination; remove seed workarounds only after
  a reproducible mount proves they are unnecessary.
- Snapshot-at-open reads must include metadata and data. On feed changes, expire memos,
  revoke changed items, then affected parents. Retry parent revokes when open children close;
  invalidate parent listings when items are reclaimed.
- Invoke `setCacheState` outside module locks. Treat known benign unknown-item failures
  separately from coherence failures. A missed generation triggers a relist/resync.
- Test an external overwrite that grows and shrinks a file, plus create after a negative
  lookup, rename and delete, with siblings held open. Existing open files must not combine
  an old length with new bytes. Document any negative-name visibility limit that remains.
- Cached metadata and bytes may be served offline; uncached reads fail promptly. Reconnection
  resumes the feed and refreshes state before declaring the mount current.

**Done when:** the daemon-backed read-only mount matches the spike's listings and byte hashes,
has a recorded cold/warm read table, and a second client exercises the coherence cases. If the
open-sibling case prevents the 5-second requirement, this is a release blocker and the SMB
decision is reopened before adding more app surface.

### Item 4. Writable FSKit and desktop semantics

Map writable handlers to the durable local operations, not directly to remote SDK mutations.
Return write success after staging that survives a daemon or extension crash; cloud publish is
visible separately. `fsync`, `F_FULLFSYNC` and close flush bytes and metadata to disk. Synchronize
and eject must have documented, tested durability semantics too.

- Create/write/truncate, sparse regions, directory mutations and atomic save via rename-over;
  retain the replaced object's history and the old object for existing open handles.
- Full xattr create/replace/remove/list behavior, including empty values, resource forks and
  `com.apple.provenance`. Fold create-following metadata into the same local save/publication
  where ordering permits. Never reject every xattr and force AppleDouble fallback.
- `.DS_Store` is local-only. Define treatment of `.localized`, `.hidden`, Spotlight and other
  probes explicitly; only known local housekeeping names bypass remote storage. Test legitimate
  user data so name filtering does not silently discard it.
- Re-run the corrected `semantics.c` tests for `RENAME_EXCL`, `RENAME_SWAP`, open-unlink,
  mmap/msync, locks and Unicode. Advertise unsupported operations accurately. The spike's FAT
  swap conclusion was unconfirmed after a faulty probe was discovered; it is not a release fact.
- Locks remain per-Mac until a separate server locking design exists. Present guarded-save
  conflicts; do not imply that two mounts coordinate `flock` or `fcntl` through FSKit.

**Done when:** Finder copies and edits, atomic-save probes, mmap writes and xattr round trips
pass; killing the extension or daemon after acknowledged writes loses no local bytes. Recovery
publishes the same journal without creating an unintended duplicate revision.

### Item 5. Pieces of shards, after the integrity decision

Keep the gateway's existing whole-shard path. Measure pieces in the client first, using the
[options and evidence](../bench/results/shard-fetch/README.md#ranged-shard-reads-options-not-built).
The direct bucket reader knows shard extents; the API-only reader does not. A pieces optimization
must preserve API fallback when storage credentials are unavailable or expire.

Recommended RFC: retain SHA-256 shard identities and add fixed-block hashes, initially evaluate
64 KiB blocks, with their verification metadata cryptographically bound to the descriptor/extent
chain. A sidecar named only by the shard hash is insufficient: an attacker can replace both
piece and sidecar unless an authenticated digest/proof binds them. Compare sidecar overhead
with a tree-proof alternative before choosing the format. Specify feature negotiation, old-pool
fallback, writer ordering, direct-upload commit checks, forks and GC reachability together.

After the RFC is accepted, implement aligned range GETs of at most 1 MiB plus the needed proofs,
verify before returning bytes, and keep piece cache/in-flight keys separate from whole shards.
Keys include shard identity, aligned range and verification scheme. Reject wrong length/range,
truncation and hash/proof mismatches, and coalesce concurrent misses with bounded memory.
Sequential readers retain whole-shard fetching/read-ahead when that costs fewer requests.

**Done when:** corruption cases return errors before bytes are exposed, concurrent identical
misses use one piece fetch, cold random 4 KiB/64 KiB reads move fewer bytes and have lower p50/p99
than whole-shard reads in the same local latency/bandwidth setup, and sequential throughput and
API fallback remain within the measured baseline. Publish request, memory and proof costs too.

Without an accepted format decision, verified whole-shard reads can ship the writable mount;
the pieces deliverable stays explicitly pending. Length-only pieces need separate user approval
of the integrity tradeoff. Background whole-shard checking cannot retroactively verify bytes
already returned. An object's shard-list API or presigned-read fallback also needs its own RFC;
both were deferred in step 4 and are not implied by this plan.

### Item 6. App compatibility and the SMB gate

Start this rig alongside item 3, and run it again after item 4. Use Finder, a code repo and
Premiere, Resolve, Final Cut, Blender and Office on the target macOS. Record app/OS versions,
operation, resulting bytes/metadata, recoverability and known limitations. Missing paid apps or
a second Mac are untested rows, not passes. A second independent SDK client can cover server/feed
logic locally but cannot prove the two-Mac application exit criterion.

If FSKit fails a required save or coherence case, make a contained SMB feasibility spike before
choosing it as default: select a maintained reusable server and check its license, bounded reads,
write/flush callbacks, xattrs/streams, Unicode/case behavior, rename-over, delete-on-close and
lease/oplock/change-notify hooks. Test the macOS SMB client against the same mount core and rig.
Bind the server to loopback with per-user authentication, and ensure a remote peer cannot reach
it. Measure caches, lease breaks and open-handle snapshots under feed changes rather than
assuming protocol support proves behavior.

**Done when:** the chosen default adapter passes the matrix and the two-Mac scenario. If both
adapters ship, both need the same durability and coherence checks, an explicit default, and
adapter-specific remembered mounts. There is no silent adapter switch for a live writable mount.

### Item 7. `/Volumes`, launch services and credentials

Package the Rust daemon and Swift bridge inside the app. Register bundled services through
SMAppService, handle disabled/approval-required states and guide extension enablement. Keep the
existing terminal launchd installation compatible; define how app installation takes ownership
without duplicate services. Store app credentials in the shared Keychain rather than the spike's
JSON file. Do not put secrets in mount URLs, arguments, status output or logs.

Add the privileged helper only for creating/validating mountpoints and mounting/unmounting in
`/Volumes`. Authenticate the requesting app/user and allow only voidfs's intended resources and
paths, including symlink/race checks; bucket access and journaling stay in the per-user daemon.
Test authorization failure, helper absence, busy eject, orderly stop and login restoration.

**Done when:** the app brings remembered mounts back after logout/login and reboot, a user can
eject a busy drive without losing acknowledged local writes, and the helper operates only on
the permitted voidfs volumes. Registration and credentials testing are implementation-time
actions, not actions performed by writing this plan.

### Item 8. Menu-bar transfers and Finder Sync

Use the existing daemon status/upload stream for mounts, connectivity, queue progress, per-item
and global pause/resume, cancel and immediate bandwidth caps. Distinguish local durable saves
from cloud completion and from conflicts. Ordinary eject leaves journaled publication available
to the running daemon; stopping the daemon resumes from disk when it returns. Define and show
the fate of unpublished mounted-file writes before exposing cancellation for them.

Add Finder Sync for pending, saving, saved and error/conflict badges and relevant context actions,
with one daemon as the state source. Apple's
[Finder Sync guide](https://developer.apple.com/library/archive/documentation/General/Conceptual/ExtensibilityPG/Finder.html)
supports folder-scoped badges and menus and recommends placing synchronization in a separate
service. Pin/unpin UI is an explicit scope choice: the cache has pin primitives, but full offline
pinning is still after parity in PARITY and is not silently added here.

**Done when:** Finder and the menu bar agree with CLI status, queue controls affect live mounts,
and states survive app/bridge/daemon restart. Badge requests for a folder do not issue one remote
request per file. The app stays usable while offline and while a large upload runs.

### Item 9. Protocol additions only where measured

The spike proposed xattr names in `?x-voidfs-list` so provenance-bearing folders avoid an attrs
request per file. Begin with memoized/batched metadata and measure Finder's request count. If
the remaining fan-out justifies the field, propose a separate protocol RFC with exact absent
versus empty semantics, pagination, permissions and compatibility. Implement only after the RFC,
with spec text, a conformance case and changelog as
[CONTRIBUTING §3](../CONTRIBUTING.md#3-changes-to-the-specs) requires.

**Done when:** 1,000/10,000-file listings and xattr-heavy Finder folders have recorded request
counts and timings. Any published field is specified and covered by conformance. This item can
be deferred if local memoization meets the measured target without changing the protocol.

### Item 10. Distribution and the exit report

Prove Developer ID FSKit provisioning for voidfs's team early, then produce the hardened,
notarized arm64 app with stapled ticket, daemon/bridge/helper/Finder extension and CLI. Document
full-Xcode, team/profile and contributor requirements. Test a clean Mac installation, one-time
extension enablement, services approval, update with unpublished writes, uninstall and recovery
of retained local state. Do not delete journaled user data on ordinary uninstall or cache clear.

**Done when:** the clean-machine path passes, the application matrix and two-Mac 50 GB/code-repo
scenario pass, and a results page records build/OS identities, cold/warm listing/read timings,
time from server commit to visible change, crash/disk-full/offline/conflict behavior and remaining
limits. Credential fallback must pass even when a backend cannot mint scoped storage credentials.

## 3. Implementation slices

Item 1 has started; the architecture and integrity choices above no longer block it. Its
namespace and snapshot handles come first, followed by staged writes, guarded publication and
recovery in separate slices. Once snapshot/coherence and restart behavior are demonstrated,
item 2's read-only transport and item 3's read-only adapter may proceed alongside those writable
core slices, at a user-owned mountpoint. Establish the signed-bundle probe on voidfs's own Apple
team before the sandboxed bridge and adapter, and the compatibility rig in parallel.
Only after snapshot/coherence and restart behavior are demonstrated should writable callbacks
land. Pieces, `/Volumes`, UI and distribution follow the same core; each remains a separate
reviewable deliverable.

### Namespace slice, 5 October

[`voidfs-client::mount::Session`](../crates/voidfs-client/src/mount.rs) supplies `lookup`,
`getattr` and `readdir`, plus one `FsError` contract mapping to native errno values. Store migration
4 persists inode identities by drive and remote `object_id`, directory names, attributes and
generations. Renames and new versions keep the inode; removing a name retains its identity record
across restarts. A replacement object gets a distinct inode. The separate local overlay and its
tombstones take precedence over remote names and reserve the namespace for later writable work.
Callers must identify the drive by its stable canonical id, rather than a reusable display alias.

Directory snapshots combine all pages of `?x-voidfs-list` with attributes. Pages must agree on
their prefix and sequence; a feed invalidation during the refresh prevents stale publication.
Successful refreshes advance the directory generation and a persisted drive sequence, so an older
response from another session or directory cannot move an inode back or replace newer attributes.
Retries are bounded and return `EAGAIN` when the namespace keeps changing. Async invalidation
persists stale state, increments generations and returns known affected inodes and directories.
The caller supplies feed invalidations; the session does not start a watcher. Opening a session
marks persisted directories stale for online refresh. Offline metadata is served only from a
previously complete directory snapshot; folders never listed fail promptly. Enumeration keeps remote
names and byte order, with an exclusive name cursor within the current generation.
Adapters must track invalidations and restart enumeration when the directory generation changes.

The original namespace slice had no open handles, file reads or writes, staged-data recovery, cache integration,
daemon transport or FSKit adapter. Item 1's save, open-unlink and byte-recovery exit criteria are
still pending, and no step 5 item is complete.

Namespace validation must cover attributes and paging, stable inodes across version changes,
rename, deletion and restart, complete offline snapshots, fresh online reopening, feed generation
races, overlays and tombstones, Unicode ambiguity and errno mapping. Each regression test must
fail with its relevant behavior broken. Workspace tests, clippy, spec validation and memory, fs
and versitygw interoperability remain required before the pull request is complete; results belong
in the pull request after they run.

### Open handles and snapshot reads, 5 October

`Session::new` now receives the daemon core's shared `Cache`. Read-only `open` refreshes the
inode's ancestors and captures its object identity, version id, ETag, size and attributes together
from an accepted namespace generation. A feed invalidation that wins before capture forces a
retry. A handle keeps those attributes and bytes after remote overwrites, renames and file
deletions; a new open observes refreshed metadata. Write opens return `EROFS`, directories return
`EISDIR`, and symlinks expose their stored target through `readlink`.

Handle ids combine a durable session epoch from the state database with a monotonic counter.
The epoch advances transactionally when a session starts and is available to a future reconnect
handshake. Closed or unknown handles from the current epoch return `EBADF`; handles from a
previous session return `ESTALE`. Exhaustion is an error instead of reusing an id. Each handle
owns one `Cache::reader` behind an async mutex, preserving sequential read-ahead while making
concurrent reads correct. Reads stop at the captured size, including empty and past-EOF reads.
Closing releases the table's reference; a read already in progress may finish.

The reader checks connectivity only after checking the cache: verified memory/disk blocks
remain available offline, while misses and queued read-ahead fail before asking a fetcher.
Content mismatches are logged; a confirmed mismatch at the bound object's own key returns `EIO`.
Both `ApiFetcher` and `BucketFetcher` use this path; direct reads retain whole-shard integrity
checks and the existing API fallback where scoped bucket access is unavailable.

Retaining a version does not make its original key a permanent address: a rename removes that
key from the current namespace. After a version-read `404` or content mismatch, the session first
refreshes the inode's ancestors and retries the same version and ETag at the namespace's current
path when it differs. A bounded fallback uses existing attribute/deleted listings for a move
the namespace has not seen; its limits are recorded below. A confirmed mismatch at the bound
object's own key remains `EIO`; inconclusive resolution that exhausts a budget returns `EAGAIN`.
This changes no server or wire protocol behavior. The current API removes
folders only after their children are gone.
If later work introduces a recursive removal represented by one retained parent, a cold
path-addressed read of its children would need an object-addressing decision: the retained parent
alone does not expose their versions through the existing API. Cached blocks remain readable.

This slice also addresses the namespace review: a missing name in a complete offline listing
returns `ENOENT`, reserving offline creates for the writable slice; folders never listed still
return `ENETDOWN`. Migration 5 stores indexed NFC forms for names without changing their remote
bytes. Equivalent lookup probes only matching rows and still rejects ambiguity. Feed
invalidation follows indexed parent/name relationships and traverses only an affected subtree,
instead of reconstructing every inode's ancestor chain. Directory refresh locks are per inode,
so unrelated folders may list concurrently while generation checks reject stale answers.

Validation covers snapshot data and attributes after overwrite, rename and file deletion, EOF,
concurrent and sequential reads, cached/uncached offline reads, stale handles across restart,
symlinks, namespace generation races, relocation scans and both fetchers. Each new regression
test must fail with its behavior broken; timing-sensitive breaks run at least three times.
Namespace measurements at 10,000 and 100,000 entries record query counts and lookup/listing
times in the pull request. Staged writes, guarded publish, conflicts, recovery, daemon session
RPCs and the Swift/FSKit adapter remain pending; no step 5 item is complete.

### Moved snapshot resolution, 6 October

This is a small prerequisite fix before slice 3's staged writes. `404` and content-mismatch
recovery first refresh the bound inode's ancestors and read its current namespace path by object
identity. A delivered rename can therefore resolve without traversing unrelated folders or the
deleted listing. Relocation changes only the reader's key: the handle retains the version, ETag,
size and attributes captured at open.

The fallback reads deleted entries one page at a time, then traverses attribute listings. Each
fallback allows up to four sequence attempts, all charged to the same per-read budget of 16
logical listing calls across relocations, including initial and final root checks. SDK retries
inside a listing call may make additional HTTP requests. A shared budget of 2 seconds spent on
resolution bounds namespace refresh and fallback work; ordinary data fetches do not consume it.
Exhausting either budget returns
`EAGAIN`, without claiming the object is absent. Page prefixes, continuation tokens and a stable
drive sequence are checked before accepting a fallback result.

If a `404` resolves to the key that just failed, the read stops with `ESTALE`; it does not repeat
the scan. A confirmed content mismatch at the bound object's own key remains `EIO`; inconclusive
resolution that exhausts a budget returns `EAGAIN`. Cached bytes remain
readable offline; an uncached read still fails promptly. Validation must cover namespace-first
relocation for both fetchers, immutable snapshot attributes, paginated deleted entries, request
and time budgets, sequence churn, and same-key termination. No step 5 item is complete.

Accounts, web, search, previews, video review, Linux/Windows mounts, server locking, retention and
encryption remain in their later steps. Cloud benchmark runs and new provider credentials do not
block local mount/core development. Passing step 4 with storage-credential cases skipped is not
proof of the direct bucket path; step 5 must exercise both direct reads when supported and the
already-built server fallback.
