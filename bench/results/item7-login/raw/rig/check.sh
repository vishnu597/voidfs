#!/bin/bash
# check.sh <phase>: what is there now, written to $EV/<phase>/. Reads only; starts nothing.
. $HOME/.voidfs-login-rig/env.sh
P=$EV/$1; mkdir -p $P
{ echo "now: $(date '+%Y-%m-%d %H:%M:%S %z')"; echo "booted: $(sysctl -n kern.boottime)"; echo "console login: $(last -1 $USER 2>/dev/null | head -1)"; } > $P/when.txt
for j in dev.voidfs.app.daemon dev.voidfs.agent; do
  { echo "\$ launchctl print gui/\$UID/$j (excerpt)"; launchctl print gui/$UID/$j 2>&1 | grep -E '^\s*(state|pid|runs|last exit code|last terminating signal|program identifier|program|parent bundle identifier) = |signing-identifier|could not find|not loaded' ; } > $P/job-$j.txt
done
VOIDFS_STATE_DIR=$D $VOID status --json > $P/status.json 2>&1
VOIDFS_STATE_DIR=$D $VOID mounts --json > $P/mounts.json 2>&1
VOIDFS_STATE_DIR=$D $VOID status > $P/status.txt 2>&1
{ echo '$ mount | grep -i voidfs'; mount | grep -i voidfs; } > $P/mount-table.txt
$V status > $P/fskit.txt 2>&1
{ echo "\$ ls -la \$MNT"; ls -la $MNT 2>&1; echo; echo "\$ shasum -a 256 (through the mount)"; (cd $MNT 2>/dev/null && find . -type f ! -name '.DS_Store' -print0 | sort -z | xargs -0 shasum -a 256 2>&1); } > $P/listing.txt
{ echo "# launchd gives the jobs' argv[0] as a bare name, so match by job pid and parent too"; jp=$(for j in dev.voidfs.app.daemon dev.voidfs.agent; do launchctl print gui/$UID/$j 2>/dev/null | awk '/^\tpid = /{print $3}'; done | tr '\n' ' '); ps -axo pid,ppid,lstart,command | awk -v jp=" $jp " 'NR==1 || index(jp, " "$1" ") || index(jp, " "$2" ") || /\/voidfs.app\/|voidfs-server|\/(loginwindow|Dock|Finder)( |$)/' | grep -v awk; } | sed -E "s#$HOME#~#g" > $P/processes.txt
[ -f $D/daemon.log ] && tail -40 $D/daemon.log > $P/daemon-log-tail.txt
echo "wrote $P"; head -1 $P/fskit.txt; grep -E '"is"|"state"|"link"' $P/status.json | head -5; tail -n +2 $P/mount-table.txt
# sanitize: home paths as ~
grep -rl "$HOME" $P | while read f; do sed -i '' "s#$HOME#~#g" "$f"; done
