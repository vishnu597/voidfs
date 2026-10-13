# `.DS_Store` fixture setup sequencing

The [observed PR #66 CI failure](https://github.com/vishnu597/voidfs/actions/runs/38434669866) occurred while `write_new` opened a newly created nearby file: `ns.open` returned `FsError::Again` at `crates/voidfs-client/tests/mount_publication.rs:978`. The [unmodified failed job output](raw/ci-38434669866-failure.txt) is retained. Publication was resumed before these fixtures finished creating and writing their files, allowing the background publisher to change the namespace generation during setup.

The two tests cover local persistence and shadowing. `Fixture::new` already pauses the queue. The restart test now retains that pause until all setup writes finish, then resumes immediately before its existing ten-second settle. The rename/shadow test retains the pause until its existing `f.publish()` call, which resumes and settles. Every assertion and timeout is unchanged; no production code or retry behavior changed.

Validation on this Mac used a new private Cargo target with development/test debug information and incremental compilation disabled. The current combined workspace source included the existing #62 fixture change and the separately staged production fixes; CI on the PR will validate its own exact source. Each focused test passed all 20 executions (40/40 total), and the [entire mount publication suite](raw/mount-publication-suite.txt) passed 24/24. [Scoped clippy](raw/clippy.txt) passed with warnings denied. [Compilation](raw/compile.txt) took 46.94 seconds. [The summary](raw/validation-summary.json) lists the repeat logs and their byte hashes, the tested fixture source hash, and private-target cleanup. No deliberate broken-source runs or shared Cargo targets were used.

Rerun from the repository root with Rust/Cargo installed; the tests start throwaway local loopback servers. The commands below create and remove only their own target directory:

```sh
fixture_target=$(mktemp -d /private/tmp/voidfs-fixture-tests.XXXXXX)
trap 'rm -rf -- "$fixture_target"' EXIT
export CARGO_TARGET_DIR="$fixture_target"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4
cargo test --locked -p voidfs-client --test mount_publication --no-run
for test_name in \
  a_ds_store_stays_on_this_mac_across_a_restart_and_nearby_names_publish \
  a_local_ds_store_renames_only_as_one_and_shadows_the_drives_own
do
  for repetition in $(seq 1 20)
  do
    /usr/bin/perl -e 'alarm shift; exec @ARGV' 45 \
      cargo test --locked -p voidfs-client --test mount_publication \
      "$test_name" -- --exact || exit "$?"
  done
done
/usr/bin/perl -e 'alarm shift; exec @ARGV' 120 \
  cargo test --locked -p voidfs-client --test mount_publication
cargo clippy --locked -p voidfs-client --test mount_publication -- -D warnings
```
