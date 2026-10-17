# The app's login daemon, 17 October 2026

Step 5, item 7, first slice ([the slice](../../docs/step-5-macos.md#the-apps-login-daemon-17-october)):
voidfs.app carries its own `void` and starts the daemon at login through SMAppService, and the
terminal's `void daemon install` agent gives way to it. macOS 27.0.1 (26A434), Xcode 27.0
(27A5194q), Apple M5 Pro, branch `item7-login-daemon` on `main` at `4dd327a`.

Nothing here registered the app's job with SMAppService, wrote a default, or wrote to
`~/Library/Application Support/voidfs`. Two commands read it: `void daemon status`, from this
checkout, found no daemon on its socket, and the drawings' first row checked whether its
`daemon.json` exists (it doesn't). The live run, on a registered copy, waits for the user's OK
([not run](#not-run)).

## The takeover, with stand-ins for launchctl and defaults

`the_apps_daemon_takes_over_the_state_directory` (`crates/voidfs-cli/tests/mounts.rs`) runs the
app's daemon as launchd runs the job, in an environment with no `VOIDFS_STATE_DIR` and no
connection. A stand-in `defaults` answers the app's `VoidfsStateDirectory` with the test's
directory. A stand-in `launchctl` logs its calls, loads and runs the two jobs, and answers `print`
for a loaded one. In order:

1. `void daemon install` loads the terminal's agent, whose daemon answers; `daemon status` names
   `dev.voidfs.daemon` as the login job.
2. The app's job is loaded and `void daemon run --app` started. The agent is booted out and its
   plist removed. Its daemon stops, and the app's daemon answers with the pid of the process
   that was started: it ran itself again in place, with its output in the test directory's
   `daemon.log`. The log has the takeover's line and the daemon's start.
3. While the app has it:
   - `daemon install` fails with `ManagedByApp` and writes no plist;
   - `daemon status` says "starts at login: voidfs.app (dev.voidfs.app.daemon, Start at login in
     its menu)";
   - `daemon uninstall` says the app starts it.
4. `daemon stop` makes the app's daemon exit 0, which launchd leaves stopped. `daemon start`
   then kickstarts the app's job.
5. A daemon started by hand with `void daemon run` is asked to stop by the next app daemon, which
   then answers. The log names the pid it stopped.
6. Apart from `print`, launchctl was asked, in order: install's bootout and bootstrap, the app
   daemon's bootout of the agent, uninstall's bootout, and `daemon start`'s kickstart of
   `dev.voidfs.app.daemon`.

Results:

- **Three runs of `tests/mounts.rs` passed**, 4 tests each ([output](raw/tests-mounts-3-runs.txt)).
- **All 54 of the CLI's tests passed**, and workspace clippy passed with warnings denied
  ([output](raw/tests-cli-all.txt)).
- **Three breaks each failed the test** ([output](raw/breaks.txt)), and the files were restored
  byte for byte afterwards:
  - With no takeover, the app's daemon exits "a daemon is already running … (pid …)", and the
    test times out waiting for it.
  - With no refusal, `daemon install` installs the agent beside the app's job.
  - With the app's job left out of who starts the daemon, `daemon status` reports no login job.

## The bundle

The Release build of this branch, into a scratch derived-data folder
([listing and signatures](raw/bundle.txt)):

- `Contents/MacOS/void` is there, 33.9 MB, arm64, version `0.0.0 (4dd327a, 2026-10-17)` (the
  branch's changes weren't committed yet).
- It is signed with the build's identity (Apple Development, team HAUTK68F56), identifier
  `dev.voidfs.void`, with the hardened runtime.
- `codesign --verify --deep --strict` passes on the app and validates `void` inside it.
- `Contents/Library/LaunchAgents/dev.voidfs.app.daemon.plist` has `BundleProgram`
  `Contents/MacOS/void`, the arguments `void daemon run --app`, `RunAtLoad`,
  `KeepAlive.SuccessfulExit` false, and the app as associated bundle.
- The "Bundle void" phase's first `cargo build --release` took 61 s, and the whole build 84 s.
  Later builds reused cargo's work.

`voidfs login-item status` on that copy said "not in this copy of the app" before the fix: SMAppService
reports `notFound` for a job that was never registered. The app now checks its bundle for the
plist and `void` itself.

Two small changes came after the last build:
- `login-item on` answers at once when the job is already on;
- `daemon run --app` refuses off macOS.

They were checked with `swiftc -typecheck` over the app target's sources, and with the CLI's
tests and clippy. The app wasn't rebuilt, since each build registers the copy.

## The menu's rows

`voidfs login-item look <folder>` drew the setup rows offscreen ([drawings](raw/look/)):

- [now](raw/look/now.png): this Mac's state. The file system extension is enabled. The Finder
  extension is reported disabled: its registration was lost during these builds, as described
  under [registration](#registration-during-these-builds). Start at login is off, because the
  default state directory has no `daemon.json`; the menu explains how to make one.
- The five states, with sample notes: [on](raw/look/on.png), [off](raw/look/off.png),
  [needs approval](raw/look/needs-approval.png) (with a button for Login Items),
  [not bundled](raw/look/not-bundled.png) and [not configured](raw/look/not-configured.png).
  Some drawings show an extension disabled, for its row and button.

## Registration during these builds

The before and after states are recorded in [registration.txt](raw/registration.txt).

- Each Release build registered the scratch copy with LaunchServices, and pluginkit then chose its
  FSKit and Finder extensions. Running its `voidfs` for the drawings did the same.
- Each time, `pluginkit -r` and `lsregister -u` of the scratch copy put pluginkit's list back as it
  was.
- After the second cleanup, FSKit named compassionate-payne's copy as the module instead of
  sleepy-germain's. The switch stayed on.
- An `lsregister -f` of sleepy-germain's copy, meant to make FSKit name it again, did not do that:
  FSKit named the main checkout's copy, and sleepy-germain's Finder extension was dropped from
  pluginkit.
- Adding the extension back and electing it (`pluginkit -a`, then `pluginkit -e use`) was blocked
  and is left to the user.

To put the Finder extension back as it was, as after item 8's live run:

```bash
pluginkit -a ~/Coding/voidfs/.claude/worktrees/sleepy-germain-0cf80f/apps/macos/build/Build/Products/Release/voidfs.app/Contents/PlugIns/VoidfsFinder.appex
```

```bash
pluginkit -e use -i dev.voidfs.app.findersync
```

At the end, FSKit names compassionate-payne's copy with the switch on. pluginkit has no voidfs
Finder extension, so there are no badges until it is added back. LaunchServices' records are as
before, apart from sleepy-germain's new record id. No `dev.voidfs.app.daemon` job, no
`VoidfsStateDirectory` default and no voidfs plist in `~/Library/LaunchAgents` exist.

## Not run

Each of these needs the user's OK:

- **SMAppService registration of the job** (`voidfs login-item on`), from a copy that is the only
  one registered, with a test state directory set through `VoidfsStateDirectory`. This would check:
  - the takeover from a real `void daemon install` agent;
  - Login Items' approval flow;
  - turning it off;
  - logout and login, and a reboot.
- **The Developer ID export**: whether `xcodebuild -exportArchive` re-signs `void` with the Developer
  ID identity and a timestamp, and whether notarization accepts the app with it.

## Replay

```bash
cargo test -p voidfs-cli --locked --test mounts
```

```bash
xcodebuild -project apps/macos/voidfs.xcodeproj -scheme voidfs -configuration Release -derivedDataPath apps/macos/build -allowProvisioningUpdates build
```

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs login-item look /tmp/voidfs-look
```

The build, and running the app's binary, register that copy with LaunchServices and its
extensions with pluginkit.
