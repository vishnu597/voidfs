#!/bin/bash
# Starts the test server on 127.0.0.1:9372 with a pool in ~/.voidfs-login-rig/pool, unless it answers.
. $HOME/.voidfs-login-rig/env.sh
if curl -s -o /dev/null -m 2 $EP/; then echo "server already answers on $EP"; exit 0; fi
/usr/bin/python3 -I -c '
import json,os,sys
k=json.load(open(sys.argv[1]))
env=dict(os.environ, VOIDFS_ACCESS_KEY_ID=k["accessKeyId"], VOIDFS_SECRET_ACCESS_KEY=k["secretAccessKey"])
os.execve(sys.argv[2], [sys.argv[2], "--store", "fs:"+sys.argv[3], "--listen", "127.0.0.1:9372", "--new-pool-feature", "multi-object-versions"], env)' $R/key.json $R/bin/voidfs-server $R/pool >> $R/server.log 2>&1 &
for i in $(seq 1 50); do curl -s -o /dev/null -m 1 $EP/ && { echo "server answers on $EP (pid $(pgrep -f "$R/bin/voidfs-server" | head -1))"; exit 0; }; sleep 0.1; done
echo "server didn't answer"; exit 1
