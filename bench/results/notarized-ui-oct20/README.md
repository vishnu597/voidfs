# Notarized update and native UI checks, 20 October 2026

**Native checks, saved-byte recovery and disposable-fixture restoration passed.**
The guarded export is installed. Native import/sign-in, the Login Items approval
warning and recovery, Force Eject's Cancel/Confirm controls, stale-dialog dismissal
and exact saved-byte recovery were verified.

## Initial update of main

The user authorized building, notarizing and installing current main, including
PRs #92 and #93, then checking the native Force Eject, sign-in/import and Login
Items flows. The starting source was `2ba2c4b984baa4284fc930d1f3769e56d9ec7f31`.

`apps/macos/scripts/release.sh` archived/exported that main revision, and Apple
accepted notarization submission `6d040353-3fd6-4253-a37c-b7a0fc450424`. The staple,
signature and Gatekeeper checks passed ([release](raw/release.txt)); the export
passed `syspolicy_check distribution` ([export checks](raw/export-checks.txt)).
The installed CLI subsequently reported `2ba2c4b`, dated 20 October, and the
installed app passed its distribution checks ([installation](raw/install.txt)).

The initial install log still resolved FSKit to an archive copy. A subsequent
archive-unregistration attempt reported LaunchServices error `-10814` and exited
with an exception ([cleanup attempt](raw/registration-cleanup.txt)). The later
guarded update has a fresh extension snapshot resolving enabled FSKit to the
installed copy, as recorded below. The final plug-in inventory contains one
FSKit record and one Finder Sync record, both under the installed app.

## Guarded export and installation

Revision `45c449f` was subsequently archived/exported, and Apple accepted
notarization submission `c00229aa-8659-4b1e-92ca-c2d59d6c2689`. Signature,
Gatekeeper, staple and distribution checks passed, and the installed CLI
reported the guarded revision ([guarded release](raw/release-guard.txt),
[guarded installation](raw/install-guard.txt)).

The fresh [extension snapshot](raw/extensions-guard.txt) shows enabled FSKit at
`/Applications/voidfs.app/Contents/Extensions/VoidfsFS.appex`. The installation log
also lists both plug-ins at their installed paths. The [final snapshot](raw/final.json)
confirms one registered record for each plug-in, both at those installed paths.

The private Keychain sign-in survived this update: endpoint, key and state-directory
checks remained true, with no `daemon.json`
([account after update](raw/sign-in-after-update.txt)).

## Initial baseline and private fixture

The [initial snapshot](raw/before.json) records macOS 27.0.1 (26A434), installed
revision `f5f6bd1`, the bridge enabled, Start at login off, no mount, no
`VoidfsStateDirectory` override, no default app state directory and no saved
sign-in for that default account.

The private fixture is `~/.voidfs-ui-rig-oct20`, with drive `ui-check-oct20`.
Its local backend on port 9373 and proxy on 9372 use throwaway credentials kept
outside this repository. The peer has separate daemon-free state. The backend
comes from the earlier external-eject target, with its source/hash and the
installed CLI version recorded in [rig.jsonl](raw/rig.jsonl).

The bridge was explicitly redirected to the private state directory
([configure](raw/configure.txt)). A [read-only health check](raw/health-draft.txt)
found that private account signed out, Start at login off, no `daemon.json`, and
no running private daemon. Both listeners and signed read-only drive queries
through backend and proxy succeeded; the installed CLI still reported `2ba2c4b`
([server health](raw/health-server-draft.json)). These are intermediate checks,
not a final restoration snapshot.

## Native sign-in and Login Items

The [native observation notes](raw/native-ui-observations.md) summarize the
operator's accessibility observations. They are narrative notes, not raw tool
output or screenshots; the linked helper logs independently check state and bytes.

On installed `2ba2c4b`, the operator opened the actual status menu and inspected
its accessibility state. It showed signed out, with the unchecked Start at login
control disabled and help saying to sign in first. **Sign in…** opened the server,
region, access-key-id and secure secret fields. **Cancel** returned to signed out.

