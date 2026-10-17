#!/bin/bash
# Selects one build for the live run, as the user approved: register.sh <run dir> <app>. The
# other copies whose FSKit module or Finder extension LaunchServices lists (others.txt) are
# unregistered, so FSKit and Finder can only pick this one. Logs each command.
R=$1; APP=$2
LSREG=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
set -x
defaults write dev.voidfs.app VoidfsStateDirectory "$R/state"; echo rc=$?
/Applications/voidfs.app/Contents/MacOS/voidfs agent-unregister; echo rc=$?
while read -r p; do $LSREG -u "$p"; echo rc=$?; done < "$(dirname "$0")/others.txt"
"$APP/Contents/MacOS/voidfs" agent-register; echo rc=$?
pluginkit -a "$APP/Contents/Extensions/VoidfsFS.appex"; echo rc=$?
pluginkit -a "$APP/Contents/PlugIns/VoidfsFinder.appex"; echo rc=$?
pluginkit -e use -i dev.voidfs.app.findersync; echo rc=$?
sleep 2
"$APP/Contents/MacOS/voidfs" status; echo rc=$?
"$APP/Contents/MacOS/voidfs" agent-status; echo rc=$?
pluginkit -m -v -p com.apple.FinderSync
pluginkit -m -v -i dev.voidfs.app.fskit
launchctl print gui/$(id -u)/dev.voidfs.agent | grep -E '^\s+(state|pid|last exit code|job state) ='
set +x
