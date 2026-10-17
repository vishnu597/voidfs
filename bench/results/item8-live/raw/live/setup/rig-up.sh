#!/bin/bash
# An isolated test rig: a filesystem-store server on a spare loopback port and a test daemon with
# its own short state directory. Usage: rig-up.sh <port> <admin port> <drive> [relay port]. With a
# relay port, the daemon reaches the server through relay.py there. Prints the run dir.
set -eu
REPO=/Users/vishnu/Coding/voidfs/.claude/worktrees/loving-mahavira-5fa530
PORT=$1; ADMIN=$2; DRIVE=$3; RELAY=${4:-}
ENDPOINT_PORT=${RELAY:-$PORT}
RUN=$(mktemp -d /private/tmp/vi8.XXXXXX)
mkdir "$RUN/mnt" "$RUN/server" "$RUN/setup"
cat > "$RUN/env.sh" <<EOF
export VOIDFS_STATE_DIR="$RUN/state"
export VOIDFS_ENDPOINT="http://127.0.0.1:$ENDPOINT_PORT"
export RIG_SERVER="http://127.0.0.1:$PORT"
export RIG_RELAY_PORT="$RELAY"
export VOIDFS_ACCESS_KEY_ID="VFSPIKEKEY0000000000"
export VOIDFS_SECRET_ACCESS_KEY="spikesecretspikesecretspikesecretspike1"
export RIG_RUN="$RUN"
export RIG_PORT="$PORT"
export RIG_ADMIN="$ADMIN"
export RIG_DRIVE="$DRIVE"
export RIG_VOID="$REPO/target/debug/void"
export RIG_APP="$REPO/apps/macos/build/Build/Products/Release/voidfs.app"
export RIG_PEER="$REPO/target/debug/examples/mount_peer"
EOF
chmod 600 "$RUN/env.sh"
. "$RUN/env.sh"
VOIDFS_LISTEN="127.0.0.1:$PORT" VOIDFS_STORE="fs:$RUN/server" \
  "$REPO/target/debug/voidfs-server" --store "fs:$RUN/server" --listen "127.0.0.1:$PORT" --admin-listen "127.0.0.1:$ADMIN" \
  --new-pool-feature multi-object-versions > "$RUN/setup/server.log" 2>&1 &
echo $! > "$RUN/setup/server.pid"
if [ -n "$RELAY" ]; then
  /usr/bin/python3 -I "$(dirname "$0")/relay.py" "$RELAY" "$PORT" > "$RUN/setup/relay.log" 2>&1 &
  echo $! > "$RUN/setup/relay.pid"
fi
ok=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
  if /usr/bin/perl -e 'alarm shift; exec @ARGV' 3 "$RIG_VOID" --json drives > "$RUN/setup/server-ready.json" 2>/dev/null; then ok=1; break; fi
  sleep 1
done
test $ok = 1 || { echo "server not ready; see $RUN/setup/server.log" >&2; exit 1; }
"$RIG_VOID" drive create "$DRIVE" > "$RUN/setup/drive-create.txt"
"$RIG_VOID" daemon start > "$RUN/setup/daemon-start.txt"
"$RIG_VOID" --json daemon status > "$RUN/setup/daemon-status.json"
echo "$RUN"
