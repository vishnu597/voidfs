#!/bin/zsh
# SPDX-License-Identifier: Apache-2.0
#
# Cold and warm timings of a mounted drive seeded by mount_seed (or seed.py): listings, a
# sequential and random reads of media/big.bin, and the first byte of a deep file. Remounts before
# every cold measurement, so the kernel has nothing cached; VOIDFS_BENCH_COLD, if set, runs before
# each remount too (to empty the daemon's cache, say).
#
#     apps/macos/scripts/bench.sh <resource> [mountpoint=/tmp/voidfs-mnt]

set -euo pipefail
zmodload zsh/datetime

url=${1:?usage: bench.sh <resource> [mountpoint]}
mnt=${2:-/tmp/voidfs-mnt}
here=${0:A:h}
tools=${TMPDIR:-/tmp}/voidfs-bench-tools
mkdir -p "$tools"
cc -O2 -o "$tools/randread" "$here/randread.c"
cc -O2 -o "$tools/bulkstat" "$here/bulkstat.c"

remount() {
    umount "$mnt" 2>/dev/null || true
    if [[ -n ${VOIDFS_BENCH_COLD:-} ]]; then eval "$VOIDFS_BENCH_COLD"; fi
    mkdir -p "$mnt"
    local start=$EPOCHREALTIME
    mount -F -t voidfs "$url" "$mnt"
    mounted=$(( (EPOCHREALTIME - start) * 1000 ))
}

# Runs a command and prints its wall time in milliseconds.
timed() {
    local label=$1; shift
    local start=$EPOCHREALTIME
    "$@" >/dev/null
    printf '  %-44s %8.1f ms\n' "$label" $(( (EPOCHREALTIME - start) * 1000 ))
}

echo "== Mount"
remount
printf '  %-44s %8.1f ms\n' "mount" $mounted

echo "== Listing (fresh mount before each cold run)"
for dir in many many10k; do
    remount
    timed "$dir: cold ls -f (readdir only)" ls -f "$mnt/$dir"
    timed "$dir: warm ls -f" ls -f "$mnt/$dir"
    remount
    timed "$dir: cold ls -l (readdir + lstat each)" ls -l "$mnt/$dir"
    timed "$dir: warm ls -l" ls -l "$mnt/$dir"
    remount
    timed "$dir: cold getattrlistbulk (Finder's call)" "$tools/bulkstat" "$mnt/$dir"
    timed "$dir: warm getattrlistbulk" "$tools/bulkstat" "$mnt/$dir"
done

echo "== Sequential read of media/big.bin (1 GiB)"
for bs in 1m 8m; do
    remount
    echo "  cold, dd bs=$bs:"
    dd if="$mnt/media/big.bin" of=/dev/null bs=$bs 2>&1 | tail -1 | sed 's/^/    /'
done
echo "  warm (kernel cache), dd bs=1m:"
dd if="$mnt/media/big.bin" of=/dev/null bs=1m 2>&1 | tail -1 | sed 's/^/    /'

echo "== Random 4 KiB reads of media/big.bin"
remount
"$tools/randread" "$mnt/media/big.bin" 300 4096 1 | sed 's/^/  cold: /'
"$tools/randread" "$mnt/media/big.bin" 300 4096 1 | sed 's/^/  again: /'
remount
"$tools/randread" "$mnt/media/big.bin" 300 4096 0 | sed 's/^/  cold: /'

echo "== First byte of a small file in a folder not yet listed"
remount
timed "cat docs/nested/deep/readme.md (cold)" cat "$mnt/docs/nested/deep/readme.md"
timed "cat docs/nested/deep/readme.md (warm)" cat "$mnt/docs/nested/deep/readme.md"
