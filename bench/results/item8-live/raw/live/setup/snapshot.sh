#!/bin/bash
# The registrations and state a live run changes, for a before/after comparison.
LSREG=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
echo "## pluginkit FinderSync"; pluginkit -m -v -p com.apple.FinderSync
echo "## pluginkit fskit"; pluginkit -m -v -i dev.voidfs.app.fskit
echo "## defaults"; defaults read dev.voidfs.app 2>&1
echo "## launchctl"; launchctl print gui/$(id -u)/dev.voidfs.app.agent 2>&1 | head -3; launchctl list | grep -i voidfs
echo "## shared state exists?"; test -e "$HOME/Library/Application Support/voidfs" && echo yes || echo no
echo "## mounts"; mount | grep -i voidfs
echo "## launchctl dev.voidfs.agent"; launchctl print gui/$(id -u)/dev.voidfs.agent 2>&1 | grep -E '^\s+(state|program identifier|last exit code|job state|pid) ='
echo "## lsregister voidfs paths"; $LSREG -dump 2>/dev/null | grep -E '^path:.*(voidfs|Voidfs)' | sort -u
echo "## voidfs processes"; ps -axo pid,command | grep -iE 'voidfs|VoidfsFinder|VoidfsFS' | grep -vE 'grep|snapshot.sh|claude|rustc|cargo' | cut -c1-200
