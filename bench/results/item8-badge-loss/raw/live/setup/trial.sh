#!/bin/bash
# One quick remount, as phase O of the live run made it: trial.sh <label>. Closes the test window,
# stops and starts the daemon in one command (it unmounts, then mounts the drive again), and opens
# <mnt>/live in Finder, logging the time of each step. Needs the rig's env.sh sourced.
L=$1; O=$RIG_RUN/out; M=$RIG_RUN/mnt
ts() { /usr/bin/perl -MPOSIX=strftime -MTime::HiRes=time -e '$t=time; printf "%s.%03d", strftime("%H:%M:%S", localtime $t), ($t-int $t)*1000'; }
{
echo "$(ts) close the test window: $(bash "$(dirname "$0")/close-windows.sh" "$M")"
sleep 1
echo "$(ts) void daemon stop; void daemon start"
"$RIG_VOID" daemon stop; "$RIG_VOID" daemon start
echo "$(ts) daemon started"
/usr/bin/perl -e 'alarm shift; exec @ARGV' 20 ls "$M/live" > /dev/null; echo "$(ts) ls <mnt>/live rc=$?"
open "$M/live"; echo "$(ts) open <mnt>/live"
"$RIG_VOID" mounts
} > "$O/$L.txt" 2>&1
cat "$O/$L.txt"
