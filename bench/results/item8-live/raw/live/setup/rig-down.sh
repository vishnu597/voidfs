#!/bin/bash
# Stops a rig rig-up.sh started: its daemon, then the server whose PID it recorded.
set -u
RUN=$1
. "$RUN/env.sh"
"$RIG_VOID" daemon stop || true
pid=$(cat "$RUN/setup/server.pid")
if ps -p "$pid" -o command= | grep -q voidfs-server; then kill "$pid"; fi
if [ -f "$RUN/setup/relay.pid" ]; then r=$(cat "$RUN/setup/relay.pid"); kill -CONT "$r" 2>/dev/null; ps -p "$r" -o command= | grep -q relay.py && kill "$r"; fi
sleep 1
ps -p "$pid" > /dev/null && echo "server $pid still running" || echo "server $pid stopped"
test -e "$HOME/Library/Application Support/voidfs" && echo "user state present" || echo "user state absent"
