#!/bin/bash
# Registration snapshot for the login and reboot check.
. $HOME/.voidfs-login-rig/env.sh
echo "## LaunchServices voidfs.app records"; $LSR -dump 2>/dev/null | grep -E "^path:.*(voidfs\.app( |$)|dev\.voidfs)" | sed -E 's/ +\(0x[0-9a-f]+\)$//' | sort -u
echo "## pluginkit voidfs plug-ins"; pluginkit -m -v -A 2>/dev/null | grep -i voidfs | awk -F'\t' '{print $1"\t"$4}'
echo "## FSKit (voidfs status)"; [ -x $V ] && $V status 2>&1 | head -1
echo "## launchd jobs"
for j in dev.voidfs.agent dev.voidfs.app.daemon dev.voidfs.daemon; do
  if out=$(launchctl print gui/$UID/$j 2>/dev/null); then echo "$j loaded: $(echo "$out" | grep -E '^\s*state = ' | head -1 | xargs), $(echo "$out" | grep -E '^\s*runs = ' | head -1 | xargs)"; else echo "$j not loaded"; fi
done
echo "## defaults VoidfsStateDirectory"; defaults read dev.voidfs.app VoidfsStateDirectory 2>&1 | head -1
echo "## ~/Library/LaunchAgents voidfs"; ls ~/Library/LaunchAgents | grep -i voidfs
echo "## real state directory"; ls ~/Library/Application\ Support/voidfs 2>&1 | head -2
echo "## ~/voidfs"; ls -A ~/voidfs 2>&1
echo "## voidfs mounts"; mount | grep -i voidfs
