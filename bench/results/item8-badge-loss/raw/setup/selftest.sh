#!/bin/bash
# Runs one badges-selftest mode against a rig-up.sh rig, doing what its "ready:" lines ask:
# the relay answers 503 or forwards again, or the daemon restarts.
# Usage: selftest.sh <run> <label> <mode> [options]. The output goes to <run>/out/<label>.txt.
set -u
RUN=$1; LABEL=$2; shift 2
. "$RUN/env.sh"
RELAY=$(cat "$RUN/setup/relay.pid")
start=$(date +%s)
"$RIG_APPBIN" badges-selftest "$RIG_DRIVE" "$@" --socket "$VOIDFS_STATE_DIR/daemon.sock" 2>&1 | while IFS= read -r line; do
  printf '%s\n' "$line"
  case "$line" in
    "ready: make the link answer errors now") kill -USR1 "$RELAY"; echo "+ relay answers 503 ($(( $(date +%s) - start )) s)";;
    "ready: let the link forward again") kill -USR2 "$RELAY"; echo "+ relay forwards ($(( $(date +%s) - start )) s)";;
    "ready: restart the daemon now"*) "$RIG_VOID" daemon stop; "$RIG_VOID" daemon start; echo "+ daemon restarted ($(( $(date +%s) - start )) s)";;
  esac
done > "$RUN/out/$LABEL.txt"
rc=${PIPESTATUS[0]}
echo "exit $rc, $(( $(date +%s) - start )) s" >> "$RUN/out/$LABEL.txt"
tail -3 "$RUN/out/$LABEL.txt"
exit "$rc"
