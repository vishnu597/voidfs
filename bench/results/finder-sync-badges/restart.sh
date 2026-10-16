#!/bin/bash
# Runs `voidfs badges-selftest <drive> daemon-restart` against the test daemon of <run-dir>
# (whose env.sh sets VOIDFS_STATE_DIR, the endpoint and the key) and restarts that daemon when
# the self-test says it is ready. Usage: restart.sh <run-dir> <drive> <output>
set -eu
run=$1 drive=$2 out=$3
repo=$(cd "$(dirname "$0")/../../.." && pwd)
source "$run/env.sh"
app=$repo/apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs
void=$repo/target/debug/void
/usr/bin/perl -e 'alarm shift; exec @ARGV' 600 "$app" badges-selftest "$drive" daemon-restart --socket "$VOIDFS_STATE_DIR/daemon.sock" \
  --peer "$repo/target/debug/examples/mount_peer" --files 20 > "$out" 2>&1 &
test=$!
until grep -q '^ready: restart the daemon' "$out" || ! kill -0 "$test" 2>/dev/null; do sleep 0.2; done
if grep -q '^ready:' "$out"; then
  "$void" daemon stop
  sleep 3
  "$void" daemon start
fi
wait "$test"
