#!/bin/bash
# Opens <mnt>/live in Finder and records the new window's id: open-live.sh <run dir> <label>.
R=$1; L=$2
ids() { /usr/bin/osascript -e 'tell application "Finder" to get id of every Finder window' | tr -d ' ' | tr ',' '\n'; }
before=$(ids)
open "$R/mnt/live" || exit 1
for _ in $(seq 1 50); do
  new=$(comm -13 <(echo "$before" | sort) <(ids | sort) | head -1)
  [ -n "$new" ] && { echo "$new" > "$R/out/$L-window.txt"; echo "window $new"; exit 0; }
  sleep 0.1
done
echo "no new window"; exit 1
