# Logout, login and a reboot, 18 October 2026

Step 5, item 7's done-when, first half: the app brings remembered mounts back after a logout and
login and after a reboot ([item 7](../../docs/step-5-macos.md#item-7-volumes-launch-services-and-credentials),
[the check](../../docs/step-5-macos.md#logout-login-and-a-reboot-18-october)). macOS 27.0.1
(26A434), Xcode 27.0 (27A5194q), Apple M5 Pro. The app is the notarized Developer ID export of
`main` at `2986b57` ([its notarization](../item7-keychain/README.md#the-developer-id-export-and-notarization)).

The user approved this run: install the export in `/Applications`, register it, turn on Start at
login with a remembered mount, and log out and in and reboot. The user did the logout, the
reboot and FSKit's switch. Everything else ran from the terminal. Afterwards the user chose to
keep the notarized build in `/Applications` and to leave the other copies unregistered.

## The rig

Everything lived in `~/.voidfs-login-rig`, outside the scratchpad, which a reboot wipes. The
scripts are in [raw/rig](raw/rig):
- `voidfs-server` on 127.0.0.1:9372 with a filesystem store, `multi-object-versions` and a
  throwaway admin key.
- The app's `VoidfsStateDirectory` set to the rig's state directory.
- A drive `login-test` with four files (3 MiB in all) uploaded by "another client": the CLI in its
  own state directory with no daemon, so `void upload` runs in the foreground.
- `check.sh` reads and starts nothing: when, both launchd jobs, `void status` and `mounts`, the
  mount table, FSKit, the listing and hashes through the mount, the processes, and the daemon
  log. `online.sh` starts the server, times the link coming online, then uploads a new file from
  the other client and times its appearance in the mount.

Nothing touched `~/Library/Application Support/voidfs`, which was absent before and after
([before](raw/before.txt), [after](raw/after.txt)). No key was in any argument, status output or
log. The job got it from the Keychain and passed it to `void` on stdin (`--key-file /dev/stdin`).

## Installing and registering

[install.txt](raw/install.txt), [registration.txt](raw/registration.txt), [agent.txt](raw/agent.txt)

- The export replaced the 9 October copy, which was kept in the rig. Installed, `spctl` says
  "Notarized Developer ID", the staple validates, and `codesign --verify --deep --strict` passes.
- `/Applications` was registered. The five other copies were unregistered: the main checkout's
  build and archive, the compassionate-payne and sleepy-germain worktree builds, and the
  notarized export in the scratchpad. FSKit then listed no voidfs module until the plug-ins were
  added; the user turned the switch on.
- **The bridge agent was refused, and a second registration fixed it.** `dev.voidfs.agent` kept
  a development launch requirement (`validation-category` 3) and ended with exit code 78. One
  `agent-unregister`/`agent-register` kept it. A second one made it 6, and the agent ran. This
  explains the `/Applications` agent refusals since 15 October. The app didn't yet
  re-register the agent by itself, as it does the daemon job. It now does
  ([the fix](../item7-agent-requirement/README.md)).

## Start at login

[login-item-on.txt](raw/login-item-on.txt)

After signing in with the throwaway key, **`login-item on` met a real kept requirement and
recovered**, as #84 intended: launchd refused the job with exit 78, the app registered it again,
and the job ran with `validation-category` 6. It took 11 s, from 10:26:43 to 10:26:54. The
self-test had only stood in for this case before.

## The remembered mount, before the logout

[setup.txt](raw/setup.txt), [mount.txt](raw/mount.txt), [0-before-logout](raw/0-before-logout)

- **The first `void mount` failed** with an input/output error. The module's log said the daemon
  wasn't running: the agent had started before `VoidfsStateDirectory` was set, and it reads the
  default only at launch. After `launchctl kickstart -k` on the agent, the mount succeeded:
  `voidfs://login-test` on `~/voidfs/login-test`, through FSKit, remembered.
- All four files read through the mount with their hashes, so this Mac cached them. A file from
  the other client appeared 0.03 s after its upload.
- **A save while the server went away.** The server, sent SIGTERM, closed its listener but waited
  on the daemon's open connection, so the link still showed online. The file
  `written-offline-before-logout.txt` was then saved through the mount. Its upload was refused
  ("Connection refused") and marked failed, to be tried again: the queue retries network
  failures after 2, 4 … 60 s. The server was then killed, and the link went offline.

## Logout and login

[1-after-login](raw/1-after-login). The user logged out at 10:30:18, back in at about 10:30:25,
and didn't open the app.

- At the logout the daemon got SIGTERM and stopped in 47 ms.
- The agent started at 10:30:26. **The login job started at 10:30:36, about 11 s after the
  login, and launchd chose that, not the app.** It took the job at 10:30:25 but held it, "pending
  spawn, domain in on-demand-only mode", and then spawned it "because speculative". In those
  13 s it held 48 jobs that way and spawned 12 "because speculative", this one among them
  ([launchd-login.txt](raw/1-after-login/launchd-login.txt)).
- `voidfs daemon-launch` read the key from the Keychain and started `void`, which logged its
  start at 10:30:37. It restored the mount at once, offline, with the server still down.
- Offline, every file read through the mount with its hash, including the saved file.
- The server started: the link came online after 14.8 s, because the offline probe had backed off
  to its 30 s ceiling. A new file from the other client appeared in 0.03 s.
- **Bug: the failed save was never tried again.** It was still failed 121 s after the link came
  online ([uploads-after-online.txt](raw/1-after-login/uploads-after-online.txt)).
  `void uploads resume 1` published it at once, and the other client read the same hash
  ([resume.txt](raw/1-after-login/resume.txt)). The queue keeps a failed entry's retry time in
  memory only (`retry_at` in `crates/voidfs-client/src/queue.rs`). It skips a failed entry that
  has none, so after any restart a network failure waits for a resume, like a `409`.

Before the reboot, the server was stopped again and the daemon saw the link go offline. Then
`written-offline-before-reboot.txt` was saved through the mount. Its upload stayed queued,
not failed ([offline-write-before-reboot.txt](raw/1-after-login/offline-write-before-reboot.txt)).

## A reboot

[2-after-reboot](raw/2-after-reboot). The user restarted the Mac: the daemon got SIGTERM at
10:39:14 and stopped in 18 ms, and it booted at 10:39:32. The login window started at 10:39:46,
the agent at 10:39:50 with the login, the job at 10:40:02, and the module at 10:40:06. `void`
logged its start at 10:40:05. The app wasn't opened.

- **The mount came back**, offline and remembered, with the job and the agent both from
  `/Applications` (`signing-identifier dev.voidfs.app`).
- Offline, every file read with its hash except one: **`written-offline-before-logout.txt` failed
  with "Network is down"** ([reads-and-uploads.txt](raw/2-after-reboot/reads-and-uploads.txt)).
  That's the rule for bytes that aren't cached. But this Mac wrote them: once the upload landed,
  publication dropped the staged copy, and nothing put those bytes in the read cache. The cache
  holds the six files read through the mount, and not this one. Before the reboot the file still
  read, though neither of the daemon's caches counted it. The kernel or the old daemon probably
  still held its pages. (`check.sh`'s listing shows it as the empty
  file's hash, because `shasum` hid the read error. The file next to it has the real error.)
- **The queued save failed at the daemon's start.** The journal, as checkpointed at the shutdown,
  had it queued. It was failed with "Connection refused" soon after the start, before the daemon
  knew the link was offline. In this process the failure kept its retry time.
- The server started: the link came online after 3.0 s. The save published by itself, without a
  resume, before it was checked 36 s later. The other client and the mount read both saves' hashes
  ([uploads-after-online.txt](raw/2-after-reboot/uploads-after-online.txt)).
- **The first change from the other client stayed missing by name.** `online.sh` looked the name
  up every 50 ms, and the mount's first answers were "no such file". The folder listing had the
  name by 10:43:28, but lookups still failed after that: FSKit kept the missed name at the root,
  [item 3's blocker](../../docs/step-5-macos.md#the-blocker-a-name-missed-while-its-folder-is-in-use).
  The first answer was "no such file", where the same lookups found the file within 0.03 s
  before the logout and after the login. The cause isn't confirmed. It may be that the restored
  folder was served as complete before the change feed caught up, after the first online since
  the start ([feed-after-online.txt](raw/2-after-reboot/feed-after-online.txt)).
- Polled by listing instead, a second change appeared in 0.03 s, and so did one after a further
  60 s offline ([online-listing-poll.txt](raw/2-after-reboot/online-listing-poll.txt),
  [reconnect-cycle.txt](raw/2-after-reboot/reconnect-cycle.txt)).

## Restored

[restore.txt](raw/restore.txt), [after.txt](raw/after.txt)

- `void unmount login-test` unmounted and forgot it.
- `voidfs server-key sign-out` removed the throwaway key and stopped the daemon.
- `voidfs login-item off` removed the job.
- `VoidfsStateDirectory` was deleted, and the agent restarted so that it reads the default again.
- The test server was stopped.
- The user kept the notarized build in `/Applications`, and left the other copies unregistered.
  The reboot had put the main checkout's build back in LaunchServices, so it was unregistered
  again, with two records whose scratchpad paths were gone.
- Only `/Applications` is registered now. FSKit's switch is on for its module, and Finder Sync
  uses its extension.
- The agent runs. Before the run, the kept requirement refused it.
- The rig, the 9 October copy and `~/voidfs/login-test` were removed.

## Not covered

- **Login Items' approval flow**: SMAppService enabled the job at once, with nothing to approve.
- **A login before the keychain is unlocked.** The job is a GUI-session agent, so both times it
  ran after the login had unlocked the keychain.
- **The menu at login.** Start at login registers the daemon, not the menu app. The app wasn't
  open after either login, so its view of the restored mount wasn't seen.
- **Eject of a busy drive, and the `/Volumes` helper**: the done-when's other two parts.
- **The form in the running menu, and Import key file…**
