#!/bin/zsh
# SPDX-License-Identifier: Apache-2.0
#
# The FSKit spike's measurements, against a drive seeded by seed.py and a module that is
# installed and enabled. Remounts before every cold measurement, so neither the kernel nor the
# module has anything cached.
#
#     apps/macos/scripts/bench.sh [voidfs://127.0.0.1:9000/spike] [/tmp/voidfs-mnt]
#
# Request counts come from the module's spike-only `.voidfs-stats` hook, read back from the log.

set -euo pipefail
zmodload zsh/datetime

url=${1:-voidfs://127.0.0.1:9000/spike}
mnt=${2:-/tmp/voidfs-mnt}
here=${0:A:h}
tools=${TMPDIR:-/tmp}/voidfs-bench-tools
mkdir -p "$tools"
cc -O2 -o "$tools/randread" "$here/randread.c"
cc -O2 -o "$tools/bulkstat" "$here/bulkstat.c"

remount() {
    umount "$mnt" 2>/dev/null || true
    mkdir -p "$mnt"
    mount -F -t voidfs "$url" "$mnt"
}

stats() {
    stat "$mnt/.voidfs-stats" >/dev/null 2>&1 || true
    sleep 1
    /usr/bin/log show --last 5s --style compact --predicate 'subsystem == "dev.voidfs" AND eventMessage BEGINSWITH "stats:"' |
        tail -1 | sed 's/.*stats: /    /'
}

reset() { stat "$mnt/.voidfs-stats-reset" >/dev/null 2>&1 || true; }

# Runs a command and prints its wall time in milliseconds.
timed() {
    local label=$1; shift
    local start=$EPOCHREALTIME
    "$@" >/dev/null
    printf '  %-44s %8.1f ms\n' "$label" $(( (EPOCHREALTIME - start) * 1000 ))
}

echo "== Listing (fresh mount before each cold run)"
for dir in many many10k; do
    remount; reset
    timed "$dir: cold ls -f (readdir only)" ls -f "$mnt/$dir"
    timed "$dir: warm ls -f" ls -f "$mnt/$dir"
    stats
    remount; reset
    timed "$dir: cold ls -l (readdir + lstat each)" ls -l "$mnt/$dir"
    timed "$dir: warm ls -l" ls -l "$mnt/$dir"
    stats
    remount; reset
    timed "$dir: cold getattrlistbulk (Finder's call)" "$tools/bulkstat" "$mnt/$dir"
    timed "$dir: warm getattrlistbulk" "$tools/bulkstat" "$mnt/$dir"
    sleep 6
    timed "$dir: getattrlistbulk after the 5 s TTL" "$tools/bulkstat" "$mnt/$dir"
    stats
done

echo "== Sequential read of media/big.bin (1 GiB)"
for bs in 1m 8m; do
    remount; reset
    echo "  cold, dd bs=$bs:"
    dd if="$mnt/media/big.bin" of=/dev/null bs=$bs 2>&1 | tail -1 | sed 's/^/    /'
    stats
done
echo "  warm (kernel cache), dd bs=1m:"
dd if="$mnt/media/big.bin" of=/dev/null bs=1m 2>&1 | tail -1 | sed 's/^/    /'

echo "== Random 4 KiB reads of media/big.bin"
remount; reset
"$tools/randread" "$mnt/media/big.bin" 300 4096 1 | sed 's/^/  /'
stats
remount; reset
"$tools/randread" "$mnt/media/big.bin" 300 4096 0 | sed 's/^/  /'
stats

echo "== First byte of a small file in a folder not yet listed"
remount; reset
timed "cat docs/nested/deep/readme.md (cold)" cat "$mnt/docs/nested/deep/readme.md"
stats
