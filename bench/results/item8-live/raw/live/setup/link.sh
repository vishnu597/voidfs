#!/bin/bash
# One link phase: link.sh <label> <file to write on the mount>. Writes the file, then records how
# long `void status`, the menu's text and its drawing take, what they say, and the queue.
L=$1; F=$2; O=$RIG_RUN/out; APPBIN="$RIG_APP/Contents/MacOS/voidfs"
t() { local s=$(/usr/bin/perl -MTime::HiRes=time -e 'print time'); "$@"; local rc=$?; local e=$(/usr/bin/perl -MTime::HiRes=time -e 'print time'); /usr/bin/perl -e "printf \"(%.2f s, rc=$rc)\n\", $e - $s" ; }
{
echo "== $L at $(date '+%H:%M:%S')"
echo "+ write $F"; t /usr/bin/perl -e 'alarm shift; exec @ARGV' 20 /bin/sh -c "printf '$L\n' > '$RIG_RUN/mnt/live/$F'"
echo "+ ls the mount"; t /usr/bin/perl -e 'alarm shift; exec @ARGV' 20 ls "$RIG_RUN/mnt/live"
/bin/sleep 3
echo "+ void status"; t /usr/bin/perl -e 'alarm shift; exec @ARGV' 30 "$RIG_VOID" status
echo "+ voidfs transfers (the menu's text)"; t /usr/bin/perl -e 'alarm shift; exec @ARGV' 30 "$APPBIN" transfers -VoidfsStateDirectory "$VOIDFS_STATE_DIR"
echo "+ transfers-selftest (the menu's controls, then its drawing)"; t /usr/bin/perl -e 'alarm shift; exec @ARGV' 60 "$APPBIN" transfers-selftest "$RIG_DRIVE" --png "$O/$L-menu.png" -VoidfsStateDirectory "$VOIDFS_STATE_DIR"
echo "+ void uploads"; t /usr/bin/perl -e 'alarm shift; exec @ARGV' 30 "$RIG_VOID" uploads
echo "+ POST /v1/mounts/states live"; t /usr/bin/perl -e 'alarm shift; exec @ARGV' 30 /usr/bin/curl -s --unix-socket "$VOIDFS_STATE_DIR/daemon.sock" -X POST http://daemon/v1/mounts/states -H 'content-type: application/json' -d "{\"drive\":\"$RIG_DRIVE\",\"folder\":\"live\"}"
} > "$O/$L.txt" 2>&1
