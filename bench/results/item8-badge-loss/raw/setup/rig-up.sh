#!/bin/bash
# An isolated test rig for the badge self-tests: a filesystem-store server on a spare loopback
# port, a relay in front of it (../../../item8-live/raw/live/setup/relay.py), and a test daemon
# with its own short state directory. Nothing is mounted or registered.
# Usage: rig-up.sh <port> <admin port> <relay port> <drive> <app>. Prints the run dir.
set -eu
REPO=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
PORT=$1; ADMIN=$2; RELAY=$3; DRIVE=$4; APP=$5
RUN=$(mktemp -d /private/tmp/vbl.XXXXXX)
mkdir "$RUN/server" "$RUN/setup" "$RUN/out"
cat > "$RUN/env.sh" <<EOF
export VOIDFS_STATE_DIR="$RUN/state"
export VOIDFS_ENDPOINT="http://127.0.0.1:$RELAY"
export VOIDFS_ACCESS_KEY_ID="VFSPIKEKEY0000000000"
export VOIDFS_SECRET_ACCESS_KEY="spikesecretspikesecretspikesecretspike1"
export RIG_RUN="$RUN"
export RIG_ADMIN="$ADMIN"
export RIG_DRIVE="$DRIVE"
export RIG_VOID="$REPO/target/debug/void"
export RIG_APPBIN="$APP/Contents/MacOS/voidfs"
export RIG_PEER="$REPO/target/debug/examples/mount_peer"
EOF
chmod 600 "$RUN/env.sh"
. "$RUN/env.sh"
"$REPO/target/debug/voidfs-server" --store "fs:$RUN/server" --listen "127.0.0.1:$PORT" --admin-listen "127.0.0.1:$ADMIN" \
  --new-pool-feature multi-object-versions > "$RUN/setup/server.log" 2>&1 &
echo $! > "$RUN/setup/server.pid"
/usr/bin/python3 -I "$REPO/bench/results/item8-live/raw/live/setup/relay.py" "$RELAY" "$PORT" > "$RUN/setup/relay.log" 2>&1 &
echo $! > "$RUN/setup/relay.pid"
ok=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
  if /usr/bin/perl -e 'alarm shift; exec @ARGV' 3 "$RIG_VOID" --json drives > "$RUN/setup/server-ready.json" 2>/dev/null; then ok=1; break; fi
  sleep 1
done
test $ok = 1 || { echo "server not ready; see $RUN/setup/server.log" >&2; exit 1; }
"$RIG_VOID" drive create "$DRIVE" > "$RUN/setup/drive-create.txt"
"$RIG_VOID" daemon start > "$RUN/setup/daemon-start.txt"
echo "$RUN"
