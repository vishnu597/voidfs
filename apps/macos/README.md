# voidfs for macOS

The macOS drive: `voidfs.app`, a SwiftUI menu-bar app that contains an FSKit module
(`VoidfsFS.appex`). Today it is the **FSKit spike**: a read-only mount of one drive. The
findings, numbers and Phase 2 plan are in [`docs/spikes/fskit.md`](../../docs/spikes/fskit.md).

| Folder | What |
|---|---|
| `App/` | The host app, and the launch agent that bridges the extension to the daemon. Run from a terminal it also takes `status`, `save`, `mount`, `settings` and the agent and bridge commands |
| `FSModule/` | The FSKit module: a `FSUnaryFileSystem` that serves `voidfs://host[:port]/drive` URLs |
| `Shared/` | SigV4 signing, the HTTP client, where access keys are stored, and the bridge's XPC interface, client and checks |
| `Probe/` | `voidfs-probe`, the module's HTTP calls without FSKit, for baselines |
| `Config/` | The extension's `Info.plist` and the entitlements |
| `scripts/` | Seeding, benchmarks and the semantics probes the spike used |

## Requirements

- macOS 27 and Xcode 27.
- An Apple Developer team. `com.apple.developer.fskit.fsmodule` is a restricted entitlement:
  without a provisioning profile that grants it, macOS kills the extension at launch. With Xcode
  signed in to your team, automatic signing creates the profile. The project uses team
  `HAUTK68F56`; set `DEVELOPMENT_TEAM` to your own, and change the App Group
  (`HAUTK68F56.dev.voidfs` in `Config/*.entitlements`, `Shared/MountStore.swift` and
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
Extensions). Then store an access key for the drive (the secret is read from stdin) and mount it:

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs save voidfs://127.0.0.1:9000/spike VFSPIKEKEY0000000000
```

```bash
mkdir -p /tmp/voidfs-mnt && mount -F -t voidfs voidfs://127.0.0.1:9000/spike /tmp/voidfs-mnt
```

Unmount with `umount /tmp/voidfs-mnt`. Logs: `log stream --predicate 'subsystem == "dev.voidfs"'`.

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
extension's code signature. The daemon must run with its state in the App Group container:

```bash
VOIDFS_STATE_DIR="$HOME/Library/Group Containers/HAUTK68F56.dev.voidfs/daemon" target/release/void daemon start
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
`wire` cover restarts, an outside change, peer refusal and the agent's codecs. In a mount, the
extension runs the same checks through the real agent when you look up these names. The results go
to the log, and a `~<anything>` suffix runs one again, since the kernel caches a missing name:

| Name | Runs |
|---|---|
| `.voidfs-xpc` | 500 XPC round trips of 4 KiB to the agent |
| `.voidfs-bridge` | The checks and timings, end to end |
| `.voidfs-bridge-daemon-restart` | Waits while you restart or kill the daemon, then checks what survived |
| `.voidfs-bridge-agent-restart` | The same for the agent (`launchctl kickstart -k gui/$(id -u)/dev.voidfs.agent`, or `kill -9`) |
| `.voidfs-bridge-remote:<name>` | Waits for an outside change to `<name>`, such as a `void upload` |