The operator reopened sign-in, used **Import key file…**, selected the private
`key.env` fixture in the native file panel and clicked **Open**. Both the file
panel and menu panel closed initially. Reopening the menu retained the form and
showed “Read the key from key.env.” The operator then clicked **Sign in**. The
actual menu showed signed in to the local endpoint and enabled the login checkbox.
The helper confirmed the expected endpoint, key and private state-directory
account, with no credential file ([native sign-in](raw/native-sign-in.txt)).

The env fixture includes the endpoint/region as well as key fields. `key.json`
contains only key fields and does not fill the server by itself. Credential
values are not recorded here.

On installed `45c449f`, turning on the actual menu's Start at login control started
the private daemon as PID 47203. Its reported commit was `45c449f`, the link was
online, and the app reported the login item on; no `daemon.json` was present
([native login on](raw/native-login-on.txt)).

In System Settings → General → Login Items, the voidfs permission switch was
initially on. The operator switched it off. The app then reported
`needs approval in System Settings → General → Login Items & Extensions`
([disabled permission](raw/native-login-needs-approval.txt)). Reopening the actual
menu showed the checkbox checked, “Waiting for your approval in Login Items” and an
**Open Login Items settings** button. Clicking that button opened the actual
Login Items page. The operator restored the voidfs switch to on. The app reported
the login item on again, and the private daemon was ready at the expected endpoint
([restored permission](raw/native-login-restored.txt)).

## Native busy eject and confirmation

The installed Swift stale-eject self-test passed: old mount JSON still decodes,
force without an instance is refused locally, and a stale force request leaves
the current attachment mounted ([installed self-test](raw/eject-stale.txt)).

With the proxy offline and a writer still open after an acknowledged 262,177-byte
save, the actual menu's **Unmount** reported Resource busy and retained the row.
**More actions → Force Eject…** opened a native alert naming the drive and path,
warning about unsaved application changes and explaining that queued uploads stay
saved on this Mac.

The operator clicked **Cancel**. The alert closed and the mounted row remained.
The helper verified the same kernel device, live and remembered intent, all
262,177 bytes and the still-open writer ([Cancel check](raw/native-force-cancel.txt)).

The operator opened the dialog again and confirmed force eject. The menu row
disappeared. Without issuing another eject, the helper found the OS mount gone,
both mount tables empty and the same queued journal entry retained, while the
writer was still held ([Confirm check](raw/native-force-confirm.txt)). This was
already true at the first observation; it does not measure click-to-eject latency.
The operator then released the writer; it observed `EIO` on the old descriptor
and exited successfully ([held writer](raw/native-force-holder.txt)).

The [recovery check](raw/native-force-recovery.txt) passed. A restart with forgotten
intent left both mount tables empty for 19 samples over two seconds, while the
same save stayed queued. An explicit offline remount read all 262,177 bytes on a
new kernel device. An intentional daemon restart then restored that remembered
mount on another new device and read the same bytes. Reconnecting published the
same entry without resume; an independent peer downloaded identical bytes and
SHA-256. The check ended with a successful ordinary eject and no live or remembered
mounts. This is a bounded no-remount observation, not an unlimited one.

The operator then mounted again and opened the actual force alert. A separate
ordinary CLI unmount/remount replaced the attachment at the same path, with a
different instance token ([replacement record in rig.jsonl](raw/rig.jsonl),
[operator marker](raw/native-identity-replacement.txt)). At the next accessibility
observation, the alert had dismissed automatically and the replacement's mounted
row remained; no Confirm was clicked. Native ordinary Unmount then removed that
row. This observes stale-dialog invalidation; the installed self-test and daemon
regressions separately exercise rejection of a stale request.

## Disposable-fixture cleanup

