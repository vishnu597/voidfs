#!/bin/bash
# Stops a rig-up.sh rig: its daemon, its relay and its server.
set -u
RUN=$1
. "$RUN/env.sh"
"$RIG_VOID" daemon stop || true
for f in relay server; do
  pid=$(cat "$RUN/setup/$f.pid")
  kill -CONT "$pid" 2>/dev/null
  if ps -p "$pid" -o command= | grep -qE 'relay.py|voidfs-server'; then kill "$pid"; fi
done
sleep 1
for f in relay server; do pid=$(cat "$RUN/setup/$f.pid"); ps -p "$pid" > /dev/null && echo "$f $pid still running" || echo "$f $pid stopped"; done
test -e "$HOME/Library/Application Support/voidfs" && echo "user state present" || echo "user state absent"
