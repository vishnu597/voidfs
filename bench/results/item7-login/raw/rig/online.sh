#!/bin/bash
# online.sh <phase>: starts the server, waits for the link to come online, then adds a file from
# another client and times how long it takes to appear in the mount. Writes $EV/<phase>/online.txt.
. $HOME/.voidfs-login-rig/env.sh
P=$EV/$1; mkdir -p $P; O=$P/online.txt
t0=$(/usr/bin/python3 -c 'import time; print(time.time())')
{ echo "## server"; $R/server.sh; } > $O 2>&1
for i in $(seq 1 300); do
  l=$(VOIDFS_STATE_DIR=$D $VOID status --json 2>/dev/null | /usr/bin/python3 -I -c 'import json,sys; print(json.load(sys.stdin)["connection"]["link"])' 2>/dev/null)
  [ "$l" = online ] && break; sleep 0.1
done
t1=$(/usr/bin/python3 -c 'import time; print(time.time())')
echo "## link: $l, $(/usr/bin/python3 -c "print(round($t1-$t0,2))") s after starting the server" >> $O
f=from-server-$1-$(date +%H%M%S).txt; echo "added from another client during $1 at $(date)" > $R/content/$f
VOIDFS_STATE_DIR=$C $VOID --endpoint $EP --key-file $R/key.json upload $R/content/$f $DRIVE:/ > $P/upload.txt 2>&1
t2=$(/usr/bin/python3 -c 'import time; print(time.time())')
for i in $(seq 1 600); do [ -f $MNT/$f ] && break; sleep 0.05; done
t3=$(/usr/bin/python3 -c 'import time; print(time.time())')
{ echo "## new file $f: uploaded (exit as in upload.txt) at +0; in the mount after $(/usr/bin/python3 -c "print(round($t3-$t2,2))") s"
  echo "local:  $(shasum -a 256 < $R/content/$f)"; echo "mount:  $(shasum -a 256 < $MNT/$f 2>&1)"; } >> $O
cat $O
