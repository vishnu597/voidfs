# Step 5: a writable macOS drive

*Proposed 4 October 2026, after step 4; architecture, integrity and local-save decisions accepted
5 October; local namespace changes and staged file data added 6 October; Rust daemon sessions
and shared feeds, guarded publication and retained conflicts added 7 October; recovery and
advertised capabilities, setting mode and mtime, and recognizing lost replies to edits, renames
and attribute changes added 8 October; the signed-bundle probe and the rest of the session calls
added 9 October. **Item 1 is complete**
([8 October](#setting-mode-and-mtime-8-october)): the Rust mount namespace, snapshot handles,
durable namespace mutations, staged writes, publication reconciliation, recovery, advertised
capabilities and attribute changes. Item 2 has begun: the daemon's sessions carry every
mount-core call. The later
deliverables remain an
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
`Core` contains the SDK client, cache, queue, connectivity and the shared per-drive `Session`. Builds supply adapters through
`DaemonConfig.adapters`; an empty list returns `NoAdapter`. The first adapter is the current
default. A remembered mount names its adapter explicitly.

The core now supplies a persistent namespace, snapshot handles, a writable local namespace
overlay and staged file data. Reads combine shared unpublished edits with each handle's remote
snapshot. The daemon now supplies bounded Rust session RPCs and one shared per-drive feed.
Guarded publication reconciles exact acknowledgements and retains conflicting versions locally,
and the core recovers from process kills at each step. The Swift bridge and writable FSKit or
SMB callbacks remain later work.

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
policies on 5 October. Local writes and guarded publication are implemented below; conflict
resolution UI remains a later deliverable.

| Decision | Accepted policy |
|---|---|
| Names | New writes store NFC names, with equivalent NFD lookups, using the approved `unicode-normalization` dependency. Existing remote names stay byte-exact. If multiple names normalize alike, lookup reports ambiguity rather than choosing one. Names remain case-sensitive. |
| Conflicts | Preserve the local saved data and remote version locally until the user resolves the conflict. Do not automatically publish a conflict sibling visible to other clients. |
| Write acknowledgement | `write` returns after staging bytes that survive a daemon or extension crash. `fsync`, `F_FULLFSYNC` and `close` flush bytes and metadata to disk. Cloud publication has separate status. |
| Files that stay open | **Accepted 6 October: publish after two seconds without writes**, as well as on fsync and close, so an application does not defer cloud publication merely by keeping a file open. |

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

**Complete, 8 October:** every bullet and the done-when are met; the evidence is in
[capabilities](#capabilities-8-october) and [setting mode and mtime](#setting-mode-and-mtime-8-october).

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
  mount table now shares one watcher per stable drive; avoid extra watchers in the Swift bridge.
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
team before the sandboxed bridge and adapter, and the compatibility rig in parallel. (The probe
passed on 9 October: [signed-bundle probe](#signed-bundle-probe-9-october).)
Only after snapshot/coherence and restart behavior are demonstrated should writable callbacks
land. Pieces, `/Volumes`, UI and distribution follow the same core; each remains a separate
reviewable deliverable.

Slice 3 is split into local namespace changes and staged file data (both implemented below):
`write`, `truncate`, `fsync`, durable close, merged reads, disk admission and staged-file leases.
Item 2's Rust session RPCs and shared per-drive feed are now implemented below. Publication
reconciliation and retained conflict states are now implemented as slice 4, and kill-point
recovery as slice 5. The user selected
a two-second quiet period for publishing files that stay open, alongside fsync and close.

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
deletions; a new open observes refreshed metadata. Read-only sessions reject write opens with `EROFS`, directories return
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

### Local namespace changes, 6 October

`Session::new_writable` receives the daemon's existing `Queue` and requires the same `Store`.
It adds exclusive empty-file `create`, `mkdir`, `unlink`, empty-directory `rmdir`, and `rename`
with replacement or exclusivity. Exchange returns `ENOTSUP`. This slice covers namespace
mutations; staged file data is described below. The existing constructor keeps a read-only namespace.
Each acknowledged edit changes the overlay, inode metadata and guarded journal entries in one
SQLite `FULL` transaction. The queue installs that committed work before waking its publisher.
New names are NFC, existing remote source names stay byte-exact, and ambiguous equivalent
names fail. Directory generations advance on edits; rename keeps the source inode. Complete
offline snapshots and locally created directories admit mutations; unknown remote parents fail
offline. `rmdir` checks the merged remote/local view and the queue orders child deletions first.

Migration 6 stores each inode's latest journal entry and remote base path, complete binary xattr
maps, and a mount-specific journal policy. `getxattr`, `listxattr`, `setxattr` (set/create/replace)
and `removexattr` retain empty values, enforce the 64 KiB combined raw name/value limit, and use
`ENOATTR` on macOS (`ENODATA` elsewhere) for missing attributes. Remote maps are read at the
bound version and accepted only if inode identity, generation and path still match. Dirty maps
and namespace overlays survive feed invalidation and restart. A paused local rename retains its
remote base path for cold reads and directory/xattr refreshes, including unseen descendants.
Pending folder refreshes verify identity and version before and after listing, so a replacement
at the old path cannot contribute children to the locally renamed inode.
Feed events also route through indexed pending remote origins, including subtree events for
items moved out of that subtree locally. Events at the old path conservatively invalidate the
pending local subtree and its current ancestors even when a tombstone hides the old name.

New names publish under an absence guard; edits use a retained version or the published version
of an earlier journal entry. Mount entries never retry `412` without their guard. A failed guard
retains the pending overlay and blocks dependent work; queue status exposes the failure. The
CLI queue retains its existing policy. Referenced lineage survives clearing finished entries.
Mount queue dependencies also compare NFC spellings, so removing a decomposed remote name
finishes before publishing a replacement at its NFC spelling.
Folder listings that omit a version require an identity-checked attributes lookup to bind one
before mutation; a folder whose attributes supply no version returns `ENOTSUP` rather than
publish without a guard.

Rename replacement is atomic locally. The current wire rename guards only its source, so remote
replacement queues a guarded destination delete followed by an exclusive guarded source rename.
A changed destination prevents its deletion; a destination created between the two requests
prevents the rename. This is two remote commits: if the delete succeeds and the source guard then
fails, the remote destination stays deleted with its history retained, while the local replacement
remains pending. Folder replacement requires an empty destination. Full publish reconciliation
(binding a new object's identity/version, clearing the overlay, and `saving`/`saved`/`conflict`
states) remains slice 4; successful uploads in this slice keep the local inode pending.

Validation covers paused uploads and resumed publication, NFC/raw remote names, native errors,
merged emptiness and child-first deletion, rename identity and replacement, cold open/unlinked
snapshots through both fetchers, restart, concurrent creates, transaction rollback, xattr
snapshots and generation races, and competing remote guards. Each new test must fail with its
behavior broken; race/churn breaks run three times. No step 5 item is complete.

Namespace-slice validation: 547 workspace tests pass (6 ignored), workspace clippy passes with warnings
denied, and spec/credential-script checks plus memory, fs and versitygw interoperability pass.
The 45 new tests were each seen to fail under targeted defects: 94 isolated failure runs also
exercise the existing queue dependency regression, with race/restart breaks repeated three times.
The namespace suite passes three final consecutive runs. Local boto3 checks skip because it is
unavailable; CI requires them and also covers MinIO and Docker Compose.

### Staged file data, 6 October

Writable sessions now implement `open(..., true)`, `write`, `truncate`, `fsync` and durable
close. `Session::new_writable` uses the existing shared `Store`, `Cache` and `Queue` with default
`StagingConfig`; `Session::new_writable_with_config` accepts a different free-space reserve,
an injectable volume free-space check and an optional quiet period. The defaults keep 256 MiB
free and queue changes after two seconds without writes. A caller can disable the timer with
`quiet_period: None`; fsync and writable close still queue changes. Read-only sessions continue
to reject write opens with `EROFS`; a write through a read-only handle returns `EBADF`.

A writable session exclusively owns its drive within the Store. A second writer or a separate
read-only alias returns `EAGAIN` while that owner lives; an existing read-only session also blocks
a new writer. Read-only sessions can still coexist with each other. Adapters must share the
writable Session to observe one local byte view. Its ownership lease remains with any owned
flush task until that task finishes, so cancellation cannot admit a second stale staging map.

Migration 7 persists a staged record per inode: the remote base captured at first write, its
path, logical size, remote visibility cutoff, local extents, dirty ranges and flush revisions.
Base timestamps use RFC3339, including dates before 1970.
One append-only byte file per inode implements the sparse staging view. Writes append their
bytes before a SQLite `NORMAL` transaction records the extent changes and new inode attributes.
Overwrites split or replace logical ranges without changing earlier physical bytes, so a
partial write or failed metadata commit preserves every prior acknowledgement. Adjacent ranges
merge when they are also adjacent in the byte file. Sparse growth stores no bytes for holes.
Truncation clips local ranges and lowers the visible remote cutoff; subsequent growth returns
zeros for that discarded region instead of reviving old bytes.

The shared local view is visible immediately through earlier handles, lookup, getattr and
directory listings. A handle keeps its own immutable remote reader for untouched gaps; local
ranges override it, and gaps beyond the visible remote base return zeros. Cached remote bytes
remain readable offline, while an uncached remote gap fails promptly. All remote gaps share
one relocation request and time budget in a read callback. Local creates and writes work offline
under complete cached parents or locally created directories. Reopening the state
restores acknowledged names, staged bytes and the remote base even if the previous session
never closed or fsynced its handle.

`write` acknowledges process-crash durability after the byte write and the `NORMAL` commit;
it does not flush the file for each write. Fsync and writable close synchronize staged bytes
and their directory and parent state directory, then commit metadata and frozen publication
entries using the original SQLite `FULL` connection with `fullfsync` enabled. The accepted
`F_FULLFSYNC` adapter callback will use this same core
flush path. Local disk completion and remote cloud completion remain separate states.
Cancellation cannot abandon the owned flush task between journal commit and its runtime state
update. Staging admission is serialized across inodes and checks the configured reserve plus
incoming physical bytes and metadata room before recording a mutation. `ENOSPC` or transaction
failure acknowledges no new bytes. An unused appended tail is retained after a metadata error
because its commit outcome can be uncertain after a late disk error.

Publication receives immutable files owned by the journal, preserving them across restart and
separating them from later appends to the live staged file. Contiguous new-file data uses a
frozen full put; existing files and sparse new-file edits use bounded write ranges, a truncation
when needed and the final logical size. Untouched remote gaps are not downloaded to publish an
edit. Patching preserves existing content type, flags, metadata and xattrs; a new-file full put
carries its complete local xattr map. Queue coalescing bounds both the encoded patch body and
the logical size of an in-memory put, including sparse holes. The guard comes from the writable handle's captured version or retained journal lineage,
so a newer remote version cannot become an accidental overwrite base. Mount guard failures keep
the local staged bytes and block dependent entries. Rename preserves the inode and flushes to
its current linked name. An open-unlinked or replaced handle can still read and write its own
bytes; its later flush does not publish another object at the old name. Atomic save through a
temporary file and rename-over retains the replaced object's bytes for its earlier handles.

Dirty bytes mark an inode `pending`; durable queue handoff marks it `saving`. Successful upload
still leaves `saving` until slice 4 binds published identities and versions, clears overlays and
reconciles saved/conflict/error states. Mutable staged logs remain retained so open handles and
pending work keep their bytes. Compaction of obsolete append ranges and removal of physical
orphans are not implemented yet. Full kill-point recovery across publication and cleanup also
remains slice 5. No step 5 item is complete.

Validation covers overlapping writes, sparse gaps, shrink/regrow, earlier-handle reads,
offline cache misses, restart before fsync, remote guard conflicts, atomic rename-over,
open-unlink, frozen queue snapshots, failed byte/metadata writes, low-space admission, close and
cancellation races, and quiet-period publication, including delayed timer rechecks after a
successful flush or a newer write following a failed attempt.
The ignored staging benchmark measures 1 GiB in 1 MiB writes and 10,000 files of 4 KiB, with
successful `NORMAL` staging and `FULL` flush commits counted by `Session::staging_commits`.
On the local Mac debug build, with uploads paused, the quiet timer disabled and an injectable
free-space check allowing admission, the measurements are:

| Workload | Total time | Mean write acknowledgement | p50 / p99 / maximum | NORMAL / FULL staging commits |
| --- | --- | --- | --- | --- |
| 1 GiB in 1 MiB writes | 0.572 s | 558 microseconds | 168 / 3,139 / 133,320 microseconds | 1,024 / 0 |
| 10,000 files of 4 KiB | 247.879 s | 539 microseconds | 342 / 3,672 / 9,767 microseconds | 10,000 / 10,000 |

The first total covers write acknowledgements without a final fsync. The second covers each
create, open, write and durable close, including another 10,000 FULL namespace-create commits;
its latency columns measure the write calls alone. Close latency was not separately measured.
These are local staging measurements, not cloud publication throughput. Reproduce with
`cargo test -p voidfs-client --locked --test mount_writes staged_write_throughput_one_gib_and_ten_thousand_small_files -- --ignored --exact --nocapture`.

Staged-data validation: 587 workspace tests pass (7 ignored), workspace clippy passes with
warnings denied, spec validation passes 55 cases / 420 steps, and the five credential-script
tests pass. Memory, fs and versitygw interoperability pass with both addressing styles and
the existing admin, aws-chunked, curl and rclone checks. Local boto3 checks skip because it is
unavailable; CI requires them and also covers MinIO and Docker Compose. Each of the 40 new
regular tests and the ignored benchmark was seen to fail under targeted runtime defects:
81 isolated failure runs, with restart, race and timing defects repeated three times. The
four cancellation, ownership and timer tests pass three consecutive restored runs.

### Rust daemon sessions, 7 October

`voidfs-daemon` now owns a weak registry of filesystem cores keyed by stable drive ID. Each
live drive has one writable `mount::Session`, the existing daemon Store/Cache/Queue, and one
`FeedWatch`, shared by all its mounts and logical RPC sessions. The registry resolves display
aliases online on each attach so alias reuse cannot attach a consumer to the wrong core. An
atomic, synchronized `drive-aliases.json` memo supports known identities offline; it is bounded
at 4,096 mappings and 1 MiB, evicts individual old identities under pressure, and removes stale
aliases when a rename, alias reuse or deletion is learned. Stable-ID records remove contradictory
legacy aliases on load. Migration 8 saves a stable ID beside each remembered mount's display
name; restoration resolves that ID and can use it offline even after memo eviction. A remembered
record without a stable ID requires an explicit remount, including a provisional offline mount
after restart: a current alias memo cannot prove its original target. Reusing a live core refreshes
its display alias before a new mount is saved. Unknown offline identities can bootstrap a read-only mount, but RPC
session creation returns `Offline` before creating a core, changing epochs or writing namespace
rows until identity is resolved. That provisional core stays read-only after reconnect; reopen after connection to get
a pinned writable core. If the provisional name was already an ID, its read-only mount must
release the same-drive lease before a writable reopen. Existing known-ID sessions stay pinned
when the display alias changes. CLI upload queue addressing retains its existing policy.

`Core.session` gives adapters the shared local byte view. Adapters enforce `MountSpec.read_only`
at their boundary; RPC consumers enforce their own policy even though they share the writable
core. The feed first commits `Session::invalidate`, then notifies every mounted observer and
socket watcher. Metadata generations increase within that core; remote sequence positions do
not decrease. The shared core emits `LocalChange` after committed local writes, truncation,
namespace/xattr edits and changed flush status, whether called through an adapter or RPC. Owned
mutation tasks preserve delivery if callers cancel. These updates target object keys and inode
IDs, including open-unlinked inodes, and reach every mount and socket observer while offline.
Adapters implement `Mounted::local_change` to invalidate inode attributes as well as linked keys;
its default forwards linked invalidations. Attribute-only edits do not request a drive resync or
advance the independent namespace generation used by enumeration. Each watch starts with a full resync, including resubscription. Remote reconnects (even idle ones),
expired history, broadcast lag and oversized events produce `All` instead of pretending that
incremental metadata is complete. Existing `ChangeWatch::next` keeps its batch-only contract;
`next_event` exposes stream connections for consumers needing resync.

The user-only Unix socket now has `/v1/fs` beside the existing controls. Kernel Unix peer
credentials authenticate the daemon's effective UID and a nonzero PID before dispatch. Each
filesystem session belongs to its creating UID/PID, including watches and release; fresh and
pooled connections from that process work, and another process is refused even if it knows the
ID. Session IDs contain 256 bits from the OS random source, with collision rejection. Controls
remain available to processes of the daemon's user. Signed app identity and protection against
same-user debugging or memory access belong to the later Mac bridge security work. The typed Rust API is
`DaemonClient::session(drive, read_only) -> FsClient`, with `FsClientError` retaining status,
code, message and native errno. The local filesystem wire version is 1 and is independent of
the object protocol and format.

| Method and path | Request / response |
| --- | --- |
| `POST /v1/fs/sessions` | `{version, drive, readOnly}` → `{version, id, drive, root, generation, metadataGeneration, readOnly, maxIo, maxEntries, capabilities}` ([capabilities](#capabilities-8-october), added 8 October within version 1) |
| `POST /v1/fs/{id}/lookup` | `{parent, name}` → `Attr` |
| `POST …/getattr`, `…/readlink` | `{ino}` → `Attr` or `{target}` |
| `POST …/readdir` | `{ino, after, limit}` → `{entries: [[name, Attr], …], generation}` |
| `POST …/open` | `{ino, write}` → `{fh, attr}` |
| `POST …/handle_attr` | `{fh}` → `Attr` |
| `GET …/read?fh=&offset=&length=` | raw `application/octet-stream` bytes |
| `PUT …/write?fh=&offset=` | raw `application/octet-stream` bytes → `{written}` |
| `POST …/truncate`, `…/fsync`, `…/close` | `{fh, size}` for truncate, otherwise `{fh}` → `{}` |
| `POST …/release` | `{}` → `{}`; closes all of that consumer's handles |
| `GET …/watch` | NDJSON `{generation, seq, resync, invalidations, inodes}` |
| `POST …/create`, `…/mkdir` | `{parent, name, mode}` → `Attr` ([9 October](#the-rest-of-the-session-calls-9-october), as are the rows below) |
| `POST …/unlink`, `…/rmdir` | `{parent, name}` → `{}` |
| `POST …/rename` | `{fromParent, fromName, toParent, toName, how}`, `how` one of `replace`, `exclusive`, `swap` → `{}` |
| `POST …/link`, `…/clone_file` | `{ino, parent, name}` → always refused: `ENOTSUP`, or `EROFS` for a read-only consumer |
| `POST …/setattr` | `{ino, mode?, mtime?}`, `mtime` RFC 3339 → `Attr` |
| `GET …/getxattr?ino=&name=` | raw `application/octet-stream` value |
| `PUT …/setxattr?ino=&name=&how=` | raw value (at most 64 KiB), `how` one of `set`, `create`, `replace` → `{}` |
| `POST …/listxattr`, `…/removexattr` | `{ino}` → `{names}`; `{ino, name}` → `{}` |
| `POST …/conflict` | `{ino}` → `{conflict}`: the core's `Conflict`, or `null` |
| `GET …/read_conflict?ino=&side=&offset=&length=` | raw bytes of the `local` or `remote` retained version |

Every session route requires `x-voidfs-generation` from creation. `SessionInfo.generation` is
the persisted core epoch; `metadataGeneration` and watch generations order all drive-wide
notifications within that core. Page generations count namespace changes and remote metadata
invalidation independently, so a data/attribute edit does not force `readdir` to retry. `Attr.generation` is the inode-local metadata
generation. Session IDs are opaque; handle IDs retain the core's persisted epoch/counter. Old sessions/generations/handles return `ESTALE`
after restart. A different logical consumer's handle returns `EBADF`, and a read-only consumer's
write/truncate/open-for-write, and since 9 October every other mutation, returns `EROFS`. Errors are `{error: {code, message, errno}}` with
a non-success status. Backend error codes/messages are clipped at UTF-8 boundaries to 256/4,096
bytes, keeping error frames within the response limit. JSON wrappers use camelCase; nested `Attr` keeps its existing Rust serde
shape (`object_id`, `version_id`, `has_xattrs`, RFC3339 `mtime`, and `Saved`/`Pending`/`Saving`/
`Conflict`/`Error` sync values). Invalidations are `{kind: "object", key}`, `{kind: "subtree", key}`
or `{kind: "all"}`. An enumeration cursor is exclusive UTF-8 name order; clients restart it
when its namespace generation changes; a page racing a namespace change or remote invalidation
returns `EAGAIN`. Local attribute-only edits preserve page generations.

Admission bounds are 32 active drives, 256 logical sessions, 1,024 handles per consumer, 32
in-flight calls, 64 watches, 8 MiB binary I/O, 256 directory entries, 64 KiB request JSON and
1 MiB response JSON. A 64-event broadcast ring never grows for a slow consumer. Events collapse
to full resync above 64 KiB, 1,024 invalidations or 4,096 inode IDs. Admission happens before
reading a request body and remains held through owned core work and response transmission;
long-lived watches use their separate limit. Controls retain their existing admission path.
The Rust client bounds successful/error bodies and NDJSON buffering, checks negotiated limits
and rejects malformed framing or regressing event generations. Ordinary calls time out after
30 seconds; an established watch may remain idle indefinitely.

Clients must explicitly `release`; socket disconnect or dropping a Rust client does not release
its logical session. A lost creation response can leave a session until daemon shutdown, within
the session cap; a lost open response retains its handle until release, within the handle cap.
Accepted core work runs in an owned task so cancellation cannot skip staging
or post-write notification. Release waits for accepted calls, closes handles and ends watches;
if a close fails, failed handles and their session remain available for retry. The last mounted
or RPC consumer drops its shared core/feed; daemon stop drains handles and ends observers before
closing the queue/store. Stopping cancels queued creation and read-only network bootstrap
requests, while any owned filesystem/DB work finishes. This slice exposes staged file-data
helpers, but namespace/xattr RPCs,
the Swift XPC service, app-container migration and FSKit callbacks remain separate work.
No step 5 item is complete. The signed-bundle probe remains a user prerequisite for the Mac
bridge/adapter; guarded publication reconciliation and full recovery remain core slices 4–5.

On the local Mac debug build, 1,000 warmed calls through the real Unix socket measured:

| Call | Mean | p50 | p99 | Maximum |
| --- | --- | --- | --- | --- |
| Warm `getattr` | 296.447 µs | 290.958 µs | 376.709 µs | 4.109 ms |
| Cache-hit 4 KiB read | 109.982 µs | 107.416 µs | 153.459 µs | 194.791 µs |

Reproduce with `cargo test -p voidfs-daemon --locked --test sessions measure_warm_metadata_and_cached_read_socket_hops -- --ignored --exact --nocapture`.
These measure the socket and Rust core, excluding Swift/XPC, FSKit and cold cloud reads. The
spike comparison remains part of the Mac bridge validation; these numbers impose no performance
gate and justify no direct-file optimization yet.

Validation for this slice: `cargo test --workspace --locked` passed 612 tests with 8 ignored,
and `cargo clippy --workspace --all-targets --locked -- -D warnings` passed. The 25 new regular
tests and one opt-in benchmark received runtime failure proofs (58 isolated failing runs in
total, with timing/restart cases repeated three times), then passed with the implementation
restored. The conformance validator passed 55 cases/420 steps; credential scripts passed 5
tests. Local interoperability passed against memory, filesystem and versitygw backends with
both addressing styles. Local boto3 checks were skipped because it is unavailable; CI covers
boto3, MinIO and Compose interoperability.

Daemon follow-up validation: the six identity, local coherence, bounded-state and session
ownership fixes pass 628 workspace tests (8 ignored), workspace clippy with warnings denied,
the 55-case/420-step spec validator and all five credential-script checks. The 15 new regular
tests and two strengthened existing tests received 47 isolated runtime failure proofs, with
restart, cancellation and race cases repeated three times, and passed after restoration. Local
interoperability passes on memory, filesystem and versitygw in both addressing styles; boto3
remains unavailable locally. Legacy name-only remembered records require explicit remounts.
Guarded publication and conflict reconciliation are implemented below; full kill-point recovery
is the next core slice. The signed-bundle probe still precedes the Swift bridge and FSKit adapter.

Accounts, web, search, previews, video review, Linux/Windows mounts, server locking, retention and
encryption remain in their later steps. Cloud benchmark runs and new provider credentials do not
block local mount/core development. Passing step 4 with storage-credential cases skipped is not
proof of the direct bucket path; step 5 must exercise both direct reads when supported and the
already-built server fallback.


### Guarded publication and conflict reconciliation, 7 October

Slice 4 reconciles the existing mount journal after publication. The core reads HEAD and full
attributes at the **acknowledged version**, validates object identity/version/kind against the
local inode, and commits journal completion with durable inode state in one SQLite transaction.
Clean saves adopt remote identity, size, mtime, mode and the full xattr map; clear only namespace overlay cells owned by
those entries; and transition to `saved`. A remote listing that observes a newly published object
before its acknowledgement is folded back onto the original local inode. Folder moves rebase
cached descendant remote paths, including children with later pending edits.

Writes, truncation, namespace changes and xattrs accepted during an upload keep their newer local
state. Publication holds staged-writer leases while capturing/reconciling, applies in-memory
changes only after the transaction commits, and emits inode metadata notifications afterwards.
`Queue::settle` includes this final reconciliation. Clean staging detaches from fresh opens;
existing handles retain the immutable local layers they saw. A subsequent write through such a
handle uses its latest own acknowledged version as a guard, while an untouched earlier handle
keeps the original snapshot guard after an external refresh. A stale writable handle cannot
join a live stage created from a newer external version: its write returns `ESTALE` before
changing bytes or metadata. Restart reconstructs the stage guard from its original base or a
proven own publication, rather than an unrelated namespace refresh. Shrink/regrow still clips
prior layers so discarded bytes cannot return.

Guard rejection (`412`), rename destination collision (`409`) and a missing source (`404`) produce
`conflict`, while other permanent errors produce `error`. Competing file byte snapshots are
written and synchronized under the private state directory before the conflict transaction;
folder snapshots retain metadata without allocating a byte file;
local attributes/xattrs and exact remote attributes/xattrs are persisted with them. `Session::conflict`
returns the retained metadata and `read_conflict` supplies bounded reads (at most 8 MiB) of each
file side without exposing filesystem paths. A blocked child resolves to the directly rejected
owner (`Conflict.ino`), preserving its own regular local reads; folder conflict reads return
`EISDIR`. The owner dependency survives later moves, cancellation and restart. A deleted remote
object is an explicit absent side, including `412` without a current version when HEAD confirms
absence. An unversioned delete response is accepted only after confirming
the source is absent; a folder that acquired children stays in conflict. Local snapshots of
a remotely moved base use the existing namespace-first, 16-request/2-second relocation bound.
Later local edits retain the conflict state and its first snapshots. Queue resume, cancel,
clear-finished and restart cannot remove the predecessor's guard or publish a conflict sibling.
The CLI publisher retains its existing overwrite/conflict-copy policy.

Snapshot capture can fail, for example when the network or local disk becomes unavailable.
The conflict records that failure and snapshot availability explicitly, retaining fetched
remote metadata even when its content capture fails; original staged/journal
sources remain retained and the mutation stays blocked. Conflict selection/resolution UI and
snapshot cleanup are later work. Mutable stage compaction, orphan collection and complete
process kill-point recovery are slice 5; this slice does not complete step 5 item 1.


Acknowledged data PUTs that still need xattrs persist that phase before the attribute request.
Retries finish only those xattrs, then reconcile the final exact version. A lost attribute reply
is accepted only when pinned before/after identity, ETag, size and complete expected attributes
match; a competing edit still conflicts. Upgrading prior mount state replays completed saves
with known versions through metadata reconciliation without repeating the mutation, and recovers
overlay ownership from persisted inode/name/journal evidence. Unprovable legacy overlays remain
local; broader interrupted-state repair and cleanup belong to slice 5.

A mount PUT with an ambiguous reply and no durable acknowledgement fails closed as a conflict,
even when a current version inherits its journal marker. Marker-only matching cannot distinguish
a later foreign edit. Exact recovery of that pre-acknowledgement window remains slice 5; the
existing CLI marker policy is unchanged.

Malformed publication metadata and permanent reconciliation failures produce visible `error`
without repeating acknowledged data; transient database failures retain the completion retry.

Guarded-publication validation: 664 workspace tests pass (8 ignored), and workspace clippy
passes with warnings denied. The 36 new tests and one strengthened existing test each received
a targeted runtime failure proof: 101 isolated failing runs, with race, restart and cancellation
defects repeated three times. All restored tests pass. Spec validation passes 55 cases / 420
steps, and the five credential-script tests pass. Local memory, fs and versitygw interoperability
passes conformance, stock S3 checks, aws-chunked and rclone in both addressing styles. Local
boto3 is unavailable; CI supplies boto3 and checks MinIO and Docker Compose as well.

Slice 5, recovery, follows. The signed-bundle probe still precedes the Swift bridge and FSKit
adapter.

### Recovery, 8 October

Slice 5 completes item 1's core slices. What a process kill, a lost reply or a cleanup leaves
behind is now recovered or removed, and a seeded model test checks the session against an
in-memory filesystem across restarts. No protocol, format or server behavior changes.

**Restart.** Opening a drive's writer queues, in the background, every stage the previous
session left with unflushed bytes: no handle of that session remains to fsync or close them.
Before, only the quiet-period timer did, so with the timer disabled acknowledged bytes stayed
local for good. `Session::recovered` waits for the hand-off, which flushes eight files at once
and stops with its session; the next writer starts it again. The writer opens without waiting:

| Unflushed files after a kill | Writer open | All queued |
| --- | --- | --- |
| 1,000 of 4 KiB | 0.09 s | 11.3 s |
| 100 of 1 MiB | 0.01–0.02 s | 1.3 s |

Each flush still syncs its stage, freezes a copy and commits at `FULL`, so the hand-off of many
small files is bounded by those syncs; batching them is possible later work.

**A lost publication reply.** A mount put the server committed, but whose reply was lost to a
timeout or a kill before the client recorded it, is now recognized exactly instead of becoming a
conflict. The version right after the put's guard in the object's full history
(`x-voidfs-all`), or the object's first version under an absence guard, must carry the put's
`<state id>.<entry id>` marker. Later edits can inherit the marker, but the first version with it
can only be the put's own, and a guarded put lands right after its guard. A recognized put is
acknowledged and reconciled without being sent again; anything else stays a conflict, as before.
Folder creation uses the same check. Patches, renames and deletes carry no marker, so a lost reply
to one of them still fails closed as a conflict. Recognizing those exactly needs a marker on
those requests, which is a protocol addition for the user to decide. ([Lost replies](#lost-replies-to-edits-renames-and-attribute-changes-8-october)
adds it, recognizes renames and attribute changes without one, and finds deletes were already
accepted.)

**Cleanup.**

- A stage whose record is gone is retired: its file goes when the last handle still reading it
  closes. Before, every saved file left its whole staging file behind, with nothing to remove it.
- An unlinked file's open handles keep its live stage, even after its removal publishes, and the
  last close discards it. A refresh no longer drops names with unpublished changes (see the
  review fixes below), so only a local unlink or replacement leaves a staged inode without a name.
- Opening a drive's writer removes stages of inodes no name reaches, unless a conflict holds them,
  then that drive's staging files no record names: a kill between creating, compacting or
  publishing a stage and recording it. Interrupted flush assemblies are now named
  `<inode>-assembly-…` so that they are attributed to a drive; legacy `snapshot-…` files go too.
- Opening the queue removes snapshots in `mount-conflicts` that no conflict records, and a run
  whose completion fails removes the snapshots it captured. Before, each retry captured both
  sides again, and an existing conflict row ignored the new copies.

**Compaction.** Staging files are append-only, so overwrites accumulate. A flush rewrites the
file once its unused bytes exceed both `StagingConfig::compact_garbage` (64 MiB by default) and
the bytes still in use. Live extents are copied in logical order to a new file, which is synced
with its directory and recorded at `FULL` before the old file is removed. A kill leaves one of
the two unrecorded, which the next writer removes. Compaction is skipped when the copy wouldn't
fit the free-space reserve. Retired layers that earlier handles hold are separate files and are
never rewritten. After a flush, a staging file is thus at most its live bytes plus the larger of
those bytes and the threshold; between flushes it grows with the writes. A 64 MiB file rewritten
three times in 1 MiB writes, fsynced after each pass:

| Compaction | Fsync after each pass | Staging file after the last |
| --- | --- | --- |
| Off | 0.06–0.09 s each | 192 MiB |
| 64 MiB threshold | 0.06–0.08 s, then 0.11–0.13 s for the pass that compacts | 64 MiB |

**Lost staged bytes.** A power failure can lose bytes a write acknowledged before any fsync,
while a later `FULL` commit makes their record durable. Before, one such stage stopped the whole
drive from opening. Now that file is marked `error`, opening it fails with `EIO` instead of
serving other bytes, and the rest of the drive works. Stage paths resolve by name in the current
state directory, so a state directory reached by another spelling still loads.

**Kill points.** Test builds have named points where a scenario running in a child process (the
unit-test binary, re-run) sends itself `SIGKILL` at a chosen hit; the test then reopens the state
directory and checks it. Other builds compile the points to nothing. Each point is covered:

| Point | After a kill there |
| --- | --- |
| `stage.appended` | The unrecorded write is absent, earlier writes whole; the restart publishes them. |
| `stage.recorded` | The recorded write is whole. |
| `flush.frozen` | Queue open removes the unreferenced copy; the writer queues the stage again. |
| `flush.committed` | The queued entries publish once. |
| `publish.sent` | Both the create's absence-guarded put and the content's version-guarded put are recognized; two versions in all. |
| `publish.acknowledged` | Reconciliation resumes without a second put. |
| `reconcile.committed` | With a published file still held open: the retired stage and the source are removed on reopen. |
| `compact.written`, `compact.committed` | Exactly the recorded copy remains, with its bytes. |
| `conflict.captured` | Unrecorded snapshots go; the conflict is recorded once with one copy per side. |

**Model.** `tests/mount_model.rs` runs seeded `proptest` sequences of create, mkdir, writes
(including through handles whose file was unlinked or replaced), truncate, rename with
replacement, unlink, rmdir, fsync, close, restart without closing, upload with handles open and
publish, over four names two folders deep. After every step the session's listings, inode
identities, sizes and bytes, and every open handle's bytes, must equal the model's. After each
publish the drive must equal the model, every name must be saved, and no staging, journal or
conflict file may remain. The default is 16 cases from a fixed seed, about 6 s;
`VOIDFS_MODEL_SEED` and `VOIDFS_MODEL_CASES` explore further: 3,600 cases over seeds 12 to 20
passed on the final code. The wider runs found three bugs in this slice: bytes left unflushed by
a killed session were never queued without the quiet timer, the stage of a file unlinked while
open was never removed, and the open-unlinked handle bug below. The default run doesn't reach
the last one, which needs a narrow sequence of steps; its own regression test guards it.

**Review fixes** (a separate commit). Reviewing PRs 41 to 46 found:

- The daemon refused an oversized socket body without reading it, so a client still sending got a
  broken pipe instead of the typed 413. That race failed main's CI. It now reads up to twice the
  limit before refusing.
- A flush whose journal commit failed left its frozen copies until the queue next opened.
- A directory refresh dropped every name the listing no longer had. If another Mac deleted a file
  with unpublished local edits, or its folder, the file vanished locally, and its flush found no
  path: the edits were neither published nor kept as a conflict. Names whose inode, or something
  under it, has unpublished changes now stay, a vanished folder holding them lists as empty, and
  the guarded publication becomes a `404` conflict that keeps the local bytes. A kept name hides
  another object listed under it until its own change resolves.
- A handle on a file unlinked while open lost its earlier bytes once the removal published: the
  stage retired as a clean save, and the handle's next write started from the empty create's
  version. Such a stage now stays live while a handle has it open.
- PR 44 included a change to `spec/conformance/cases.json` (ce4c2da, the direct-upload token
  case's edit), which the handoff rules reserve for the user's approval. It keeps the case's
  intent, with a test proving it over 4,096 nonces; it is only flagged here.

Item 1 is not marked complete: advertising unsupported hard links, cloning and cross-machine locks
to adapters remains (done in [capabilities](#capabilities-8-october), which found one more gap).
Conflict resolution and snapshot cleanup remain UI work.

Recovery validation: 690 workspace tests pass (9 ignored), and workspace clippy passes with
warnings denied. The 26 new tests, the four review-fix tests among them, were each seen to fail
with their code broken: 40 isolated failing runs, each alone under a timeout, with the lost-reply
breaks repeated three times. All restored tests pass. Spec validation passes 55 cases / 420
steps, and the five credential-script tests pass. Local memory, fs and versitygw interoperability
passes conformance, stock S3 checks, aws-chunked and rclone in both addressing styles; boto3 is
unavailable locally, and CI supplies it and checks MinIO and Docker Compose. Reproduce the
measurements with
`cargo test -p voidfs-client --locked --test mount_recovery recovery_and_compaction_measurements -- --ignored --nocapture`.

### Capabilities, 8 October

Item 1 asks that unsupported hard links, exchange, cloning and cross-machine locks be advertised
and tested explicitly. `Session::capabilities()` now returns what an adapter advertises, for
FSKit's volume capabilities and `pathconf` answers, and the daemon's session reply carries it.
No protocol, format or server behavior changes.

| Capability | Value | What the core does |
| --- | --- | --- |
| `hardLinks` | no | `Session::link` is refused; every file has one name |
| `exchange` | no | `RenameMode::Swap` (`RENAME_SWAP`, `exchangedata`) is refused, both files untouched |
| `exclusiveRename` | yes | `RenameMode::Exclusive` (`RENAME_EXCL`) fails with `EEXIST` |
| `clone` | no | `Session::clone_file` (`clonefile`) is refused |
| `locks` | `local` | The core has no lock calls. FSKit's kernel grants `flock` and `fcntl` locks without reaching the module ([spike §4.5](spikes/fskit.md#45-what-must-a-writable-mount-additionally-handle)), so they hold on this Mac only; no other machine sees them. Cross-machine locking (D7) needs its own design |
| `caseSensitive` | yes | `OTHER` doesn't find `other`, and both can exist |
| `nfcNames` | yes | New names are stored NFC; any equivalent spelling finds a name unless two normalize alike (`EILSEQ`); remote names stay byte-exact |
| `persistentIds` | yes | Inode numbers survive restarts, renames and new versions |
| `xattrs`, `maxXattrBytes` | yes, 65,536 | An object's xattr names and values together |
| `maxNameBytes` | 255 | |
| `maxPathBytes` | 1,024 | A file's path from the drive root, or a folder's with its trailing slash |

**Refusals use `ENOTSUP`.** `FsError::Unsupported` mapped to `EOPNOTSUPP`, which on macOS is 102,
"operation not supported on socket". macOS documents `ENOTSUP` (45) for a filesystem that doesn't
support `link`, `clonefile`, a `renamex_np` flag or `exchangedata`, and SpaceFS's mount returns it
for all four ([head-to-head](../bench/results/mac-head-to-head/README.md#semanticsc-observed)).
Apps fall back on that value, so `Unsupported` now maps to `ENOTSUP` (a separate commit). Linux
has one value for both, so nothing changes there. A folder whose attributes supply no version
also returns `ENOTSUP` now.

**Decisions.**

- The capabilities describe the drive's core, not one consumer: a read-only RPC session gets the
  same set, and `SessionInfo.readOnly` already says it can't write. A read-only core refuses
  `link` and `clone_file` with `EROFS`, as it does `rename`.
- `link` and `clone_file` exist only to refuse, so an adapter forwards every callback through one
  error mapping, and supporting either later changes no adapter.
- The local wire version stays 1. `capabilities` is an added response field: earlier readers
  ignore it, since `SessionInfo` doesn't deny unknown fields. `FsClient` requires it rather than
  guess, so a daemon built before it is refused at session creation. Version 1 has no consumer
  outside this workspace yet, so nothing in use breaks; the Swift bridge should require it too.
- The RPC doesn't carry rename, namespace or xattr calls yet, so `Swap` isn't reachable through
  it; an unknown call such as `link` returns `404 NoSuchOperation` with `ENOTSUP`.
- Symbolic-link creation, sparse files and volume sizes aren't advertised yet. The adapter items
  add what FSKit needs as further fields, which older readers ignore.

**Item 1 against its bullets.** Item 1 was not yet complete here: the core couldn't set the mode
or the modification time of an existing file or folder. [Setting mode and mtime](#setting-mode-and-mtime-8-october)
adds that, and item 1 is complete.

| Item 1 asks for | Where it is |
| --- | --- |
| Stable inodes, a persisted overlay, explicit directories, NFC names with equivalent lookups, case-sensitive collisions, byte-exact remote names, ambiguity rejected, handle identity kept through rename and open-unlink | [Namespace](#namespace-slice-5-october), [local namespace changes](#local-namespace-changes-6-october); `tests/mount.rs`, `mount_mutations.rs`, `mount_writes.rs` (`open_unlink_keeps_local_reads_and_writes_without_publishing_after_close`) |
| Open, close, lookup, enumeration, getattr, read, write, truncate, rename-over, remove, mkdir and xattrs with one error mapping | `Session` and `FsError::errno`; `mount_reads.rs`, `mount_writes.rs`, `mount_mutations.rs`, `mount_xattrs.rs` |
| Setting attributes (`chmod`, `utimes`, `setattrlist`) | Not met by this slice: `create` and `mkdir` took a mode and writes set the mtime, but no call changed either afterwards. Met by [setting mode and mtime](#setting-mode-and-mtime-8-october): `Session::setattr` and `setattr_*` in `mount_mutations.rs` |
| Unsupported hard links, exchange, cloning and cross-machine locks advertised and tested | This section; `advertised_capabilities_match_what_the_core_does`, `a_read_only_core_advertises_the_same_and_refuses_links_as_read_only`, `capabilities_reach_the_socket_with_unsupported_operations_marked` |
| A read-only open binds identity, version, size and attributes and reads that version through the cache; a writable open adds its local generation | [Open handles](#open-handles-and-snapshot-reads-5-october), [staged file data](#staged-file-data-6-october) |
| Every acknowledged mutation durable with its journal; data kept for open handles; the overlay rebuilt after restart | [Staged file data](#staged-file-data-6-october), [recovery](#recovery-8-october); `local_overlay_and_queue_survive_a_restart_and_remote_refresh` |
| Writes admitted within the disk reserve, `ENOSPC` without acknowledging, pending/saving/conflict/error states | `reserve_admission_returns_enospc_before_bytes_or_metadata_change`; `Sync` |
| Queue guards and the `412` policy; both versions kept on a conflict; no later unguarded overwrite | [Guarded publication](#guarded-publication-and-conflict-reconciliation-7-october); `mount_publication.rs` |
| Done when: save, rename, open-unlink and crash recovery against a temporary store; reads agree with the overlay while paused; kill points over staged bytes, metadata, publish and cleanup, with a published entry held open | [Recovery](#recovery-8-october)'s kill-point table and model test; `killed_after_reconciling_a_publication_held_open_leaves_no_orphans` |

Capabilities validation: 693 workspace tests pass (9 ignored), and workspace clippy passes with
warnings denied. The three new tests and the two changed ones (the errno mapping and the typed
client's refusals) were each seen to fail with their code broken: eight breaks, 12 isolated
failing runs. Spec validation passes 55 cases / 420 steps, and the five credential-script tests
pass. Local memory, fs and versitygw interoperability passes conformance, stock S3 checks,
aws-chunked and rclone; boto3 is unavailable locally, and CI supplies it and checks MinIO and
Docker Compose.

### Setting mode and mtime, 8 October

`Session::setattr(ino, mode, mtime)` sets the permission bits and modification time of a file,
folder or symbolic link: `chmod`, `utimes` and `setattrlist`, as `touch -t`, `cp -p`, `tar` and a
Finder copy that keeps its date use them. It was item 1's last missing operation. Size stays with
`truncate` on a handle. No protocol, format or server behavior changes: the queue's `attrs`
entries already published mode and mtime under a guard (protocol §4.8).

- The change and its guarded `attrs` entry commit in one transaction, as an xattr change does.
  It works offline under a complete listing and survives a restart. A version another Mac made
  after the local base turns it into a conflict that keeps the local values.
- `None` leaves a value unchanged, and only what changes is sent: a `chmod` leaves the mtime, as
  POSIX does. A value the inode already has queues nothing, so setting the date a file already
  shows makes no version.
- Times keep microseconds, as the server does, so a published time reads back equal. Years 0 to
  9999 are accepted, those before 1970 included; others have no RFC 3339 form and return
  `EINVAL`, as do modes above `07777`. Nothing is applied in part.
- The drive root is refused with `ENOTSUP`, as its xattrs are: it has no object to carry them.
  A read-only core refuses with `EROFS`, and an unlinked inode with `ESTALE`.
- A write sets the mtime to now and keeps the mode. A later `setattr` wins until the next write,
  and the flush publishes the inode's current mode and mtime, so a copy that sets its date before
  closing publishes that date. Handles sharing the file's live stage see the change through
  `handle_attr`, which now takes the mode from local state along with the size and mtime; other
  handles keep their snapshot's attributes, as they do for remote changes. The change is recorded
  while the stage is locked, so a cancelled caller can't leave the stage's view behind its record.
- The daemon's socket doesn't carry it yet: namespace, xattr and attribute RPCs are item 2's.

With this, every item 1 bullet in the [table above](#capabilities-8-october) and its done-when are
met, and **item 1 is complete**: step 5's first completed item.

Validation for setting mode and mtime: 697 workspace tests pass (9 ignored), and workspace clippy
passes with warnings denied. The four new tests and the read-only test's new assertion were each
seen to fail with their code broken: 12 breaks, 13 isolated failing runs. They cover a missing
journal entry, a missing no-op check, unrounded and wrongly converted pre-1970 times, the year,
mode and root checks, the stage view and `handle_attr`, an unguarded publish, a read-only core,
and a write that drops the mode. Spec validation passes 55 cases / 420 steps, and the five
credential-script tests pass. Local memory, fs and versitygw interoperability passes; boto3 is
unavailable locally, and CI supplies it and checks MinIO and Docker Compose.

### Lost replies to edits, renames and attribute changes, 8 October

When the reply to a guarded mount publication is lost (a timeout, a dropped connection, a kill),
the retry finds the guard already moved, by the request's own effect. Since slice 5 a put was
recognized by its marker; the other mount operations were false conflicts: nothing was lost, but
the user would have had to resolve a conflict between a save and itself. SpaceFS doesn't
recognize its own writes either: its documentation says a retried guarded write gets `412` and
leaves that to the caller, and its mount appears to publish unguarded, so a resent edit can
overwrite another writer's save (docs.spacefs.com, read 8 October; not observed in the app). The
mount now keeps its guards and recognizes its own outcome:

| Operation | A retry after it landed gets | Now known by |
| --- | --- | --- |
| put, folder | `412` | its marker on the version right after its guard (slice 5) |
| write, truncate (a patch) | `412` | the same: edits carry the marker now ([RFC 0005](../rfcs/0005-user-metadata-on-edits.md)) |
| rename | `404`, or `412` if a new object took the old key | the inode's object at the destination, and a `rename` right after the guard in its history |
| attributes, xattrs, mode, mtime | `412` | an `attrs` version right after the guard whose attributes are the guard's with this change, and the same content |
| delete | `204` without a version | already accepted once a `HEAD` confirms the key is absent (slice 4); the recovery notes above were wrong to list it |

A rename changes no content, and an attribute change none either, so another writer's identical
change right after the guard has the same outcome and is taken as this one's. Anything else
after the guard stays a conflict: a rename to another destination, other attributes, another
writer's edit, or a version that inherited an earlier marker of this client's.

**Markers on edits (RFC 0005, protocol draft 1, revision 9).** A write, a patch and a splice now
accept `x-amz-meta-*` (protocol §4.1–§4.3): each header sets that user-metadata entry on the new
version, and the others keep their values, as all of them did before. The queue sends
`voidfs-entry: <state id>.<entry id>` on every write and patch, as on puts, and a mount edit
whose guard fails looks for it on the version right after its guard. An edit's version thus
carries `voidfs-entry` in its user metadata, as a put's always has, and keeps every other entry. An older server ignores
the header, so the edit's version keeps the marker of the version before, which names another
entry: the lost reply stays a conflict, as before, and nothing is misrecognized. The user
approved the spec change on 8 October; the conformance case is `user-metadata-on-edits`.

**A fix to slice 5: an entry whose reply was lost goes alone.** A retried run could carry more
entries than the attempt that landed, because writes queued while the reply was outstanding
coalesced into it, and recognizing the head's marker then took them as published unsent. A new
file's create whose reply was lost, then a sparse edit before the retry, left the remote file
empty and dropped the local bytes. Now an entry that may have landed is retried on its own.
Entries that followed it in the attempt that landed are sent again on top of the recognized
version, which changes nothing: a run of writes and truncates applied again to its own result
leaves it as it was, at the cost of one more version in this rare case. A transient failure,
such as a timeout, now marks the entry as possibly landed, as a stop or a restart did, and the
mark lasts until an attempt finishes the entry rather than until the next one starts.

The CLI queue shares that mark, so after a timeout its put checks the current version's marker
before sending the bytes again, a rename that then finds its source gone counts as done (as it
already did after a restart), and a run that timed out is retried alone. Its `412` policy is
unchanged.

**Kill points.** `patch.sent`, `rename.sent`, `attrs.sent` and `delete.sent` follow the request,
as `publish.sent` does in a put. After a kill at each, the restart publishes once and the
history shows the one version the operation made:

| Point | After a kill there |
| --- | --- |
| `patch.sent` | The retried patch is known by its marker: `put`, `put`, `write`, then the mtime's `attrs` |
| `rename.sent` | `put`, `put`, `rename`, at the new name |
| `attrs.sent` | `put`, `put`, `attrs`, with the new mode |
| `delete.sent` | Absent, with nothing left queued |

**Limits.** The rename check reads the inode's object id from the state store, so a rename whose
inode has no remote identity yet stays a conflict. Attributes compare exactly as the server
spells them; a server spelling the same time or mode differently would get a conflict. A rename
that landed and was then moved again elsewhere, before the retry, is a conflict: the object is
not where this rename put it.

Validation for lost replies: the 11 new tests and the conformance case were each seen to fail
with their code broken: 16 breaks, 34 isolated failing runs, the lost-reply breaks three times
each. They cover the run that grew after a lost reply, the possibly-landed mark on a timeout and
its lasting past an inconclusive attempt, rename, attribute and patch recognition and each kill
point, accepting any `404`, any attributes or any marker, the SDK leaving the header out, a
server ignoring it (the conformance case fails against it, and a client facing it gets the
conflict), the core's merge, and a delete's `404` turned into a conflict. The restored recovery
and crash tests pass three consecutive runs. 708 workspace tests pass (9 ignored), and workspace
clippy passes with warnings denied; one existing test now also expects the edit's marker in the
user metadata it otherwise preserves. Spec validation passes 56 cases / 432 steps, and the five
credential-script tests pass. Local memory, fs and versitygw interoperability passes, the new
case included in both addressing styles; boto3 is unavailable locally, and CI supplies it and
checks MinIO and Docker Compose.

### Signed-bundle probe, 9 October

The FSKit spike ran with development signing only. This probe shows voidfs's own team, HAUTK68F56,
can ship the module the way SpaceFS does, and that the sandboxed extension, signed for
distribution, reaches a helper the app registers: the path item 2's bridge needs. It used the
spike's app, a local memory-store server and one test drive, on macOS 27.0.1 with Xcode 27 beta
(27A5194q). No bucket, account or SpaceFS session was involved.

| Step | Result |
| --- | --- |
| Profile | A Developer ID provisioning profile, "voidfs FSKit Developer ID", for `dev.voidfs.app.fskit` with FSKit Module, made in the developer portal by the user: all devices, expires 2044. The app needs none: its App Group carries the team prefix |
| Build | `apps/macos/scripts/release.sh` archives, then exports with `scripts/ExportOptions-DeveloperID.plist`: both bundles signed "Developer ID Application: … (HAUTK68F56)" with the hardened runtime; the extension embeds the profile and has `com.apple.developer.fskit.fsmodule` |
| Notarization | Accepted with no issues (submission `48919b2d-00a1-4ebe-a642-cf5d4f3795bf`), so the beta toolchain is no obstacle. Stapled; `stapler validate` passes |
| Gatekeeper | `spctl --assess`: accepted, "Notarized Developer ID"; `syspolicy_check distribution`: ready for distribution. Installed in `/Applications` with a quarantine flag, the first launch showed the downloaded-app prompt, checked the notarization online, and ran once the user chose Open |
| The module | FSKit found it at `/Applications/voidfs.app/Contents/Extensions/VoidfsFS.appex`, off until the user switched it on once. `mount -F -t voidfs` mounted the drive and read its file; AMFI logged nothing about it. The running extension verifies as the Developer ID build with the profile |
| The helper | The app's own binary, as a launch agent in `Contents/Library/LaunchAgents/dev.voidfs.agent.plist`, registered with `SMAppService` (`voidfs agent-register`): enabled at once, without an approval step; launchd holds it with the Mach service `HAUTK68F56.dev.voidfs.agent` and a team launch constraint, and starts it on demand from `/Applications` |
| Extension to helper | 500 XPC round trips of 4 KiB from the sandboxed extension: p50 59 µs, p90 80 µs, p99 150 µs (the spike, development-signed: 62 µs). The unprefixed `dev.voidfs.agent` fails from the sandbox, as in the spike |

**Translocation.** Copied into `/Applications` by a script rather than moved in Finder, the
quarantined app ran from a randomized `AppTranslocation` path, and so did its extension; the
helper, started by launchd, ran from `/Applications`. A downloaded disk image that the user drags
into Applications is moved in Finder and avoids this. Item 7's installer must keep it that way.

**Not covered.** Mounting in `/Volumes` still needs `com.apple.developer.fskit.mount`, which
neither profile grants; item 7's privileged helper stays the plan, as SpaceFS does it. The probe
used the spike's read-only module and its echo, not the daemon's session RPCs, which item 2's
bridge will forward. Contributors still need their own team.

Reproduce with a Developer ID provisioning profile of that name and a notarytool keychain
profile from `xcrun notarytool store-credentials`:
`apps/macos/scripts/release.sh voidfs-notary`, then the steps in `apps/macos/README.md`.

### The rest of the session calls, 9 October

The daemon's `/v1/fs` socket now carries every mount-core call an adapter needs: `create`,
`mkdir`, `unlink`, `rmdir`, `rename` with its mode, the `link` and `clone_file` refusals,
`setattr`, the four xattr calls, and the conflict reads. `FsClient` has a typed method for each,
and the [wire table](#rust-daemon-sessions-7-october) lists them. No protocol, format or server
behavior changes: this is the local API.

**Decisions.**

- **Binary values travel raw.** Xattr values and conflict reads use `GET` or `PUT` with a query
  and a raw `application/octet-stream` body, as `read` and `write` do; JSON stays on `POST`.
  Base64 in JSON would have put a 64 KiB value (87 KiB encoded) over the 64 KiB JSON bound.
  Names are percent-encoded in the query (any UTF-8 but NUL) and decoded strictly: an unknown,
  repeated or missing field is `EINVAL`.
- **Limits.** A `setxattr` body over 64 KiB is refused before the core with `E2BIG` (413
  `TooLarge`), the answer the core gives when an object's names and values together exceed
  64 KiB; the client won't send one. A conflict read is at most `maxIo` (8 MiB), as a `read` is.
  Names keep the core's checks (`EINVAL` for `/`, NUL, or more than 255 bytes).
- **Read-only consumers get `EROFS` for every mutation**, at the socket and before the core: the
  drive's core is shared and writable, so the refusal is the boundary's. That includes `link` and
  `clone_file`, which a writable consumer gets `ENOTSUP` for, as it does a `swap` rename; both
  names stay as they were. Reading xattrs and conflicts is allowed.
- **A conflict keeps its Rust shape.** `conflict` returns the core's `Conflict` as serde writes
  it (snake_case; `local_xattrs` as byte arrays), as nested `Attr`s do, and `read_conflict` reads
  the retained bytes. The Swift bridge won't forward either: resolving a conflict is the app's
  (item 8), not the extension's.
- **The wire version stays 1.** The calls are additive. A daemon from before answers them
  `404 NoSuchOperation` with `ENOTSUP`, as it still answers any unknown call; nothing outside
  this workspace speaks version 1 yet, and the app will carry its own daemon (item 7).
- **Times.** `setattr`'s `mtime` is RFC 3339, as `Attr` spells it, so times before 1970 and
  microseconds round trip. One beyond what RFC 3339 can say is refused by the client before
  sending, and one the daemon can't parse is `EINVAL`.

**Tests** (`crates/voidfs-daemon/tests`), through the real socket unless said otherwise:

| Test | Shows |
| --- | --- |
| `namespace_calls_cross_the_socket_into_the_journal_and_notify_other_sessions` | `mkdir`, `create` (NFD stored NFC), `rename` (inode kept, rename-over, `EEXIST` for exclusive, `ENOTSUP` for swap with both names kept), `unlink`, `rmdir` (`ENOTEMPTY`, `ENOTDIR`, `EISDIR`, `ENOENT`, `EINVAL`), another session's watch and page generation, and the journal publishing it all once resumed |
| `attributes_and_binary_xattrs_cross_the_socket` | `setattr` (a pre-1970 time with microseconds, a `chmod` keeping the time, `EINVAL`, `ENOTSUP` at the root), binary and empty xattr values under a name needing escapes, `EEXIST`/`ENOATTR` for create/replace, 64 KiB at the limit and `E2BIG` over it, a strict query, and publication |
| `read_only_consumers_are_refused_every_mutation_and_links_are_unsupported` | `EROFS` for each of the eleven mutations from a read-only consumer, `ENOTSUP` for link, clone and swap from a writable one, and nothing reaching the namespace or the journal |
| `conflicts_and_their_retained_versions_cross_the_socket` | A guarded save refused by another writer's version: `conflict` and both retained versions read from a read-only consumer, ranges, and refusals |
| `sessions_have_random_ids_and_are_owned_by_the_connecting_process` (`session_security.rs`) | Another process can't use a known session for `create`, `rename`, `setattr`, `setxattr`, `getxattr` or `conflict` |
| `hostile_xattr_conflict_and_time_answers_are_bounded` (`fs_client.rs`) | The client's bounds against a fake daemon: an xattr over 64 KiB, a conflict read longer than asked, a malformed conflict, values and times it won't send |
| `capabilities_reach_the_socket_with_unsupported_operations_marked` (changed) | `link` through the socket: `ENOTSUP`, or `EROFS` when read-only; an unknown call is still `NoSuchOperation` |

Validation: 713 workspace tests pass (9 ignored), and workspace clippy passes with warnings
denied. The five new tests and the two changed ones were each seen to fail with their code
broken: 20 breaks, each run alone under a timeout. They cover `create` dispatched as `mkdir`, a
rename ignoring its mode, `unlink` and `rmdir` swapped, the read-only check dropped from
`create`, `link`, `setxattr` and `removexattr`, `setattr` dropping the time, names sent
unescaped, an oversized value answered with `EINVAL`, times before 1970 losing a second,
conflicts answered as none, conflict reads unbounded at the socket, a client accepting an
oversized xattr or a longer conflict read, sending any value or clamping an unrepresentable
time, and the owner check skipped for the xattr routes or `create`. Spec validation passes 56
cases / 432 steps, and the five credential-script tests pass. Local memory, fs and versitygw
interoperability passes; boto3 is unavailable locally, and CI supplies it and checks MinIO and
Docker Compose.
