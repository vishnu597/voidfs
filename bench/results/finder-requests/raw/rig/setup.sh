#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
# The 22 October rig, as it was set up: a throwaway server on loopback, the request relay in
# front of it, a private daemon reaching the server through the relay, and the installed app's
# agent pointed at that daemon's state directory, so the installed FSKit module mounts through it.
# Run from the repository root, with release builds of voidfs-server and the finder_seed example.
# Undo it with restore.sh.
set -euo pipefail
R=$HOME/.voidfs-finder-rig
V=/Applications/voidfs.app/Contents/MacOS/void
mkdir -p "$R"/store "$R"/state "$R"/logs "$R"/mnt "$R"/results
chmod 700 "$R"
(umask 077; $V keys generate --scope admin --format env > "$R/key.env")
cp target/release/voidfs-server "$R/voidfs-server"
cp apps/macos/scripts/request-relay.py "$R/request-relay.py"
cp bench/results/finder-requests/raw/rig/cold.sh bench/results/finder-requests/raw/rig/remount.sh "$R/"

set -a; . "$R/key.env"; set +a
nohup "$R/voidfs-server" --store "fs:$R/store" --listen 127.0.0.1:9381 --admin-listen 127.0.0.1:9382 \
    --new-pool-feature multi-object-versions > "$R/logs/server.log" 2>&1 &
echo $! > "$R/server.pid"
nohup /usr/bin/python3 -I "$R/request-relay.py" 9380 127.0.0.1:9381 "$R/logs/requests.jsonl" \
    --delay-file "$R/delay-ms" > "$R/logs/relay.log" 2>&1 &
echo $! > "$R/relay.pid"
until curl -sf -o /dev/null http://127.0.0.1:9382/readyz; do sleep 0.1; done

# The seed goes straight to the server, not through the relay. One tagged set per Finder run.
VOIDFS_ENDPOINT=http://127.0.0.1:9381 target/release/examples/finder_seed finder l0 i0 l12 i12

export VOIDFS_STATE_DIR=$R/state VOIDFS_ENDPOINT=http://127.0.0.1:9380 VOIDFS_KEY_FILE=$R/key.env
$V daemon start
defaults write dev.voidfs.app VoidfsStateDirectory "$R/state"
launchctl kickstart -k "gui/$(id -u)/dev.voidfs.agent"
# A hidden path: mounted under ~/voidfs, Raycast's file indexer walked the whole drive.
$V mount finder "$R/mnt"
