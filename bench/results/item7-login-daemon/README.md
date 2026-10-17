# The app's login daemon, 17 October 2026

Step 5, item 7, first slice ([the slice](../../docs/step-5-macos.md#the-apps-login-daemon-17-october)):
voidfs.app carries its own `void` and starts the daemon at login through SMAppService, and the
terminal's `void daemon install` agent gives way to it. macOS 27.0.1 (26A434), Xcode 27.0
(27A5194q), Apple M5 Pro, branch `item7-login-daemon` on `main` at `4dd327a`.

The [live run](#the-live-run), approved by the user, registered the job with SMAppService from a
build of `7fb2658`. It used a throwaway state directory, set through `VoidfsStateDirectory`, and
a local test server, and was then undone. Nothing here wrote to
`~/Library/Application Support/voidfs`, which was absent before and after. Two commands read it:
`void daemon status`, from this checkout, found no daemon on its socket, and the drawings' first
row checked whether its `daemon.json` exists (it doesn't).

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

- [now](raw/look/now.png): this Mac's state at the time. The file system extension is enabled.
  The Finder extension is reported disabled: its registration had been lost during these builds,
  and it was put back after the live run, as described under
  [registration](#registration-during-these-builds). Start at login is off, because the default
  state directory has no `daemon.json`; the menu explains how to make one.
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
- Adding the extension back was blocked at first. The user then approved restoring everything
  with the live run, and after it `pluginkit -a` of sleepy-germain's Finder extension put it
  back. pluginkit elects it ("+") as before, so `pluginkit -e use` wasn't needed.

```bash
pluginkit -a ~/Coding/voidfs/.claude/worktrees/sleepy-germain-0cf80f/apps/macos/build/Build/Products/Release/voidfs.app/Contents/PlugIns/VoidfsFinder.appex
```

At the end of the live run ([after.txt](raw/live/after.txt)):
- pluginkit's voidfs plug-ins match the first snapshot, apart from the Finder extension's new
  UUID and time;
- LaunchServices' records are as before, apart from sleepy-germain's new record id;
- FSKit names `/Applications`'s copy as the module, with the switch on. It named sleepy-germain's
  at the start, then compassionate-payne's.
- No `dev.voidfs.app.daemon` job, no `VoidfsStateDirectory` default and no voidfs plist in
  `~/Library/LaunchAgents` exist.

## The live run

Approved by the user, run on 17 October from 13:05 to 13:07 (local time). Its outputs are in
[raw/live](raw/live/), with the daemon's whole log in [daemon.log](raw/live/daemon.log).

**The rig.**
- The Release build of `7fb2658`, in the scratch folder. Building registered it with LaunchServices
  and pluginkit beside the other copies, so a copy of the module stayed registered throughout.
- `voidfs-server` on `127.0.0.1:9372` with a filesystem store and the throwaway local key, and a
  new drive.
- A state directory under `/private/tmp/vli.Ngb9LM`, set as the app's `VoidfsStateDirectory`
  before anything was registered.
- The bridge agent was already registered and enabled, so Start at login left it alone.
- Snapshots: [before](raw/live/before.txt) and [after](raw/live/after.txt).

**What happened**, each from the bundled `void` and the app's `voidfs`:

1. **Not configured** ([output](raw/live/01-not-configured.txt)). With no `daemon.json` in the
   directory, `login-item on` explained how to make one, registered nothing, and exited 1.
2. **A real terminal agent** ([output](raw/live/02-terminal-agent.txt)). `void daemon start`
   kept the connection and started a daemon. `void daemon install` stopped it and loaded
   `~/Library/LaunchAgents/dev.voidfs.daemon.plist` for the directory. launchd ran its daemon,
   pid 56983, and `daemon status` named the agent. The plist holds no secret.
3. **Start at login** ([output](raw/live/03-login-item-on.txt)). `login-item on` registered the
   job, enabled at once with no approval step, and said "The daemon (pid 57104) starts at login.
   It took over from the daemon that was running (pid 56983). It replaced the terminal's agent".
   - `launchctl print` shows the job running `void daemon run --app` with pid 57104, the
     daemon's own pid: the `exec` into the log kept it.
   - The agent is gone from launchd and from `~/Library/LaunchAgents`, and its daemon exited.
   - The log has the agent's daemon stopping on SIGTERM at 17:05:50.141, the takeover line, and
     the app's daemon starting at 17:05:50.256.
   - `daemon status` says "starts at login: voidfs.app".
4. **While the app has it** ([output](raw/live/04-refusal-stop-start.txt)).
   - `void daemon install` failed with `ManagedByApp` and wrote no plist.
   - `login-item on` again said "The daemon already starts at login."
   - `void daemon stop` stopped it, and 15 s later launchd still hadn't started it again (last
     exit code 0).
   - `void daemon start` kickstarted the job: run 2, pid 57309.
5. **A crash** ([output](raw/live/05-crash-restart.txt)). After `kill -9`, launchd ran the
   daemon again 1.1 s later: run 3, last terminating signal "Killed: 9".
6. **A daemon started by hand** ([output](raw/live/06-hand-started-takeover.txt)).
   - After `daemon stop`, `void daemon run` was started outside launchd, then the job was
     kickstarted, as a login would.
   - The app's daemon asked the hand-started one to stop (it exited 0), logged "stopped the daemon
     that was running (pid 57526), to take over", and answered 0.3 s later.
7. **Turning it off** ([output](raw/live/07-login-item-off.txt)). `login-item off` unregistered
   the job. launchd stopped the daemon with SIGTERM, `launchctl print` no longer finds the job,
   and `login-item status` says off.

**Restored.** In order:
1. the default deleted;
2. the server stopped;
3. the run directory removed;
4. the scratch copy's plug-ins removed and the copy unregistered;
5. the Finder extension added back (above).

Nothing from the run was left running.

## Not run

- **Login Items' approval flow.** Registration was enabled at once. The "needs approval" state
  appears when the user switches the item off in System Settings, which is theirs to do.
- **Logout and login, and a reboot**, which the done-when asks for, with remembered mounts.
  Nothing was mounted in this run.
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
