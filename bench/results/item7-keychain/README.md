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
same self-test and sign-ins.

Nothing here registered the job with SMAppService, wrote defaults, or touched
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

## Not covered

- **The Developer ID export**: it needs the "voidfs App Developer ID" profile, then notarizing.
- **The live run**: the job registered with SMAppService, launchd running `voidfs daemon-launch`,
  a sign-in that restarts the daemon, a sign-out that stops it, and a key read before the first
  unlock after a reboot.
- **The form in the running menu**, and its Import key file panel.
