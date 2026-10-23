#!/bin/bash
# Unmounts and mounts the drive again, keeping the private daemon's state.
set -euo pipefail
R=$HOME/.voidfs-finder-rig
V=${VOID:-/Applications/voidfs.app/Contents/MacOS/void}
export VOIDFS_STATE_DIR=$R/state VOIDFS_ENDPOINT=http://127.0.0.1:9380 VOIDFS_KEY_FILE=$R/key.env
$V unmount "$R/mnt" >/dev/null
$V mount finder "$R/mnt" >/dev/null
