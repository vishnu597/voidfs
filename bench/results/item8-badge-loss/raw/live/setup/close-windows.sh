#!/bin/bash
# Closes the Finder windows this run opened, by the ids open-live.sh recorded, and only those:
# close-windows.sh <run dir>. Prints how many it closed.
n=0
for f in "$1"/out/*-window.txt; do
  [ -f "$f" ] || continue
  id=$(cat "$f")
  c=$(/usr/bin/osascript -e "tell application \"Finder\" to count (every Finder window whose id is $id)")
  if [ "$c" != 0 ]; then /usr/bin/osascript -e "tell application \"Finder\" to close Finder window id $id"; n=$((n + c)); fi
  rm "$f"
done
echo "closed $n"
