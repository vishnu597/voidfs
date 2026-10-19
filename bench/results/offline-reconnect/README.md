# A saved listing can overtake adapter invalidation, 19 October 2026

Investigation of the Oct 19 handoff's first-online missed name, based on `main` at `1b5df16`.
The handoff's suggested stale-offline-listing fault did not reproduce. A separate notification
race did: a refreshed core snapshot could hide a foreign change from mount observers.

## What the tests establish

`voidfs-client/tests/mount.rs` opens a persistent namespace online, records root and nested
listings, closes it, then reopens the store and session offline. Cached names and negative
lookups require no requests. On the first online access, both directories refresh and find
names committed after the link comes online, before any feed invalidation is delivered. Names
committed after that refresh may be absent until the delayed feed arrives; delivering it clears
those misses immediately. Offline access does not mark old snapshots fresh for online use.

`mount_publication.rs` forces a directory refresh to overtake each foreign create, overwrite,
rename and delete event, at the root and in a nested directory. Before this change, the create
event returned no observer invalidations because the inode already recorded the remote version
([original-code failure](raw/before.txt)). Matching the database is not evidence that a mount
adapter discarded its older positive or negative lookup.

The fix reports foreign changes to saved or absent names, even if a refresh already learned them.
The core still invalidates its metadata in the same transaction. Its own acknowledged
publications and changes hidden by unpublished local edits retain their suppression; changes
beneath a locally moved folder still report their old remote paths. Echo suppression uses the
existing bounded, session-local publication history, so replay after restart or history eviction
can send extra harmless invalidations.

`voidfs-daemon/tests/remote_events.rs` uses a real daemon, a fake adapter, and a proxy that can
refuse connections and gate only change-feed bodies. It restores a remembered mount offline,
serves its saved namespace, reconnects, creates a remote name, lets a lookup refresh the core,
then releases the delayed feed. The adapter must still receive the creation invalidation.
It fails that assertion on the original `main` implementation after consuming the event
([failure](raw/daemon-before.txt)), and passes with the fix
([result](raw/daemon-test.txt)).

## Validation

- [Targeted client suites](raw/client-tests.txt): 64 passed across `mount`, `mount_publication`,
  `mount_seeding` and `queue`. This includes #88's retry after restart and initially unknown-link
  queue behavior, and #89's offline reads of a published save after restart.
- [Daemon integration](raw/daemon-test.txt): the remembered-mount regression passed.
- [Full workspace tests](raw/workspace-tests.txt): 779 passed, 9 ignored, no failures, including
  the daemon's notification and own-publication checks and the new regression.
- [Clippy](raw/clippy.txt): `--workspace --all-targets --locked -- -D warnings` passed.
- Spec conformance validation passed (56 cases, 432 steps), as did all five credential-script
  tests ([commands and results](raw/validation-commands.txt)).

All Rust tests use disposable local state and local test servers. No installed app, production
state directory, credentials, service registrations or FSKit switches were changed.

## Handoff progress and remaining work

- [x] Investigate the restored offline namespace without FSKit.
- [x] Fix the reproducible pre-feed-refresh notification gap and add regression coverage.
- [ ] Verify the original missed-name symptom on a live FSKit mount. The fake adapter does not
  model FSKit's pinned negative lookups, so these tests do not prove the cause of the reboot
  observation or remove the existing FSKit blocker.
- [ ] Build, notarize, install and recheck #88–#90 live. The release script combines archive,
  export, notarization and stapling; stapling can register its app copy. An attempt to inspect
  an unsigned build plan failed because this Xcode 27 no longer supports `-dry-run`
  ([command and toolchain](raw/xcode-commands.txt), [diagnostic](raw/xcode-dry-run.txt)). No actual
  Xcode build ran; this preflight does not validate #90 in Xcode.
- [ ] Continue the handoff's decisions about `/Volumes` and busy eject, RFC 0006 limits, shard
  integrity, hardware and paid services, Apple Feedback, menu login behavior and housekeeping.

Before a live recheck, copy the rig outside temporary storage. The historical
`item7-login/raw/rig/online.sh` polls the filename with `-f`, which can prime FSKit's negative
lookup: poll the containing directory listing instead. Its hash checks should also use a checked
`cat` piped to `shasum` under `pipefail`, so a mount read error cannot be mistaken for valid bytes.
The historical rig is retained as evidence of the original run.