The operator turned Start at login off in the native menu, observed its unchecked
control and daemon-stopped message, then clicked Sign out. The menu showed signed
out with the login checkbox disabled, and the operator quit the app. The helper's
[pre-cleanup status](raw/before-cleanup.txt) confirmed signed out, login off, no
credential file and no private daemon.

[Cleanup](raw/cleanup.txt) exited successfully. The original account status was
preserved, the private rig was removed, the previous absent state-directory
override and login-off state were restored, and both local services stopped.
The journal's sole entry was `done`; no unpublished data remained. A scan of
56 evidence files against the known private key values found no leaked files
([cleanup events](raw/rig.jsonl)).

The [final machine snapshot](raw/final.json) confirms installed `45c449f` with a
valid signature, enabled FSKit and bridge, and one installed-path record each for
FSKit and Finder Sync. The default account is signed out, Start at login is off,
the state-directory override and default state directory are absent, and there
are no mounts. The rig is absent and both loopback ports are closed. The native
System Settings permission switch was restored to its original on state before
the login service was turned off.

## Guard work and completed focused tests

The confirmation guard gives each mounted attachment an instance token
and sends the dialog's expected instance with eject requests. A stale instance,
including after replacement at the same path or daemon restart, must be refused.
The menu also refuses force eject against a daemon that cannot supply that
identity. This guarded code is a change after the first `2ba2c4b` notarization;
the separate `45c449f` accepted export above is the guarded installed binary.

Completed checks of the current guard work:

- [Daemon/FSKit integration](raw/daemon-instance-tests.txt): 20 passed, including
  stale/replaced instances, prior daemon identity, multi-attachment rejection,
  backward payload compatibility and existing external-eject behavior.
- [CLI mount compatibility](raw/cli-mount-compatibility-tests.txt): 6 passed,
  including explicit-force wire requests and no fallback.
- [Spec validation](raw/conformance-validate.txt): 56 cases, 432 steps passed.
- [Credential-script checks](raw/credentials-scripts.txt): all 5 passed.

The final guarded Xcode archive also builds the Swift guard sources. The
[workspace run](raw/workspace-tests.txt) passed 800 tests, with 9 ignored. It
preceded the last failed-attachment normal-cleanup fix; the entire daemon package
was rerun on final production source (83 passed, 1 ignored), then its assertion-only
Clippy correction received a focused rerun. Strict workspace Clippy passed.
Four memory/filesystem, path/virtual protocol runs each passed 46 cases, with
10 documented capability skips ([validation summary](raw/validation-summary.json)).

## Completion evidence

- [x] Archive/export, notarize and install guarded `45c449f`, with submission and
  active installed extension recorded; verify final installed plug-in inventories.
- [x] Reopen the imported form, submit native Sign in and confirm the expected
  private Keychain account without a credential file; verify it survives update.
- [x] Observe the actual Force Eject dialog's Cancel and Confirm actions with a
  held offline writer, verifying mount retention/removal and the saved journal entry.
- [x] Close out exact saved-byte recovery through offline remount/restart and
  later independent peer publication.
- [x] Observe native login enabled/startup, then disable its System Settings
  permission and verify the app's actual needs-approval state.
- [x] Reopen the menu to record its native approval warning, restore the original
  permission switch and verify recovery.
- [x] Verify automatic native alert dismissal after replacing the attachment at
  the same path, with the replacement left mounted and no Confirm clicked.
- [x] Restore the private Keychain/login/default state, stop the local fixture,
  verify no unpublished data remains and scan evidence.
- [x] Record final machine and registration inventory.

The [operator scripts](raw/scripts/README.md) separate native UI actions from
status/byte checks. Cleanup refuses unfinished data and held writers. Native
import/sign-in, approval-warning/recovery and native force controls are verified;
saved-byte recovery, fixture cleanup and final inventory passed. This run did not
repeat logout/reboot, test a clean-Mac install, test another Mac, or cover unsaved
application buffers and unsynchronized mmap stores. It does not complete item 7
or remove the earlier FSKit and application limitations.
