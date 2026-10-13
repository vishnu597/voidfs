# Upload drive controls across aliases and stable IDs

13 October 2026, Apple M5 Pro, macOS 27.0.1 (26A434). These tests use ephemeral loopback
`TestServer` instances and daemon state directories under private temporary storage. They do
not use a production endpoint, the repository's `.env*`, or shared daemon state.

Mounted saves are queued under a stable drive ID; CLI imports are queued under the display
alias. The old daemon resolved both `void uploads pause --drive <alias>` and
`--drive <id>` to the alias, so the mounted entries remained eligible to publish. Changing
every scope to an ID would instead miss existing CLI alias entries.

The contained fix keeps both stored names and makes drive pause/resume/cancel include both.
It persists both pause keys in one transaction, extends existing pauses before a new mounted
session or import can enqueue, and migrates legacy alias-only pauses before the queue worker
starts. Online resolution refreshes the existing bounded, validated identity memo; offline
controls use that memo. The public daemon scope schema and queue entry/journal formats do not
change. Unknown offline names retain the previous exact-name behavior.

The production change is independent of the application's large-file guarded-publication fix.
This evidence accompanies its focused PR; unrelated media and GUI work is recorded separately in the application matrix.

| Regression | Faithful old code | Fix alone on promoted main |
| --- | --- | --- |
| Mixed CLI alias + mounted ID: pause by alias, resume/cancel by ID; unrelated drive untouched | [Fails](raw/old-mixed-names.txt): pause counts 1 instead of 2 | Pass |
| Alias pause before the first session, then a newly queued mounted ID entry | [Fails](raw/old-future-mount.txt): the ID is not paused | Pass; resumed server bytes equal `mounted bytes` |
| Restart with legacy alias-only pause, then offline ID/alias controls | [Fails](raw/old-startup-migration.txt): startup leaves only the alias | Pass; migration precedes any session/control request and both names work offline |

Each old-code run executed exactly one new test under a 180-second timeout, returned Cargo
exit 101 and failed at its intended behavioral assertion. The old production sources were
unpacked with `git archive 0fe8913ba3d2d6a94ab899762943accbff259cca` into an owned private
temporary folder; only the new integration-test file was copied into them. No shared source
was rolled back. The same folder then received only this production patch, without the
concurrent SDK/media changes: [all five upload tests pass](raw/fixed-tests.txt). The tests
use daemon filesystem RPCs to create mounted entries; an FSKit mount is not required to
exercise this queue-control defect.

Adjacent daemon bootstrap/shutdown, stable identity, offline unmount, provisional sessions,
ownership/security and filesystem sessions pass: [31 tests, 1 measurement ignored](raw/adjacent-tests.txt).
Isolated daemon/client all-target clippy with warnings denied is in
[isolated-clippy.txt](raw/isolated-clippy.txt). These targeted checks do not claim a full
workspace or interoperability run for this PR.

## Reproduce

Run from the repository root. The recorded target was `target/item6`; a fresh named target
keeps a review run separate. The tests need local Unix sockets and loopback test-server access.

```bash
CARGO_TARGET_DIR=target/upload-drive-scope-check \
  /usr/bin/perl -e 'alarm shift; exec @ARGV' 180 \
  cargo test --locked -p voidfs-daemon --test uploads -- --nocapture

CARGO_TARGET_DIR=target/upload-drive-scope-check \
  /usr/bin/perl -e 'alarm shift; exec @ARGV' 180 \
  cargo test --locked -p voidfs-daemon --test identity --test offline_unmount \
  --test sessions --test bootstrap_shutdown --test provisional --test session_security

CARGO_TARGET_DIR=target/upload-drive-scope-check \
  cargo clippy --locked -p voidfs-daemon -p voidfs-client --all-targets -- -D warnings
```

To repeat the old-code proof from a checkout containing the new tests, preserve the current
test file in an isolated promoted-main source tree and run each test individually:

```bash
SCOPE_OLD=$(mktemp -d /private/tmp/voidfs-upload-scope-old.XXXXXX)
git archive 0fe8913ba3d2d6a94ab899762943accbff259cca | tar -x -C "$SCOPE_OLD"
cp crates/voidfs-daemon/tests/uploads.rs "$SCOPE_OLD/crates/voidfs-daemon/tests/uploads.rs"
SCOPE_TARGET="$PWD/target/upload-drive-scope-check"
cd "$SCOPE_OLD"
for SCOPE_TEST in \
  drive_controls_apply_to_cli_alias_and_mounted_id_entries \
  alias_pause_covers_new_mounted_entries_before_the_first_session \
  legacy_alias_pause_is_migrated_before_worker_start_and_offline_controls_share_names
do
  CARGO_TARGET_DIR="$SCOPE_TARGET" /usr/bin/perl -e 'alarm shift; exec @ARGV' 180 \
    cargo test --locked -p voidfs-daemon --test uploads "$SCOPE_TEST" -- --exact --nocapture
done
```

The fixes live in client queue scope/migration/control code and daemon uploads/session
identity/startup code, with the three integration regressions in
[`crates/voidfs-daemon/tests/uploads.rs`](../../../crates/voidfs-daemon/tests/uploads.rs).
No dependency, policy, server protocol or automatic publication conflict rule is added.
