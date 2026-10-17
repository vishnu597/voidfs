#!/bin/bash
# Waits up to $1 seconds for the test daemon's queue to have nothing to send.
for _ in $(seq 1 "$1"); do
  n=$("$RIG_VOID" --json status | /usr/bin/python3 -c 'import json,sys; u=json.load(sys.stdin)["uploads"]; print(u.get("unpublished", 0))')
  [ "$n" = 0 ] && { echo "idle"; exit 0; }
  sleep 1
done
echo "not idle after $1 s"; exit 1
