#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
#
# One step of the head-to-head's scripted measurements, the FSKit spike's bench.sh made to work on
# any mount. The caller resets the mount first, so the step's first measurement is cold:
#   voidfs: restart voidfs-server (empty shard cache) and remount;
#   Space:  Clear Cache, Eject and Mount in its Settings (through computer use).
#
#     h2h-step.sh <label> <root of the test tree, e.g. /tmp/Space/h2h> <step> [tools dir]
#
# Steps: deep+many-f, many-bulk, many-l, 10k-f, 10k-bulk, 10k-l, dd1m, dd8m, rand-nocache,
# rand-cached. Tools: randread and bulkstat from apps/macos/scripts, built with cc -O2.

set -euo pipefail
label=$1; root=$2; step=$3; tools=${4:-${TMPDIR:-/tmp}/voidfs-bench-tools}
big=$root/media/big.bin

now() { perl -MTime::HiRes=time -e 'printf "%.6f", time'; }

# Runs a command and prints its wall time in milliseconds.
timed() {
    local what=$1; shift
    local t0 t1
    t0=$(now); "$@" >/dev/null; t1=$(now)
    perl -e "printf \"%-10s %-14s %-34s %10.1f ms\n\", '$label', '$step', '$what', ($t1 - $t0) * 1000"
}

# Prints dd's own summary line for a sequential read of the 1 GiB file.
ddread() {
    local bs=$1 what=$2
    printf '%-10s %-14s %-34s ' "$label" "$step" "$what"
    dd if="$big" of=/dev/null bs="$bs" 2>&1 | tail -1
}

case $step in
deep+many-f)
    timed "cold cat docs/nested/deep/readme.md" cat "$root/docs/nested/deep/readme.md"
    timed "cold ls -f many (1,000)" ls -f "$root/many"
    timed "warm ls -f many" ls -f "$root/many"
    timed "warm ls -l many" ls -l "$root/many"
    timed "warm getattrlistbulk many" "$tools/bulkstat" "$root/many" ;;
many-bulk)
    timed "cold getattrlistbulk many (1,000)" "$tools/bulkstat" "$root/many"
    timed "warm getattrlistbulk many" "$tools/bulkstat" "$root/many" ;;
many-l)
    timed "cold ls -l many (1,000)" ls -l "$root/many"
    timed "warm ls -l many" ls -l "$root/many" ;;
10k-f)
    timed "cold ls -f many10k (10,000)" ls -f "$root/many10k"
    timed "warm ls -f many10k" ls -f "$root/many10k" ;;
10k-bulk)
    timed "cold getattrlistbulk many10k" "$tools/bulkstat" "$root/many10k"
    timed "warm getattrlistbulk many10k" "$tools/bulkstat" "$root/many10k" ;;
10k-l)
    timed "cold ls -l many10k (10,000)" ls -l "$root/many10k"
    timed "warm ls -l many10k" ls -l "$root/many10k" ;;
dd1m)
    ddread 1m "cold dd bs=1m big.bin"
    ddread 1m "warm dd bs=1m big.bin" ;;
dd8m)
    ddread 8m "cold dd bs=8m big.bin" ;;
rand-nocache)
    printf '%-10s %-14s ' "$label" "$step"; "$tools/randread" "$big" 300 4096 1 ;;
rand-cached)
    printf '%-10s %-14s ' "$label" "$step"; "$tools/randread" "$big" 300 4096 0 ;;
*)
    echo "unknown step $step" >&2; exit 2 ;;
esac
