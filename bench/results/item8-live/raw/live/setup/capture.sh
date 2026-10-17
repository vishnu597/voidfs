#!/bin/bash
# Records what the CLI, the menu and Finder's model say at one moment: capture.sh <label> [png].
# Needs the rig's env.sh sourced.
L=$1; O=$RIG_RUN/out; APPBIN="$RIG_APP/Contents/MacOS/voidfs"
date '+%H:%M:%S' > "$O/$L-time.txt"
"$RIG_VOID" status > "$O/$L-status.txt" 2>&1
"$RIG_VOID" uploads --all > "$O/$L-uploads.txt" 2>&1
"$APPBIN" transfers -VoidfsStateDirectory "$VOIDFS_STATE_DIR" > "$O/$L-transfers.txt" 2>&1
: > "$O/$L-states.txt"
for f in "" live live/nested; do
  echo "\$ POST /v1/mounts/states {drive, folder: \"$f\"}" >> "$O/$L-states.txt"
  /usr/bin/curl -s --unix-socket "$VOIDFS_STATE_DIR/daemon.sock" -X POST http://daemon/v1/mounts/states \
    -H 'content-type: application/json' -d "{\"drive\":\"$RIG_DRIVE\",\"folder\":\"$f\"}" >> "$O/$L-states.txt"
  echo >> "$O/$L-states.txt"
done
if [ -n "${2:-}" ]; then
  "$APPBIN" transfers-selftest "$RIG_DRIVE" --look --png "$O/$L-menu.png" -VoidfsStateDirectory "$VOIDFS_STATE_DIR" > "$O/$L-menu.txt" 2>&1
fi
