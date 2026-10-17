#!/bin/bash
# One quick remount, as phase O of the live run made it: trial.sh <label>. Closes the test window,
# stops and starts the daemon in one command (it unmounts, then mounts the drive again), waits
# for <mnt>/live to be there again, and opens it in Finder, logging the time of each step. Needs
# the rig's env.sh sourced. GAP=<seconds> waits that long between the stop and the start, as phase
# I's slow restart did.
L=$1; O=$RIG_RUN/out; M=$RIG_RUN/mnt; H=$(dirname "$0")
ts() { /usr/bin/perl -MPOSIX=strftime -MTime::HiRes=time -e '$t=time; printf "%s.%03d", strftime("%H:%M:%S", localtime $t), ($t-int $t)*1000'; }
{
echo "$(ts) close the test window: $(bash "$H/close-windows.sh" "$RIG_RUN")"
sleep 1
echo "$(ts) void daemon stop; sleep ${GAP:-0}; void daemon start"
"$RIG_VOID" daemon stop; sleep "${GAP:-0}"; "$RIG_VOID" daemon start
echo "$(ts) daemon started"
for _ in $(seq 1 200); do [ -e "$M/live" ] && break; sleep 0.1; done
t=$(ts); [ -e "$M/live" ] && echo "$t <mnt>/live is there" || echo "$t <mnt>/live is not there after 20 s"
w=$(bash "$H/open-live.sh" "$RIG_RUN" "$L"); echo "$(ts) open <mnt>/live: $w"
"$RIG_VOID" mounts
} > "$O/$L.txt" 2>&1
cat "$O/$L.txt"
