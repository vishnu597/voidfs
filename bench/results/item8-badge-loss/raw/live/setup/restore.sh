#!/bin/bash
# Puts back what register.sh changed: restore.sh <run dir> <app>. Logs each command.
R=$1; APP=$2
LSREG=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
set -x
bash "$(dirname "$0")/close-windows.sh" "$R/mnt"
mount | grep -F "$R/mnt"
pluginkit -e default -i dev.voidfs.app.findersync; echo rc=$?
pluginkit -r "$APP/Contents/PlugIns/VoidfsFinder.appex"; echo rc=$?
pluginkit -r "$APP/Contents/Extensions/VoidfsFS.appex"; echo rc=$?
"$APP/Contents/MacOS/voidfs" agent-unregister; echo rc=$?
sleep 2
$LSREG -u "$APP"; echo rc=$?
defaults delete dev.voidfs.app VoidfsStateDirectory; echo rc=$?
while read -r p; do
  $LSREG -f -R "$p"; echo rc=$?
  [ -d "$p/Contents/Extensions/VoidfsFS.appex" ] && { pluginkit -a "$p/Contents/Extensions/VoidfsFS.appex"; echo rc=$?; }
done < <(grep -v '^/Applications/' "$(dirname "$0")/others.txt")
pluginkit -a /Users/vishnu/Coding/voidfs/.claude/worktrees/sleepy-germain-0cf80f/apps/macos/build/Build/Products/Release/voidfs.app/Contents/PlugIns/VoidfsFinder.appex; echo rc=$?
$LSREG -f -R /Applications/voidfs.app; echo rc=$?
pluginkit -a /Applications/voidfs.app/Contents/Extensions/VoidfsFS.appex; echo rc=$?
/Applications/voidfs.app/Contents/MacOS/voidfs agent-register; echo rc=$?
sleep 2
/Applications/voidfs.app/Contents/MacOS/voidfs status | head -1
/Applications/voidfs.app/Contents/MacOS/voidfs agent-status
pluginkit -m -v -p com.apple.FinderSync
pluginkit -m -v -i dev.voidfs.app.fskit
set +x
