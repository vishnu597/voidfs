# voidfs for macOS

The macOS drive: `voidfs.app`, a SwiftUI menu-bar app that contains an FSKit module
(`VoidfsFS.appex`) and runs as the launch agent that bridges the module to the `void` daemon. A
mount is writable unless mounted read-only ([step 5, item 4](../../docs/step-5-macos.md#writable-fskit-12-october));
the module reads and writes every byte and name through the daemon, which holds the server and
key and stages writes on this Mac before it publishes them. The
FSKit spike that started it is in [`docs/spikes/fskit.md`](../../docs/spikes/fskit.md).

| Folder | What |
|---|---|
| `App/` | The host app, the launch agent that bridges the extension to the daemon, and the bridge's in-process checks. Run from a terminal it also takes `status`, `mount`, `settings` and the agent and bridge commands |
| `FSModule/` | The FSKit module: a `FSUnaryFileSystem` that serves `voidfs://<drive>` resources from daemon sessions |
| `Shared/` | The bridge's XPC interface and the extension's client of it, with its metadata memo |
| `Probe/` | `voidfs-probe`, the spike's HTTP calls without FSKit or the daemon, for baselines |
| `Config/` | The extension's `Info.plist` and the entitlements |
| `scripts/` | Measurements and checks of a mounted drive, the semantics probes, and the release script |

## Requirements

- macOS 27 and Xcode 27.
- An Apple Developer team. `com.apple.developer.fskit.fsmodule` is a restricted entitlement:
  without a provisioning profile that grants it, macOS kills the extension at launch. With Xcode
  signed in to your team, automatic signing creates the profile. The project uses team
  `HAUTK68F56`; set `DEVELOPMENT_TEAM` to your own, and change the App Group
  (`HAUTK68F56.dev.voidfs` in `Config/*.entitlements`, `Shared/Bridge.swift` and
  `LaunchAgents/dev.voidfs.agent.plist`) and the team in the bridge's code-signing requirements
  (`Shared/Bridge.swift`, `App/SelfTest.swift`) to match.

## Build, enable, mount

```bash
xcodebuild -project apps/macos/voidfs.xcodeproj -scheme voidfs -configuration Release -derivedDataPath apps/macos/build -allowProvisioningUpdates -allowProvisioningDeviceRegistration build
```

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs settings
```

Switch on **voidfs** in the pane that opens (General → Login Items & Extensions → File System
Extensions). The module reaches the daemon through the app's launch agent: start the daemon (it
keeps the server and key, from `VOIDFS_ENDPOINT`, `VOIDFS_ACCESS_KEY_ID` and
`VOIDFS_SECRET_ACCESS_KEY` the first time) and register the agent from the copy you built (see
[the bridge](#the-bridge-to-the-daemon) below):

```bash
void daemon start
```

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs agent-register
```

Then mount a drive by its alias or id. `void mount` asks the daemon, which mounts it with the
module (the `fskit` adapter) and mounts it again when it starts:

```bash
void mount footage ~/voidfs/footage
```

`mount` works too, into any empty folder you own; the resource names only the drive, never a
server or a key:

```bash
mkdir -p /tmp/voidfs-mnt && mount -F -t voidfs voidfs://footage /tmp/voidfs-mnt
```

Unmount with `void unmount footage` or `umount /tmp/voidfs-mnt`. The app's `mount <drive>
[mountpoint]` command and its menu ask the daemon as `void mount` does. Logs:
`log stream --predicate 'subsystem == "dev.voidfs"'` (the module logs as category `fs`, the agent
as `agent`).

Another machine's changes reach the mount through the daemon's change feed, in tens of
milliseconds, with one limit FSKit leaves: a name this Mac looked up and missed stays missing to
path lookups while anything below its folder is in use, and in the drive's top folder until the
drive is mounted again; listings show it at once
([the blocker](../../docs/step-5-macos.md#the-blocker-a-name-missed-while-its-folder-is-in-use)).

Writes go to the daemon, which stages them on this Mac: `write(2)` returns once the bytes are
staged (they survive the daemon, the agent or the module being killed), they reach the disk
within a second, since FSKit passes no `fsync` to a module, and they publish on close or after
two seconds without writes (`void uploads` shows them). `.DS_Store` stays on this Mac; locks are
this Mac's only. Two limits: `renamex_np(RENAME_SWAP)` replaces the destination on any FSKit
volume instead of swapping, and xattrs (resource forks included) are limited to 64 KiB a file
([writable FSKit](../../docs/step-5-macos.md#writable-fskit-12-october)). `mount -o trace` logs
every callback the module gets.

## Checking a mount

The scripts take a drive filled by `cargo run --release -p voidfs-sdk --example mount_seed`
(the spike's tree, as `scripts/seed.py` makes it with boto3), and `mount_peer` from the same
examples plays another machine:

| Script | What |
|---|---|
| `bench.sh <resource> [mountpoint]` | Cold and warm listings, sequential and random reads; `VOIDFS_BENCH_COLD` runs before each remount (to empty the daemon's cache, say) |
| `manifest.sh <mountpoint>` | Every path, type, size, mode, mtime and SHA-256, to compare two mounts with `diff` |
| `coherence.py <mountpoint> <peer…>` | Overwrites, renames, deletes and new files from another client, with a sibling and the file itself held open |
| `offline.py <mountpoint> <relay pid> "<relay command>" <peer…>` | The server refusing and hanging behind a relay, and coming back |
| `restarts.py <mountpoint> <label> <command…>` | A file held open across a restart of the daemon or the agent |
| `semantics.c <folder>` | `RENAME_EXCL`, `RENAME_SWAP`, open-unlink, mmap and msync, locks, xattrs, Unicode names: build with `cc` and run on a mount |
| `writeprobe.c <folder> <test>` | Timed writes, `fsync`, `F_FULLFSYNC`, closes, `sync` and mmap stores, to see which call waits for the module; `unflushed` and `dirty` wait for a kill |

## Release build: Developer ID, notarized

`scripts/release.sh` builds the app as it ships: archived, signed with Developer ID, notarized and
stapled, then checked with `codesign`, `spctl` and `stapler` (the
[signed-bundle probe](../../docs/step-5-macos.md#signed-bundle-probe-9-october) ran it on
9 October). It needs, for team `HAUTK68F56` or your own:

- a Developer ID Application certificate in the login keychain;
- a Developer ID provisioning profile for `dev.voidfs.app.fskit` with the FSKit Module capability,
  named `voidfs FSKit Developer ID` (or change `scripts/ExportOptions-DeveloperID.plist`), installed
  in `~/Library/Developer/Xcode/UserData/Provisioning Profiles/<UUID>.provisionprofile`;
- notarization credentials in the keychain, saved once with an app-specific password:

```bash
xcrun notarytool store-credentials voidfs-notary --apple-id <Apple ID> --team-id HAUTK68F56
```

```bash
apps/macos/scripts/release.sh voidfs-notary
```

The result is `apps/macos/build/release/voidfs.app`. Install it by moving it into Applications in
Finder; copied there by a script, a quarantined app runs from a translocated copy. Building
registers the build folders' copies with LaunchServices, and FSKit can use any registered copy of
the module: `voidfs status` shows which one it uses, and
`/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -u <path to voidfs.app>`
forgets a copy without deleting it.

## The bridge to the daemon

The app's launch agent is the extension's bridge to the Rust daemon (step 5, item 2): the
sandboxed extension calls it over XPC on `HAUTK68F56.dev.voidfs.agent`, and it forwards a typed,
bounded set of filesystem calls to the daemon's socket and relays its invalidations
([the Swift bridge](../../docs/step-5-macos.md#the-swift-bridge-9-october)). It accepts only the
extension's code signature. It connects to the daemon the CLI runs, at its default state
directory, `~/Library/Application Support/voidfs`
([one daemon](../../docs/step-5-macos.md#one-daemon-for-the-cli-and-the-app-9-october)):

```bash
void daemon start
```

Register the agent from the copy of the app you built (registering from another copy changes which
binary launchd runs; after switching between a Developer ID and a development copy, launchd may
refuse the first start, and unregistering and registering again fixes it):

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs agent-register
```

`agent-status` reports it, `agent-unregister` removes it, and `bridge-check` shows that it turns
the app itself away. `bridge-selftest <drive>` runs the bridge's checks in process, without
launchd or FSKit; its modes `daemon-restart`, `agent-restart`, `remote <key>`, `refused` and
`wire` cover restarts, an outside change, peer refusal and the agent's codecs. (Until item 3 the
extension also ran them when a mount looked up names such as `.voidfs-bridge`; the mount itself now
goes through the agent, and those names are gone.)

To point the agent at a daemon elsewhere, as `VOIDFS_STATE_DIR` points the CLI (a test daemon, say,
so that nothing touches the shared location), set a default and restart the agent; delete the
default afterwards:

```bash
defaults write dev.voidfs.app VoidfsStateDirectory /tmp/vfsd.test
```

```bash
launchctl kickstart -k gui/$(id -u)/dev.voidfs.agent
```
