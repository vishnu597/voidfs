#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
# Undoes setup.sh: closes Finder windows on the mount, unmounts and forgets it, stops the private
# daemon, clears the agent's state-directory override and restarts the agent, and stops the
# relay and the server. Copy what you want from ~/.voidfs-finder-rig before deleting it.
set -uo pipefail
R=$HOME/.voidfs-finder-rig
V=/Applications/voidfs.app/Contents/MacOS/void
osascript -e 'on run argv
    tell application "Finder"
        repeat with w in (every Finder window)
            try
                if (POSIX path of (target of w as alias)) starts with (item 1 of argv) then close w
            end try
        end repeat
    end tell
end run' "$R/mnt/"
(
    export VOIDFS_STATE_DIR=$R/state VOIDFS_ENDPOINT=http://127.0.0.1:9380 VOIDFS_KEY_FILE=$R/key.env
    $V unmount "$R/mnt"
    $V daemon stop
)
defaults delete dev.voidfs.app VoidfsStateDirectory
launchctl kickstart -k "gui/$(id -u)/dev.voidfs.agent"
for pid in "$R/relay.pid" "$R/server.pid"; do
    kill "$(cat "$pid")" 2>/dev/null
done
# The server waits on open connections when asked to stop.
sleep 3
for pid in "$R/relay.pid" "$R/server.pid"; do
    kill -9 "$(cat "$pid")" 2>/dev/null
done
true
