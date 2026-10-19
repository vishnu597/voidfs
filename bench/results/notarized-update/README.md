# Notarized update and FSKit verification, 19 October 2026

The Developer ID export of `f5f6bd14344ea026d74390154f23c57d0dc95944` (PR #91) replaced the
notarized `2986b57` install in `/Applications/voidfs.app`. This checks the handoff's short live
recheck of #88–#90 and the new foreign-notification fix. macOS 27.0.1 (26A434), Xcode 27.0
(27A5194q), one Mac. The user authorized notarization, installation and the live verification.
Times in raw JSON are UTC; the run was approximately 12:53–13:08 America/Toronto.

## Release and installation

- `apps/macos/scripts/release.sh` archived the full Xcode app, exported with its app and FSKit
  Developer ID profiles, notarized and stapled it ([release](raw/release.txt)). Apple accepted
  submission `d50e3c83-a70c-450a-ac40-1e0f9351aab5`.
- Signature, Gatekeeper, staple and `syspolicy_check distribution` passed on both export and
  install. The bundled `void` is Developer ID signed as `dev.voidfs.void` and reports `f5f6bd1`
  ([export checks](raw/export-checks.txt), [installed checks](raw/install-retry.txt)).
- macOS refused the first in-place copy despite writable ownership
  ([initial attempt](raw/install.txt)). The old install still verified; its temporarily removed
  bridge was restored. After the user chose to enable Codex App Management, the retry succeeded.
- The installation kept FSKit enabled. Archive intermediate and exported-copy registrations
  were removed; the final inventory lists only `/Applications`
  ([before](raw/registrations-before.txt), [after](raw/after.txt)). The initial sandboxed
  [inspection](raw/before.txt) could not resolve service/signing authority; the escalated
  registration and release checks supersede those sandbox results.
- Installed `agent-selftest` and `login-item selftest` passed all 21 and 23 checks, respectively
  ([agent](raw/agent-selftest.txt), [login item](raw/login-item-selftest.txt)). Both live jobs ran
  without refusal and with validation category 6, from the installed bundle
  ([live services](raw/live-services.txt)). A kept requirement was not deliberately introduced.

## Disposable rig and results

The rig lived in `~/.voidfs-update-rig-oct19`, outside temporary storage. Its app state, peer CLI
state, key, filesystem pool and mountpoint were separate. The login daemon used a temporary
`VoidfsStateDirectory` and a throwaway Keychain item. No real bucket, paid service or production
state was used. The server ran at localhost:9373, and the daemon used a localhost:9372 proxy that
could return mutation-only `503`s or gate feed bodies while forwarding signed requests unchanged.
The peer CLI used the backend and its own daemon-free state directory. Keys stayed in a private
file or child environment and were redacted from saved logs.

| Check | Observed result | Evidence |
|---|---|---|
| #88 transient retry after restart | A 131,111-byte FSKit save failed with `503`, entry 2 recorded `retry_at=1792429424372`, and the same entry/deadline survived restart. After removing the fault it became done without a resume; peer bytes matched. | [retry](raw/retry-second.txt) |
| #88 startup with endpoint down | Entry 3 stayed queued with no error or retry deadline through restart. A persistent read-only observer sampled every 25 ms without ending during the SQLite writer restart. Reconnection published it without a resume; peer bytes matched. | [offline repeat](raw/offline-third.txt), [recovery](raw/online.txt) |
| #89 own saves cached at publication | A 262,177-byte save was uploaded and verified through the separate peer, with no online mount read. Its first mount read after offline restart matched SHA-256 `25626a35790b1d1d99f3cab272e92799acba85a6fc3589dc272b1396b4b5886c`. Mount device changed from 805306375 to 805306376. | [first offline read](raw/offline-second.txt) |
| #90 installed services | Agent and login-item self-tests passed; both live jobs were running, category 6, last exit 0, no refusal. This validates the installed build, without recreating a signing-category mismatch. | [live jobs](raw/live-services.txt) |
| First online names | Root and nested names appeared in listings 0.014 and 0.0185 seconds after peer command return. Their first pathname reads matched the peer payload. Only listings were polled before those reads. | [online](raw/online.txt) |

Every daemon restart in the primary checks had a new PID and a different mount device, rather
than merely reconnecting a surviving kernel mount. The offline startup repeat used a persistent
read-only SQLite connection, so opening a fresh observer could not race WAL-sidecar recreation.
The first own-save read in the earlier run remains valid independently of its observer issue.

## Controlled feed race and remaining FSKit limit

The [controlled test](raw/scripts/feed-race.py) restarted the remembered mount offline, verified
cached root/nested baseline bytes and deliberately obtained `ENOENT` for two unique future names.
It then let the endpoint answer while holding feed bodies, created those names from the peer,
and used a read-only RPC session to refresh both daemon listings before releasing the feed.
Session creation, operations and watch ran in one controller PID with the required generation.

Both actual nonlocal `Object` invalidations arrived after release, even though the core already
held their remote versions. Both FSKit listings showed the names. The nested pathname read the
right bytes; the deliberately missed root pathname still returned `ENOENT`
([structured report](raw/feed-race.json), [run](raw/feed-race-run.txt)). It was still missing at
13:07:03, more than five seconds after delivery. Both paths read matching bytes after a remount
([later check and remount](raw/pinned-name-remount.txt)).

The notification fix is verified live. It does not remove the existing root negative-name
blocker or establish the exact cause of the 18 October reboot observation. This is a peer on one
Mac, not the two-Mac scenario, and the natural timing starts after peer command return, not at a
separately instrumented server commit.

## Restored and limits

[Restore log](raw/restore.txt), [final registrations](raw/after.txt).

- No unpublished changes remained, and the daemon reported restart-safe. The test mount was unmounted and
  forgotten, its Keychain item removed, Start at login turned off, the temporary default deleted
  and the bridge restarted with the default state path. Local proxy/server processes stopped.
- The disposable rig was removed. `~/Library/Application Support/voidfs` was absent before and
  after. Only `/Applications/voidfs.app` is registered; its bridge is enabled, FSKit remains on,
  Finder Sync uses its extension, and no mount is remembered.
- A rollback ZIP of the old `2986b57` install remains at
  `apps/macos/build/previous-install-2986b57.zip` (ignored by Git; no registered old copy).
- No logout, reboot, clean-Mac install, update with pre-existing unpublished writes, two-Mac run,
  paid-app matrix, Login Items approval-flow or busy-eject check was performed.

## Reproducibility and harness corrections

The complete operator scripts are [rig.py](raw/scripts/rig.py),
[proxy.py](raw/scripts/proxy.py), [checks.py](raw/scripts/checks.py) and
[feed-race.py](raw/scripts/feed-race.py). `rig.py init` requires a freshly built local server at
`/private/tmp/voidfs-oct19-target/debug/voidfs-server`; [server build](raw/server-build.txt).
The phases run explicitly: `configure`, `retry`, `offline`, `online`, then `feed-race.py --run`.
They change login services, a state-directory default and a Keychain item; review and restore
those changes when reproducing. Mounted reads use checked Python reads, so a read error cannot
be mistaken for an empty file's hash.

Three harness problems were corrected and the affected observations repeated:
- `void show` needs separate drive/path arguments, not the upload target syntax
  ([initial error](raw/retry.txt)). The peer read was corrected before mutation faults began.
- Apple's Python launcher changed argv[0] after process creation, so the proxy identity guard
  correctly refused a stop ([initial error](raw/offline.txt)). UID, launch time and exact script
  arguments were verified, then process recording moved after listener readiness.
- Reopening a read-only SQLite observer raced WAL-sidecar recreation and ended its thread in
  the [first offline run](raw/offline-second.txt). Before/after queue state and the first offline
  own-save read succeeded, but that sampler was incomplete. The [repeat](raw/offline-third.txt)
  kept its read-only connection open through the restart and sampled successfully.

No product source changed during the live run. No further Rust tests were needed beyond PR #91's
779 passing workspace tests and clean Clippy run. All four Python operator scripts passed syntax
checks; the proxy's signing, mutation faults and feed gate were exercised by the live checks.
