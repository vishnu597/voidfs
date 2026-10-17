# The server key in the Keychain, 17 October 2026

Step 5, item 7, second slice ([the slice](../../docs/step-5-macos.md#the-server-key-in-the-keychain-17-october)):
you sign in to the voidfs server from voidfs.app's menu, the key goes into the shared team
Keychain, and the app's login job hands it to the daemon on a pipe. macOS 27.0.1 (26A434),
Xcode 27.0 (27A5194q), Apple M5 Pro, branch `item7-keychain` on `item7-developer-id` at
`45310f4` (on `main` at `57cfae8`).

The user chose, on 17 October:
- **The shared team Keychain**: the data-protection keychain, access group `HAUTK68F56.dev.voidfs`.
- **`daemon.json` moves in**, and the file goes, once the app owns the state directory.
- **The app hands the key over**, so that only `dev.voidfs.app` needs provisioning profiles. The
  job runs the app's binary, which reads the item and starts `void` with the key on a pipe. The
  [probe](../item7-keychain-probe/README.md) showed a `void` reading it itself would need its own
  bundle, App ID and profiles.

The first runs used a hand-signed test copy, because Xcode had no account to make a profile for
`dev.voidfs.app`. The user then signed Xcode in to the team, and the signed Release build ran the
same self-test and sign-ins. The user then approved [the live run](#the-live-run), which
registered the job from that build with a throwaway state directory, and was undone.

Apart from the live run, nothing here registered the job with SMAppService or wrote defaults.
Nothing touched
`~/Library/Application Support/voidfs`, which was absent before and after
([snapshots](raw/registration.txt)). The drawings' first row read whether the Keychain has a key
for that directory (it doesn't) and whether its `daemon.json` exists (it doesn't).

## The CLI, with stand-ins for launchctl and defaults

`the_apps_daemon_takes_its_key_from_the_app` (`crates/voidfs-cli/tests/mounts.rs`) loads the
app's job in a stand-in `launchctl`, whose kickstart runs `void daemon run --app --logged
--key-file /dev/stdin` as the launcher does: under `env -i`, with `VOIDFS_STATE_DIR` and
`VOIDFS_REGION` set, its output appended to `daemon.log`, and the key on stdin when the stand-in
Keychain, a file in the test's folder, has one.

1. **No key in the app.** `void daemon start`, run with a full key in its environment, fails with
   `NoCredentials` within 10 s. The message says to sign in from the menu and quotes the log's
   line. No `daemon.json` is written.
2. **A key in the app.** `void daemon start` from an environment with no key succeeds. `daemon
   status` names `dev.voidfs.app.daemon` and the app's server. There is still no `daemon.json`,
   and the log has the daemon's start and no secret.
3. Apart from `print`, launchctl was asked for two kickstarts of `gui/<uid>/dev.voidfs.app.daemon`.

Results ([output](raw/tests-cli.txt)):

- **All 55 of the CLI's tests passed**, and workspace clippy passed with warnings denied.
- **Two breaks each failed the test** ([output](raw/breaks.txt)), and the file was restored byte
  for byte afterwards:
  - With `daemon start` writing `daemon.json` for the app's directory too, step 1 succeeds
    instead of failing.
  - With no early answer, step 1 fails with `DaemonFailed` after 20 s instead of `NoCredentials`.

## The build

- **The signed build failed at first** ([output](raw/build.txt)). The app target now claims
  `keychain-access-groups`, so it needs a development profile that covers `dev.voidfs.app`. Xcode
  had no account signed in ("No Accounts"), so `-allowProvisioningUpdates` couldn't make one.
- **Signed in, it built.** With the user's account added in Xcode → Settings → Accounts, the
  Release build with `-allowProvisioningUpdates` succeeded:
  - Xcode made the team's wildcard profile, "Mac Team Provisioning Profile: *" (`db3784cb…`,
    `HAUTK68F56.*`, granting `keychain-access-groups HAUTK68F56.*`, expiring 17 October 2027),
    and embedded it in the app. The FSKit extension kept its own profile, and the Finder
    extension needs none.
  - The app is signed with Apple Development, `application-identifier HAUTK68F56.dev.voidfs.app`,
    the group, and `get-task-allow`, as a development build has.
  - The bundled `void` is signed with Apple Development too, and meets the handover's code
    requirement. `codesign --verify --deep --strict` passes.
  - The Developer ID export still needs a profile named "voidfs App Developer ID"
    (`ExportOptions-DeveloperID.plist`), made on the developer website.
- **Without a profile, a Developer ID bundle is killed.** Test copy 1 was the unsigned build with
  its extensions removed, signed with Developer ID and the app's entitlements. It was killed at
  launch (exit 137), amfid -413 "No matching profile found". Bare Developer ID tools ran without
  one in the probe; bundles don't.
- **Test copy 2 ran everything below first.** It is the unsigned Release build of this branch with
  `Contents/Extensions` and `Contents/PlugIns` removed, so that no plug-in could register.
  - Its `void` is signed with Developer ID, identifier `dev.voidfs.void`, the hardened runtime.
  - The app is signed with Apple Development, claiming the group with
    `application-identifier HAUTK68F56.dev.voidfs.app.fskit`. It embeds Xcode's development
    profile for the FSKit extension (`212b8b5a…`), which grants `keychain-access-groups
    HAUTK68F56.*`.
  - That profile is only a test harness: the real app needs its own. `codesign --verify --deep
    --strict` passes.
- **Signing prompted twice.** The first Developer ID signing asked to use the signing key, and
  the user allowed it. Apple Development signing didn't ask.

## The Keychain self-test

`voidfs server-key selftest` used a throwaway state directory (`/private/tmp/vks.…`, made by
`mkdtemp`) and a server address nothing listens on. All 19 checks passed on test copy 2
([output](raw/selftest.txt)), and again on the signed build, whose `void` is signed with Apple
Development rather than Developer ID ([output](raw/selftest-real-build.txt)):

- **The store.** No key at first. A saved key reads back, saving again replaces it, and the key
  can be removed.
- **Adopting `daemon.json`.** With no key in the Keychain, the file's key moves in and the file is
  deleted. With a key there already, the Keychain's key wins and the file is deleted.
- **The handover's check.** The bundled `void` passes the code requirement (`anchor apple generic`,
  team `HAUTK68F56`, identifier `dev.voidfs.void`) and runs. `/bin/cat`, and an ad hoc copy of
  `void` signed with identifier `dev.voidfs.void`, are refused (-67050) before the key is written.
- **The launcher**, `voidfs daemon-launch -VoidfsStateDirectory <dir>`, as the job runs it:
  - its daemon answers on the socket with the Keychain's server, and runs as the launcher's
    child;
  - `daemon.json` is deleted, and the Keychain's key kept;
  - when the daemon is stopped as asked, the launcher exits 0;
  - a SIGTERM to the launcher reaches the daemon, which stops, and the launcher exits 0;
  - when the daemon is killed with `kill -9`, the launcher dies by signal 9 too, so that launchd
    runs the job again;
  - with no key it logs the line `void daemon start` looks for and exits 0;
  - no secret is in the log.

The item and the directory were removed at the end.

## Signing in from the command line

`voidfs server-key …` against a local `voidfs-server --store memory`, with a throwaway admin key
from `void keys generate` and a throwaway state directory, from test copy 2
([output](raw/sign-in.txt), the directory shown as `<dir>`):

| Run | Result |
|---|---|
| `status` | not signed in |
| `sign-in` with a key the server doesn't know | refused, `403 InvalidAccessKeyId`; nothing kept |
| `sign-in` to a port nothing listens on | refused, connection refused |
| `sign-in` to an `ftp://` URL | refused before any check (exit 2) |
| `sign-in --key-file key.env`, the server from the file's `VOIDFS_ENDPOINT` | signed in |
| `sign-in` again with `--region eu-west-1` | replaced; `status` shows the region |
| `sign-out`, twice | "Signed out.", then "Not signed in." |

The signed build repeated the unknown key, `key.env`, a key file on stdin (`--key-file -`) with
`--region eu-west-1`, and the sign-out, with the same results ([output](raw/sign-in-real-build.txt)).

No secret appeared in the output. Each check runs the bundled `void drives` with the key on
stdin. The connection-refused message is the CLI's whole error, which is long for the menu. The
job wasn't registered, so each sign-in ended with "Turn on Start at login to start the daemon
with it". The restart that follows a sign-in while the job is on is untested.

## The menu

`voidfs login-item look <folder>` drew the setup rows offscreen:
[not signed in](raw/look/not-configured.png), [the sign-in form with a refusal](raw/look/sign-in.png),
and [signed in](raw/look/signed-in.png). The secret field shows dots. Start at login stays off
until you sign in, with "Sign in to your voidfs server first: the daemon starts with that key."

## Registration

[Snapshots](raw/registration.txt) of LaunchServices, pluginkit, FSKit, the launchd jobs, the
`VoidfsStateDirectory` default, `~/Library/LaunchAgents` and the state directory:

- The unsigned build registered its copy with LaunchServices, and `lsregister -u` removed it.
  pluginkit ignored its unsigned extensions.
- After test copy 2's runs, LaunchServices had a record for it, removed with `lsregister -u`. It
  came back by itself before the signed build, so both test copies were unregistered and deleted.
- The signed build registered its copy with LaunchServices, and pluginkit chose its FSKit and
  Finder extensions. `pluginkit -r` of both and `lsregister -u` of the copy put LaunchServices and
  pluginkit back as they were.
- **FSKit then named another copy of the module.** Before the build, it named `/Applications`'s;
  afterwards, the compassionate-payne worktree's, three times over 30 s. The switch stayed on. The
  first slice's runs saw the same drift, and an `lsregister -f` to steer it did harm then, so it
  was left for the user.
- The app's job was never loaded, and no defaults were written.

## The live run

Approved by the user, run on 17 October from 15:33 to 15:38 (local time), from the signed
Release build of `68c5296`. Its outputs are in [raw/live](raw/live/), with the daemon's whole log
in [daemon.log](raw/live/daemon.log).

**The rig.**
- Two `voidfs-server --store memory` on `127.0.0.1:9372` and `:9373`, each with its own throwaway
  key from `void keys generate`.
- A state directory under `/private/tmp/vli.…` (`$D`), set as the app's `VoidfsStateDirectory`
  before anything was registered.
- The bridge agent was registered and enabled, so Start at login left it alone.
- Snapshots: [before](raw/live/before.txt) and [after](raw/live/after.txt).

**What happened**, from the build's `voidfs` and bundled `void`:

1. **Not signed in** ([output](raw/live/01-not-signed-in.txt)). `login-item on` said "Sign in to
   your voidfs server first", registered nothing and exited 1.
2. **A terminal daemon** ([output](raw/live/02-terminal-daemon.txt)). `void daemon start`, with
   the first server and key in its environment, wrote `daemon.json` (mode 600) and started a
   daemon. `server-key status` said the terminal had kept a key there.
3. **Start at login, first try: refused by a launch constraint**
   ([output](raw/live/03-login-item-on.txt), [launchd](raw/live/03-launchd-log.txt),
   [requirement](raw/live/03-lwcr.txt)).
   - `login-item on` registered the job, but launchd killed `voidfs` at once: "Launch Constraint
     Violation … (Constraint not matched) … failure proc [vc: 3]". The daemon didn't answer within
     20 s.
   - The job's lightweight code requirement (LWCR) still named `signing-identifier
     dev.voidfs.void`, the program of the first slice's job, registered in that slice's live run
     and unregistered after it. Background Task Management had kept the item (`4C5730D7…`) and
     its requirement, so the new job, which runs `voidfs` (`dev.voidfs.app`), was refused.
   - launchd's own repair failed ("Unable to get updated LWCR … Invalid argument"). After that
     each run said "Could not find and/or execute program", exit code 78 (EX_CONFIG), every 10 s.
4. **Unregistered and registered again** ([output](raw/live/03b-reregister.txt)). `login-item off`
   then `on` made a new item (`0104A10B…`) whose requirement names `dev.voidfs.app`. The daemon
   answered at once.
5. **The handover** ([output](raw/live/03c-handover.txt)).
   - launchd runs `voidfs daemon-launch` (pid 80649). Its child is `void daemon run --app --logged
     --endpoint http://127.0.0.1:9372 --key-file /dev/stdin` (pid 80650).
   - The log has "moved the server key from daemon.json into the Keychain, and deleted the file",
     then the terminal's daemon stopping, and the app's daemon starting on the first server.
   - `daemon.json` is gone. `server-key status` and `daemon status` name the first server and
     key, and `daemon status` names `dev.voidfs.app.daemon` as the login job.
   - Neither process has the secret in its arguments or environment (`ps -E -ww`), nor does the
     log.
6. **Signing in again restarts the daemon** ([output](raw/live/04-sign-in-restarts.txt)).
   `server-key sign-in` to the second server with the second key said "The daemon (pid 80822)
   runs with it". The job ran again (run 2) within 0.3 s, on the second server, with the
   second key.
7. **The CLI while the app has it** ([output](raw/live/05-cli-stop-start-crash.txt)).
   - `void daemon stop` stopped it, and 15 s later launchd hadn't run it again (last exit code 0).
   - `void daemon start` from an empty environment kickstarted the job. `void daemon restart`,
     with the first key in its environment, restarted it on the app's key, the second. Neither
     wrote `daemon.json`.
   - After `kill -9` of the daemon, the launcher logged "the daemon (pid 80983) was killed by
     signal 9" and died by the same signal. launchd ran the job again, its last terminating
     signal "Killed: 9".
   - The restart and the crash each waited 10 s. A run shorter than the job's 10 s minimum
     runtime makes launchd wait before the next one; the first slice's run saw 1.1 s because
     its daemon had run longer.
8. **Signing out stops it** ([output](raw/live/06-sign-out.txt)). `server-key sign-out` said
   "Signed out: the daemon (pid 81502) stopped, and starts again once you sign in". launchd left
   the job stopped (last exit code 0). Then `void daemon start`, with a key in its environment,
   failed in 0.1 s with "voidfs.app has no server key for the daemon: sign in from its menu",
   quoting the launcher's log line. No `daemon.json` was written.
9. **Turning it off** ([output](raw/live/07-login-item-off.txt)). `login-item off` unregistered
   the job, and `launchctl print` no longer finds it.

**Restored** ([output](raw/live/08-restore.txt)). In order: the default deleted; the servers
stopped; the state directory removed (its Keychain item had gone at the sign-out); the build's
copy unregistered from LaunchServices (pluginkit had no plug-ins from it this time). The snapshot
after matches the one before. No process from the run was left.

**What it means.** BTM keeps a job's launch requirement across unregistering and registering
the same label once. A version of the app whose job runs a program with another signing
identifier is then refused until the job is registered a second time. The bridge agent did the
same on 9 October when switching between Developer ID and development copies
([the Swift bridge](../../docs/step-5-macos.md#the-swift-bridge-9-october), findings). The first
slice's job never shipped, but an update that changes a job's program, or a switch between
differently signed copies, would hit it.

## Registering again

After the live run, at the user's request, Start at login notices that refusal and registers the
job once more (`apps/macos/App/LoginItem.swift`):

- **The check** reads `launchctl print gui/<uid>/dev.voidfs.app.daemon`. launchd refuses the job
  if its LWCR block names another `signing-identifier` than this binary's own (the job runs this
  binary), or if its last exit code is 78 (EX_CONFIG). Neither `void` nor the launcher exits 78.
- **Turning it on** checks while it waits for the daemon, about once a second. On a refusal it
  unregisters and registers the job, once, and waits again. Its note says why, for example
  "launchd refused the job (its launch requirement names dev.voidfs.void, not dev.voidfs.app), so
  it was registered again."
- **Turning it on while it is on**, and **opening the menu**, check too. The menu checks once per
  run of the app, for a job left on across an update.
- `login-item status` says when launchd refuses the job.

`voidfs login-item selftest` ([output](raw/login-item-selftest.txt)) passed all 15 checks on the
signed Release build:
- the check against `launchctl print` as it was in the live run: the kept requirement (with and
  without exit code 78), the job's own requirement, exit code 78 alone, clean and failed exits, a
  signing identifier outside the requirement's block, and an unsigned binary;
- this binary's signing identifier, `dev.voidfs.app`;
- the wait, with stand-ins for the daemon and launchd: refused then answering; refused still (once
  only, the wait starting over); not refused, past the old daemon; registering again failing; and
  the note.

Three breaks each failed it, run as a plain `swiftc` build of the app's sources, whose own
identifier check fails by design ([output](raw/login-item-breaks.txt)). The file was restored byte
for byte afterwards.
- With the identifier comparison removed, the kept requirement isn't named, and without exit 78 isn't caught.
- With registration repeated on every refusal, a refusal that stays makes it register twice.
- With the requirement's block never ending, an identifier after it counts.

The fix wasn't run against a real kept requirement: that needs a job registered with another
program first. Its build registered the copy with LaunchServices and pluginkit, and unregistering
it put both back ([snapshots](raw/registration.txt)).

## Not covered

- **The Developer ID export**: it needs the "voidfs App Developer ID" profile, then notarizing.
- **Logout and login, and a reboot**: the key read by the job at login, and before the first
  unlock.
- **Registering again against a real kept requirement**, rather than recorded `launchctl print`
  output and stand-ins.
- **The form in the running menu**, and its Import key file panel.
