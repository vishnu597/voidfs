#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
#
# Prints a mount's listing and byte hashes, one sorted line per item, so two mounts of the same
# drive can be compared with diff: path, type, size, permissions, modification time, and the
# SHA-256 of each file's bytes. Inode numbers differ by design and are left out.
#
#     apps/macos/scripts/manifest.sh <mountpoint> > manifest.txt

set -euo pipefail
mnt=${1:?usage: manifest.sh <mountpoint>}
cd "$mnt"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
find . -mindepth 1 -print0 | xargs -0 stat -f '%N%t%HT%t%z%t%Lp%t%m' | LC_ALL=C sort > "$tmp/stat"
find . -type f -print0 | xargs -0 shasum -a 256 | awk '{h=$1; $1=""; sub(/^ +/, ""); print $0 "\t" h}' | LC_ALL=C sort > "$tmp/sha"
LC_ALL=C join -t "$(printf '\t')" -a 1 "$tmp/stat" "$tmp/sha"
