# Native UI observation notes, 20 October 2026

These are the operator's notes from native accessibility observations supplied
during this run. They are not raw tool output or screenshots. Credential values
are omitted. Helper logs and `rig.jsonl` separately record private comparisons,
mount identity, journal state and byte checks.

Codex drove the native controls through CUA. The user reopened the status-menu
panel when the UI tool could not attach to the windowless menu-bar app.

## Import and Sign in, installed `2ba2c4b`

- Opening the status menu showed signed out. Start at login was unchecked and
  disabled, with help saying to sign in first.
- **Sign in…** opened server, region, access-key-id and secure secret fields.
  **Cancel** returned to signed out.
- **Import key file…** opened the native file panel. Selecting the private
  `key.env` fixture and clicking **Open** closed the file panel and menu panel.
- Reopening the menu retained the form and showed “Read the key from key.env.”
  Clicking **Sign in** showed signed in to the local endpoint and enabled Start
  at login. [Private verification](native-sign-in.txt) passed without recording
  credentials or creating `daemon.json`.

## Login Items, installed `45c449f`

- Turning on Start at login in the actual menu started the private daemon
  ([helper](native-login-on.txt)).
- System Settings → General → Login Items initially showed the voidfs permission
  switch on. The operator switched it off; the app reported `needsApproval`
  ([helper](native-login-needs-approval.txt)).
- Reopening the menu showed Start at login checked, “Waiting for your approval in
  Login Items” and **Open Login Items settings**. Clicking that button opened the
  actual Login Items page.
- Restoring the voidfs permission switch to on was visibly confirmed. The helper
  then reported the service on and the private daemon ready
  ([helper](native-login-restored.txt)).

## Busy Unmount and Force Eject, installed `45c449f`

- While a writer held an acknowledged offline save open, native **Unmount**
  reported Resource busy and kept the mounted row.
- **More actions → Force Eject…** opened a native alert naming the drive and path,
  warning about unsaved application changes and explaining that queued uploads
  remain saved on this Mac.
- Clicking **Cancel** dismissed the alert and retained the mounted row. The
  [helper](native-force-cancel.txt) confirmed the same kernel device, remembered
  intent, still-open writer and exact 262,177 bytes.
- Opening Force Eject again and confirming removed the row. The
  [helper](native-force-confirm.txt) found the OS mount gone, both tables empty
  and the saved entry still queued while the writer remained held. No GUI
  click-to-eject latency was measured. [Subsequent recovery](native-force-recovery.txt)
  independently passed offline remount/restart and peer-byte checks.

## Native stale-dialog dismissal

- After another mount, the operator opened the actual Force Eject alert.
- A separate ordinary CLI unmount/remount replaced the attachment at the same
  path. `rig.jsonl` records the different old and new instance tokens.
- At the next native accessibility observation, the alert was automatically
  gone and the replacement's mounted row remained. No Confirm was clicked.
- Native ordinary **Unmount** then removed that row. This checks automatic
  dialog invalidation; it does not manufacture a click race after replacement.

## Native shutdown of the private session

- The operator turned Start at login off. Its checkbox was unchecked, and the
  menu said the daemon stopped.
- Clicking **Sign out** showed signed out and disabled Start at login. The
  operator quit the app.
- The [pre-cleanup status](before-cleanup.txt) and [cleanup](cleanup.txt) separately
  confirmed signed out, login off, no private daemon, original account preserved
  and private rig removed. The System Settings permission switch had already
  been restored to its original on state.
