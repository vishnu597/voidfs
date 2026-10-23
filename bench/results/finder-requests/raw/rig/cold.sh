#!/bin/bash
# Empties the private daemon's cache and namespace, keeping its connection, and mounts again.
set -euo pipefail
R=$HOME/.voidfs-finder-rig
V=${VOID:-/Applications/voidfs.app/Contents/MacOS/void}
export VOIDFS_STATE_DIR=$R/state VOIDFS_ENDPOINT=http://127.0.0.1:9380 VOIDFS_KEY_FILE=$R/key.env
$V unmount "$R/mnt" >/dev/null 2>&1 || true
$V daemon stop >/dev/null
rm -f "$R/state/state.sqlite" "$R/state/state.sqlite-wal" "$R/state/state.sqlite-shm" "$R/state/drive-aliases.json"
rm -rf "$R/state/cache"; mkdir "$R/state/cache"
$V daemon start >/dev/null
if [ "${1:-}" != "--no-mount" ]; then $V mount finder "$R/mnt" >/dev/null; fi
