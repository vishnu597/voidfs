# Spike: a native FSKit mount on macOS

*2026-09-27. MacBook Pro, Apple M5 Pro, 48 GB; macOS 27.0 (26A428); Xcode 27.0 beta (27A5194q,
SDK 26A5353p). Server: `voidfs-server` release build, `fs:` store, on the same machine.*

This spike built the first native macOS mount of a voidfs drive and used it to answer the open
questions in [§5.3 of the plan](../RESEARCH_AND_PLAN.md#53-the-macos-mount-decision-the-riskiest-platform-choice).
Everything below was measured or observed on the machine above unless it says otherwise.

## 1. Summary

- **It works.** A sandboxed FSKit extension inside a SwiftUI menu-bar app mounts a drive
  read-only with `mount -F -t voidfs voidfs://host:port/drive <dir>`, as the logged-in user, with
  no root, no kext and no macFUSE. Finder, `ls`, `cat`, `mmap`, xattrs and Unicode names all
  behave.
- **It is fast enough.** Sequential reads of a 1 GiB file ran at **2.3–2.5 GB/s**, faster than a
  single-stream client talking to the same server, because the kernel keeps about 15 1 MiB reads
  in flight. Listing 1,000 files took **21–33 ms** cold and **6–8 ms** warm. A random 4 KiB read
  took **1.5–1.9 ms** at the median, of which FSKit itself adds about 0.2–0.5 ms.
- **The Rust client core should run in a per-user launchd agent**, not in the extension. There is
  one extension process per mounted volume, it exits on unmount, and when it crashes the volume is
  force-unmounted and not relaunched. The sandbox allows XPC to an agent whose service name starts
  with the App Group (62 µs round trip).
- **Target macOS 27, not 26** (decided the same day; SpaceFS supports 26.4). A network drive needs
  `FSVolume.setCacheState` to show other machines' changes, and that API, like the new handler
  protocols, is macOS 27 only.
- **Signing needs a provisioning profile.** `com.apple.developer.fskit.fsmodule` is a restricted
  entitlement; without a profile, AMFI kills the extension at launch. Xcode's automatic signing
  issued a team profile that grants it. `com.apple.developer.fskit.mount`, needed for
  `FSClient.mountSingleVolume` (mounting in `/Volumes`), is also restricted and was **not**
  granted, so the host app falls back to `mount(8)`.
- **FSKit's coherence tools are coarse.** Only `revoke` makes the kernel forget cached
  attributes and missing names. `invalidate` drops pages but keeps the old size, which produces a
  torn read. A descriptor opened before a change keeps the old size and reads new bytes cut at
  it. Phase 2 should serve reads from the version current at open (snapshot-at-open).
- **A writable mount has sharp edges**, found on Apple's own FSKit volume (FAT): `renamex_np`
  with `RENAME_SWAP` reports success but overwrites the destination and loses its data
  (unconfirmed since 1 October: `semantics.c` printed both files from one buffer, which makes a
  correct swap look like this; see §4.5); locks are local only; Finder and `ls -l` look up an AppleDouble `._name` for every file; macOS sets
  `com.apple.provenance` on nearly every new file.

## 2. What was built

```
apps/macos/
  voidfs.xcodeproj    Xcode 16+ project format (folder-synchronized groups); three targets
  App/                voidfs.app: SwiftUI menu-bar host; also headless commands (status, save, mount, settings)
  FSModule/           VoidfsFS.appex: the FSKit module (FSUnaryFileSystem + FSVolume handlers)
  Shared/             SigV4 signer, HTTP client, credential store, XPC echo (spike only)
  Probe/              voidfs-probe: the same HTTP calls without FSKit, for baselines
  Config/             Info.plist of the extension, entitlements
  scripts/            seed.py, bench.sh, randread.c, bulkstat.c, semantics.c, xpc-echo-agent.swift
```

- **The extension** conforms to the macOS 27 protocols `FSVolume.Handler`, `ReadWriteHandler`,
  `XattrHandler` and `DataCacheHandler`:
  - `lookup`, `readdir` and `getattr` are answered from one `?x-voidfs-list` per folder (§4.9),
    cached for 5 s or until the change feed marks the listing stale.
  - `read` is one ranged GET per kernel read.
  - xattrs come from `?x-voidfs-attrs` (§4.8). They are fetched only for files whose listing entry
    says `hasXattrs`; for every other file, `getxattr` answers `ENOATTR` without a request.
  - A long-polled change feed (§5.6) revokes what other machines change.
  - Every write operation answers `EROFS`.
  - File ids are derived from the object id, so inode numbers are stable across mounts.
- **Credentials** are stored by the host app in the App Group container
  (`~/Library/Group Containers/HAUTK68F56.dev.voidfs/mounts.json`, mode 0600) and read by the
  extension. Never put them in the mount URL: `mount` and `df` print the URL to every user.
- **Signing** uses Xcode automatic signing for team HAUTK68F56 (Apple Development certificate
  and a Mac Team Provisioning Profile).

### How to run it

Prerequisites: Xcode 27 signed in to a team whose profile can carry the FSKit Module capability,
and a running server:

```bash
cargo run --release -p voidfs-server -- --store fs:/tmp/voidfs-spike --listen 127.0.0.1:9000 \
  --access-key-id VFSPIKEKEY0000000000 --secret-access-key spikesecretspikesecretspikesecretspike1
```

```bash
VOIDFS_ENDPOINT=http://127.0.0.1:9000 VOIDFS_ACCESS_KEY_ID=VFSPIKEKEY0000000000 VOIDFS_SECRET_ACCESS_KEY=spikesecretspikesecretspikesecretspike1 python3 apps/macos/scripts/seed.py
```

```bash
xcodebuild -project apps/macos/voidfs.xcodeproj -scheme voidfs -configuration Release -derivedDataPath apps/macos/build -allowProvisioningUpdates -allowProvisioningDeviceRegistration build
```

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs settings
```

Turn **voidfs** on in System Settings → General → Login Items & Extensions → File System
Extensions (the command above opens that pane). Then store a key and mount:

```bash
echo spikesecretspikesecretspikesecretspike1 | apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs save voidfs://127.0.0.1:9000/spike VFSPIKEKEY0000000000
```

```bash
mkdir -p /tmp/voidfs-mnt && mount -F -t voidfs voidfs://127.0.0.1:9000/spike /tmp/voidfs-mnt
```

```bash
apps/macos/scripts/bench.sh
```

Mount options go after `-o`: `ttl=<ms>` (listing cache), `nofeed`, `iosize=<bytes>`. Logs:
`log stream --predicate 'subsystem == "dev.voidfs"'`. The module also answers a few
**spike-only** names looked up in any folder, which always return `ENOENT`:
`.voidfs-stats` and `.voidfs-stats-reset` log call and request counts; `.voidfs-bench` times
reads from inside the extension; `.voidfs-cache:<action>:<mode>:<coherency>:<key>` calls
`setCacheState`; `.voidfs-xpc` pings the echo agent. Remove them before Phase 2 ships.

## 3. Measurements

All numbers are over loopback against a local server, so they measure the mount's own overhead,
not a WAN. "Cold" means a fresh mount (nothing cached in the kernel or the module); every cold run
remounted first.

### 3.1 Listing

| Folder | Call | Cold | Warm | HTTP requests |
|---|---|---|---|---|
| 1,000 files | `getattrlistbulk` (what Finder uses) | **21–33 ms** (10 runs; two runs inside `bench.sh` took 60 and 163 ms) | 6–8 ms | 1 listing |
| 1,000 files | `ls -f` | 21–40 ms | 6 ms | 1 listing |
| 1,000 files | `ls -l` | 189 ms | 69 ms | 1 listing |
| 10,000 files | `getattrlistbulk` | 103–184 ms | 19 ms | 10 listing pages |
| 10,000 files | `ls -l` | 1.7 s | 0.65 s | 10 listing pages |
| Direct HTTP, no FSKit | `voidfs-probe list` | 1,000: 4.5–11 ms; 10,000: 36–44 ms | | |

- Most enumerations, including the ones behind `ls -f`, arrived **with attributes** wanted, so one
  listing request per folder page answers `ls`, `stat` and Finder alike.
- `ls -l` is slow for a different reason: it makes about four upcalls per file (a lookup, a
  missed lookup of `._name`, two `listxattr`), about 17 µs each. None of them causes an HTTP
  request.
- A file three folders deep takes **7.4 ms** to its first byte on a cold mount: 4 listings and 1
  read.
- Mount takes 77–82 ms, including one describe request (§5.4); unmount takes 10 ms.

### 3.2 Sequential read (video-scrubbing proxy)

| 1 GiB file, cold | Throughput (as `dd` reports it, 10⁹ bytes) | Kernel reads |
|---|---|---|
| `dd bs=1m` | **2.29–2.33 GB/s** (0.46 s) | 1,024 × 1 MiB, ~15 in flight |
| `dd bs=8m` | 2.32–2.52 GB/s | the same |
| `shasum -a 256` of the whole file | 2.1 s; hash matches the seeded file | the same |
| Warm (kernel page cache) | 21.6 GB/s | none |
| Direct HTTP, 1 MiB × 1 in flight | 0.95–1.08 GB/s | |
| Direct HTTP, 1 MiB × 4 in flight | 1.41 GB/s | |
| Direct HTTP, 8 MiB × 4 in flight | 2.06 GB/s | |

- The kernel caps each read at **1 MiB**. Setting `f_iosize` to 256 KiB, 4 MiB or 16 MiB
  changed nothing. Throughput comes from the kernel's read-ahead keeping many reads in flight.
  Over a WAN, bigger requests would have to come from the module's own prefetch.
- The SHA-256 of the file matched the seeded file both through `voidfs-probe` and through the
  mount.

### 3.3 Small random reads

| 4 KiB at random offsets of 1 GiB, 300 reads | p50 | p90 | p99 |
|---|---|---|---|
| Mount, `F_NOCACHE` (every read reaches the module) | **1.5–1.9 ms** | 3.1–4.0 ms | 4.5–5.6 ms |
| Mount, page cache on (the kernel reads 16 KiB pages) | 1.3–1.5 ms | 2.9–3.1 ms | 4.4–4.8 ms |
| Direct HTTP from an ordinary process | 0.4–0.6 ms | 1.8–2.2 ms | 2.8–3.2 ms |
| The same HTTP call made inside the extension | 1.3–1.4 ms | 3.2–3.3 ms | |

- The FSKit call path adds about 0.2–0.5 ms. The rest of the gap is the extension process:
  the same `URLSession` request costs 0.8 ms more there, at any task priority. Clamping an
  ordinary process to background QoS reproduces most of it (1.1 ms), but the cause was not fully
  isolated.
- Against a real bucket 20–80 ms away this overhead does not matter. A local chunk cache would
  avoid it.
- `mmap` of the 1 GiB file, touching one byte per MiB: 1.15 s, 1,024 page-ins of 16–512 KiB.

### 3.4 Server unreachable

| | Connection refused (server down) | Hung (server stopped with `SIGSTOP`) |
|---|---|---|
| Cached listing or cached data | Served: 5–7 ms | Served: 5–7 ms |
| Anything else | `EHOSTDOWN` ("Host is down") in 8–21 ms | `ETIMEDOUT` after 16 s **per request**: `stat` 32 s, `read` 48 s, `ls` of an unlisted folder 96 s |
| New mount | Fails in 78 ms: "Host is down" | |
| After the server returns | Same mount works, no remount; the change feed reconnects (backoff 1 → 30 s) | The same |

The hung case is the real problem: a Finder window over a stalled connection would hang for
minutes. Phase 2 needs a connectivity state that fails fast after the first timeout (or serves
from cache) and probes in the background, with short connect timeouts separate from transfer
timeouts.

### 3.5 Other machines' changes (change feed)

With the final revoke strategy (§5), over loopback:

| Change made through S3 | Visible on the mount |
|---|---|
| Overwrite (12 → 32 bytes) | 3–60 ms, content and size |
| New file, in `ls` | 2–3 ms |
| New file, by `stat` of a name looked up (and missed) before it existed | 2–3 ms; **not while another file in that folder is held open** |
| Delete | 2–57 ms |

## 4. Answers to §5.3's open questions

### 4.1 Where should the Rust client core run?

**In a per-user launchd agent, reached over XPC.** The extension should stay a thin front end:
it holds an in-memory copy of the metadata it has served, and it reads cached data straight from
files in the App Group container.

The evidence:

| Observation | Consequence |
|---|---|
| One extension process per mounted volume (two drives mounted → two `VoidfsFS` processes, 20–24 MB each) | An in-process core runs once per mount: several caches, several upload queues, several change-feed connections, coordinated only through files |
| The process exits when its volume is unmounted | Nothing in it can drain an upload queue after eject, pre-fetch, or maintain the cache |
| When the process dies, fskitd logs "Instance died" and force-unmounts the volume; nothing relaunches it | A crash in an in-process core loses every open file and any upload in memory. An agent survives, and the next mount picks up its journal |
| From the extension, XPC reached the agent under the App-Group-prefixed name `HAUTK68F56.dev.voidfs.agent` but not under `dev.voidfs.agent`, which worked from an ordinary process | The agent must register an App-Group-prefixed mach service |
| XPC round trip, 4 KiB: 27–29 µs from an ordinary process, **62 µs p50 / 153 µs p99 from the extension** | Per-operation cost of the agent hop. An FSKit upcall costs at most about 17 µs, so forwarding every upcall of a warm `ls -l` to the agent would add about 250 ms per 1,000 files (69 ms today). Hence the metadata memo in the extension |
| The extension reads its App Group container (it loads the access keys from there) | Chunk files the agent writes there can be read by the extension directly, with no XPC for cached data |

Shape for Phase 2:

- **Agent (Swift shell around the Rust core):** chunk cache and metadata database in the App
  Group container, write journal, upload queue, one change-feed connection per drive, and
  credentials. It is registered as a login item by the app (`SMAppService`).
- **Extension:** metadata memo, validated by feed generations that the agent pushes; data reads
  from cached chunk files, or over XPC on a miss; writes appended to the journal through the
  agent; and `setCacheState` calls the agent asks for.
- **CLI:** uses the Rust core directly for S3 work, and talks to the agent for mount state.

### 4.2 What does the extension's sandbox allow?

- It runs **as the logged-in user** (uid 501), launched by the per-user `fskit_agent`, with
  `HOME` in `~/Library/Containers/dev.voidfs.app.fskit/Data`.
- **Outgoing network** works with `com.apple.security.network.client`: plain HTTP to 127.0.0.1
  worked. Apple's own FTP module also has `network.server`; voidfs does not need it.
- **Files:** its own container, and the App Group container, which it read. Writing there is
  granted by the App Group entitlement but was not exercised. Nothing else was needed or tested.
- **Mach lookups:** a service named with the App Group prefix worked; the same service under an
  unprefixed name did not.
- **Keychain:** the team profile grants `keychain-access-groups: HAUTK68F56.*`, so a shared
  Keychain item can replace the spike's JSON credential file. Not tested.

### 4.3 Signing, the entitlement and user enablement

| Experiment | Result |
|---|---|
| Xcode, Developer ID certificate, no profile | Refuses to build: "requires a provisioning profile with the FSKit Module feature" |
| Extension signed by hand with `fskit.fsmodule`, Developer ID, no profile, then executed | Killed at exec by AMFI: "No matching profile found", "restricted entitlements" |
| Host app signed with `fskit.mount`, no profile | Killed at exec the same way |
| Extension signed without `fskit.fsmodule` | Discovered by FSKit and listed; never enabled, so whether it would run is untested |
| Xcode automatic signing (`-allowProvisioningUpdates -allowProvisioningDeviceRegistration`) | Created an Apple Development certificate, registered the Mac, and issued "Mac Team Provisioning Profile: dev.voidfs.app.fskit" granting `fskit.fsmodule`. Works |
| `FSClient.mountSingleVolume` from the host app | fskitd: "Client is missing 'com.apple.developer.fskit.mount' entitlement" (`EPERM`). The team profile Xcode issued does not include it |
| `mount -F -t voidfs <url> <dir>` as the user | Works; `mount` is an entitled platform binary |

- **Enablement.** A newly installed module is off. `mount` then fails with "Module
  dev.voidfs.app.fskit is disabled!". The user turns it on once in System Settings → General →
  Login Items & Extensions → File System Extensions. On macOS 27, `FSClient.shared.
  openFileSystemExtensionsSettings()` opens that pane, and `FSClient.shared.installedExtensions`
  reports `isEnabled`, so the app can guide the user.
- **Discovery.** FSKit found the module wherever the app was registered with LaunchServices
  (even in a build folder); no copy to `/Applications` was needed.
- **Distribution:** Developer ID signing needs a Developer ID provisioning profile that carries
  the FSKit Module capability, plus notarization. (Tried with voidfs's team on 9 October: it
  works, notarized, with a sandboxed extension reaching an `SMAppService` agent; see
  [step 5's signed-bundle probe](../step-5-macos.md#signed-bundle-probe-9-october).) SpaceFS
  ships exactly that, notarized ([PARITY.md §5](../PARITY.md#5-what-spacefss-mac-app-is-made-of)).
  SpaceFS also shows that `/Volumes` needs no `fskit.mount` entitlement: a privileged
  LaunchDaemon helper mounts there.
- **Contributors** cannot build a runnable module without their own team, because the
  entitlement is profile-gated. Document this in `CONTRIBUTING.md` in Phase 2.

### 4.4 Which resource type and mount flow fit a network drive?

- **`FSGenericURLResource`**, with `FSSupportsGenericURLResources = true` and
  `FSSupportedSchemes = [voidfs]` in `EXAppExtensionAttributes`. This is exactly how Apple's own
  FTP client is built (`com.apple.fskit.ftp`, macOS 27). `FSPathURLResource` is for file-backed
  volumes, `FSBlockDeviceResource` for disks.
- **Flow:** `mount -F -t voidfs voidfs://host:port/drive <dir>`, as the user, into any folder the
  user owns. FSKit skipped `probeResource` and called `loadResource`, where the module
  authenticates and fails fast. Then come `activate`, which receives the `-o` options one at a
  time as `["-o", "ttl=0", "-o", "nofeed"]`, and `mount`, which receives none.
- `FSUnaryFileSystem` means one resource, one volume and one process per drive.
- `mountSingleVolume` would mount in `/Volumes` but needs the restricted `fskit.mount`
  entitlement (§4.3).
- `mount` and `df` print the resource URL (`voidfs://127.0.0.1:9000/spike on /private/tmp/…
  (voidfs, nodev, nosuid, read-only, noowners, noatime, fskit)`), so the URL must never carry a
  secret. FSKit sets `noowners`: files show as the mounting user.

### 4.5 What must a writable mount additionally handle?

Observed on the voidfs mount (read-only) and on Apple's FSKit FAT module (writable), with
`scripts/semantics.c`:

| Behaviour | Evidence | Phase 2 must |
|---|---|---|
| **xattrs and AppleDouble** | Finder opening a 1,000-file folder asked about 470 `getxattr`s (FinderInfo, ResourceFork, TextEncoding, quarantine) and looked up `._name` for every file; `ls -l` did the same. FAT, which has no native xattrs, got `._a.txt` files | Implement `XattrHandler` for every name (so the kernel never falls back to AppleDouble); answer `._*` lookups from cache; never store `._` files. Put xattr names in listings (RFC below) |
| **`com.apple.provenance`** | Set on almost every file created from this machine, even by `echo hi > f` | Fold xattrs set right after create into the file's creating version, or each new file costs two commits |
| **Atomic-save rename over an existing file** | Works on FSKit (`renameItem` with `overItem`) | Implement as one rename version with the replaced object's history kept |
| **`RENAME_SWAP`** | On FSKit FAT: **returns success and overwrites**. `b`'s data was lost; confirmed after a remount. APFS swaps correctly. FSKit's rename callback has no flags. *1 October: `semantics.c` printed `a=` and `b=` from one buffer, so a correct swap printed as "a=A b=A" too (seen on APFS). The probe is fixed; FAT was not tested again, so this finding is unconfirmed ([head-to-head](../../bench/results/mac-head-to-head/README.md#semanticsc-observed))* | Advertise `RENAME_SWAP = no` (FSKit does) and test the voidfs module the same way. File a Feedback with Apple |
| `RENAME_EXCL` | Correct on FSKit (`EEXIST`, no change) | Nothing extra |
| `exchangedata`, `clonefile` | `ENOTSUP`, as advertised | Nothing; apps fall back |
| **`flock` and `fcntl` locks** | Succeed on the voidfs mount and never reach the module. FSKit has no lock callbacks | Locks are per-Mac. Cross-machine locking (D7) needs its own mechanism, for example lock objects on the server that the host app shows |
| **mmap** | Shared writable `mmap` + `msync` reached disk on FAT; read `mmap` works on voidfs | Serve page-outs as ordinary writes; flush on `synchronize` |
| `F_FULLFSYNC` | Succeeds on FSKit | Map to "journal durable" |
| Open-unlink | A deleted file stayed readable through its descriptor on FAT | Opt in with `enableOpenUnlinkEmulation`, or keep the object until the last close |
| **`.DS_Store`, `.localized`, `.hidden`** | Finder looked up `.DS_Store` in every folder it opened. At mount, Spotlight (`.Spotlight-V100`, `.metadata_never_index*`), Image Capture (`DCIM`) and Finder (`.hidden`) probe the root | Decide per name: `.DS_Store` as a local-only file, not synced (it churns versions); answer probes without requests |
| **Case sensitivity** | voidfs keys are case-sensitive and the volume says so. `Hello.txt` does not find `hello.txt` | Test Adobe and Office on a case-sensitive volume early. Some Mac apps assume case-insensitive storage |
| Unicode | Stored NFC; the module matches NFD lookups too (`open` by either form works) | Keep; normalize on create |
| Required attributes | Every lookup, `getattr` and read reply must fill `0x4000000000003fff`: the 14 documented attributes plus undocumented bit 62. "Incomplete population … results in undefined behavior" | Fill everything, always |

## 5. Kernel caching and coherence

This is the part of FSKit that decides whether a shared network drive feels correct. The module
granted the kernel read caching (`FSVolume.DataCacheHandler`). The change feed then told it what
changed elsewhere, and the spike tested what each `setCacheState` action actually does.

| After another machine overwrote a cached file (12 → 32 bytes) | Size seen | Content seen |
|---|---|---|
| Nothing, or `.update` | old | old |
| `.invalidate` or `.pushInvalidate` | **old (12)** | **new bytes cut at the old size**: a torn read |
| **`.revoke`** | new | new |

- `.invalidate` drops cached pages but not the cached size. The next read is clipped to the old
  size, and only its reply's attributes fix the size for later reads.
- `.revoke` drops the vnode, so the next access looks the item up again and gets fresh
  attributes.
- A descriptor opened before the change keeps its vnode after a revoke: it read the new bytes at
  the old length (`fstat` still said 12).
- There is no API to push new attributes into the kernel.
- **Negative lookups** (a name looked up before it existed) are cached by the kernel. Only a
  `.revoke` of the folder clears them, and only if the kernel holds no child of that folder.
  Revoking the known children first, then the folder, worked when siblings had been `stat`ed or
  read. It did **not** work while a sibling was held open.
- `.revoke` returned `kIOReturnBadArgument` for items the kernel had never looked up (known only
  from a listing) and for a deleted item. Harmless; the module ignores it.
- One bug was found and fixed in the spike: after the kernel reclaims an item, the parent's
  cached listing still pointed at the old instance. The next lookup returned it, a re-list
  registered a twin, and the feed's revoke then hit the twin with no effect. The fix: mark the
  parent listing stale on reclaim.

Consequences for Phase 2:

1. **Snapshot-at-open** (§8.6 of the plan). While a file is open, read the version that was
   current when it was opened (`versionId`, §4.5). Revoke on change, so the next open sees the
   new version. An open handle then sees a consistent old file instead of a torn one.
2. Revoke changed items, and revoke folders whose names changed after their children. Retry the
   folder revoke when its last open child closes (`DataCacheHandler.close`).
3. These tools exist only on macOS 27 (`FSKIT_API_AVAILABILITY_V3`). On macOS 26 a module has no
   way to evict kernel caches, so other machines' changes would stay invisible to anything cached.
   This follows from the SDK's availability annotations; the spike did not run on macOS 26.

## 6. Other findings

- **The SDK and the OS disagree.** On macOS 27.0 (26A428), FSKit sends
  `activateVolumeWithOptions:replyHandler:` and `deactivateVolumeWithOptions:replyHandler:`. The
  Xcode 27 beta SDK (26A5353p) declares `activateWithOptions:` and `deactivateWithOptions:`. The
  first mount crashed with "unrecognized selector". The module now answers to both (see
  `VoidfsVolume.swift`). Check again when a final Xcode 27 ships. The runtime's protocol
  selectors can be listed with `protocol_copyMethodDescriptionList`.
- **API generations.** macOS 27 deprecates the `FSVolume.*Operations` protocols for
  `FSVolume.*Handler` ones. The new ones reply with attributes (a lookup no longer needs a
  separate `getattr`) and take an `FSContext` with the caller's uid and gid. Supporting macOS 26
  would mean implementing both.
- `FSClient` on macOS 27 adds `mountSingleVolume` and `openFileSystemExtensionsSettings`, and
  `installedExtensions` reports whether the module is enabled.
- A `log stream --level debug` attached to the extension did not change the measured latency.
- zsh shadows `/usr/bin/log` with a builtin; scripts call it by full path.

## 7. Risks

1. **Profile-gated entitlements.** Developer ID with FSKit Module works for SpaceFS but is untried
   for voidfs's team. `fskit.mount` is not available to the team; a privileged mount helper, as
   SpaceFS uses, gets volumes into `/Volumes` without it.
2. **FSKit bugs and churn.** Two surprises in one day: the `RENAME_SWAP` data loss and the SDK
   selector skew. Expect more, and keep an app-compat rig (§8.1 of the plan) running on every
   macOS seed.
3. **Coherence limits** (§5): no attribute push, and negative entries pinned by open siblings.
   Snapshot-at-open covers files; a stale "missing" name can remain until the folder's open files
   close.
4. **Hung connections** block callers for 16 s per request (§3.4) until the client has a
   connectivity state.
5. **Crash = eject.** A crashing extension unmounts the drive. Keep the extension small; put the
   complex code in the agent.
6. **Per-request overhead inside the extension** (0.8 ms over loopback, cause not isolated).
   Harmless over a WAN; worth re-measuring once the agent exists.

## 8. Recommendation for Phase 2

1. **Keep the native FSKit module.** The spike found no blocker. Raise the minimum to **macOS
   27** (§5, §6).
2. **Agent-first architecture** (§4.1). Build `crates/client` as the core of a per-user agent,
   with an App-Group-prefixed XPC service, a chunk cache and journal in the App Group container,
   and a thin Swift extension.
3. **Coherence:** snapshot-at-open reads, revoke-based invalidation, and folder revokes retried
   on close. The feed stays the source of truth; the listing TTL is only a fallback.
4. **Connectivity state** with fail-fast and background probes; serve metadata from cache while
   offline, and say so in the menu bar.
5. **Writes:** xattrs everywhere, provenance folded into creates, rename-over, no AppleDouble,
   `.DS_Store` local-only. Test `RENAME_SWAP` on the voidfs module before shipping.
6. **Protocol RFC (proposed, not written):** add xattr names to `?x-voidfs-list` entries (for
   example `"xattrs": ["com.apple.FinderInfo"]`, and possibly values up to a small size). Today
   `hasXattrs` saves about 470 requests when Finder opens a 1,000-file folder of files without
   xattrs. Once files carry `com.apple.provenance`, every `getxattr` would cost one `?x-voidfs-attrs`
   request per file. By [CONTRIBUTING](../../CONTRIBUTING.md#3-changes-to-the-specs) this needs an
   RFC, spec text and a conformance case.
7. **Signing:** create a Developer ID provisioning profile with FSKit Module (SpaceFS ships one);
   mount into `/Volumes` through a privileged helper rather than waiting for `fskit.mount`;
   document the contributor path.

### How the Rust core would reach Swift (UniFFI)

Decided, not built:

- `crates/client-ffi` exports the core with UniFFI proc-macros: objects such as `Drive`, async
  functions (`list`, `read`, `write`, `commit`) that UniFFI maps to Swift `async`, and callback
  interfaces for feed events.
- The build compiles a static library for `aarch64-apple-darwin` (add `x86_64-apple-darwin` and
  `lipo` only if an Intel build is ever needed) and runs `uniffi-bindgen generate --language
  swift`. Then
  `xcodebuild -create-xcframework` packages `VoidfsCore.xcframework` plus `VoidfsCore.swift`. An
  Xcode build phase (or a script run before `xcodebuild`) produces it.
- The core runs its own tokio runtime and does HTTP and SigV4 with the same crates as
  `voidfs-conformance` (`aws-sigv4`, path-style, single percent-encoding).
- The agent links the XCFramework and exports it over `NSXPCConnection`. The extension does
  **not** link Rust. It stays Swift, speaking XPC and reading chunk files.
- The in-process alternative would link the XCFramework into the extension instead. It works
  technically, but §4.1 rules it out.

## 9. Limits of this spike

- Loopback only: no Cloudflare R2 or WAN numbers yet, and no TLS.
- Read-only mount; the writable behaviours in §4.5 come from Apple's FAT module and from what the
  read-only mount observed.
- One machine, one macOS build, a beta Xcode.
- Not tested: Finder sidebar placement, logout and sleep behaviour, memory limits on the
  extension, `FSPathURLResource`, and running on macOS 26.
