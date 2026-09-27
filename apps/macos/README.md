# voidfs for macOS

The macOS drive: `voidfs.app`, a SwiftUI menu-bar app that contains an FSKit module
(`VoidfsFS.appex`). Today it is the **FSKit spike**: a read-only mount of one drive. The
findings, numbers and Phase 2 plan are in [`docs/spikes/fskit.md`](../../docs/spikes/fskit.md).

| Folder | What |
|---|---|
| `App/` | The host app. Run from a terminal it also takes `status`, `save`, `mount` and `settings` |
| `FSModule/` | The FSKit module: a `FSUnaryFileSystem` that serves `voidfs://host[:port]/drive` URLs |
| `Shared/` | SigV4 signing, the HTTP client, where access keys are stored |
| `Probe/` | `voidfs-probe`, the module's HTTP calls without FSKit, for baselines |
| `Config/` | The extension's `Info.plist` and the entitlements |
| `scripts/` | Seeding, benchmarks and the semantics probes the spike used |

## Requirements

- macOS 27 and Xcode 27.
- An Apple Developer team. `com.apple.developer.fskit.fsmodule` is a restricted entitlement:
  without a provisioning profile that grants it, macOS kills the extension at launch. With Xcode
  signed in to your team, automatic signing creates the profile. The project uses team
  `HAUTK68F56`; set `DEVELOPMENT_TEAM` to your own, and change the App Group
  (`HAUTK68F56.dev.voidfs` in `Config/*.entitlements` and `Shared/MountStore.swift`) to match.

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
