#!/bin/bash
# Selects this build for a live run, as the user approved. Logs each command.
R=$1
. "$R/env.sh"
LSREG=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
set -x
defaults write dev.voidfs.app VoidfsStateDirectory "$R/state"; echo rc=$?
defaults read dev.voidfs.app
/Applications/voidfs.app/Contents/MacOS/voidfs agent-unregister; echo rc=$?
"$RIG_APP/Contents/MacOS/voidfs" agent-register; echo rc=$?
$LSREG -u /Applications/voidfs.app; echo rc=$?
pluginkit -a "$RIG_APP/Contents/Extensions/VoidfsFS.appex"; echo rc=$?
pluginkit -e use -i dev.voidfs.app.findersync; echo rc=$?
sleep 2
"$RIG_APP/Contents/MacOS/voidfs" status; echo rc=$?
"$RIG_APP/Contents/MacOS/voidfs" agent-status; echo rc=$?
pluginkit -m -v -p com.apple.FinderSync
pluginkit -m -v -i dev.voidfs.app.fskit
launchctl print gui/$(id -u)/dev.voidfs.agent | grep -E '^\s+(state|program|path|last exit|pid|job state) ='
set +x
