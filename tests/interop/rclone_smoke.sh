#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Exercises a voidfs server with stock rclone, the way people sync folders with it.
#
#   VOIDFS_ENDPOINT=http://127.0.0.1:9000 VOIDFS_ACCESS_KEY_ID=... VOIDFS_SECRET_ACCESS_KEY=... \
#       tests/interop/rclone_smoke.sh
#
# Needs rclone. The remote comes from the environment alone: no rclone.conf is read or written.
# Exits non-zero at the first failure.

set -euo pipefail

: "${VOIDFS_ENDPOINT:?set VOIDFS_ENDPOINT}" "${VOIDFS_ACCESS_KEY_ID:?set VOIDFS_ACCESS_KEY_ID}" "${VOIDFS_SECRET_ACCESS_KEY:?set VOIDFS_SECRET_ACCESS_KEY}"
command -v rclone > /dev/null || { echo "needs rclone" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/voidfs-rclone.XXXXXX")"
trap 'rm -rf "$work"' EXIT
export RCLONE_CONFIG="$work/rclone.conf"
: > "$RCLONE_CONFIG"
export RCLONE_CONFIG_VOIDFS_TYPE=s3
export RCLONE_CONFIG_VOIDFS_PROVIDER=Other
export RCLONE_CONFIG_VOIDFS_ENDPOINT="$VOIDFS_ENDPOINT"
export RCLONE_CONFIG_VOIDFS_ACCESS_KEY_ID="$VOIDFS_ACCESS_KEY_ID"
export RCLONE_CONFIG_VOIDFS_SECRET_ACCESS_KEY="$VOIDFS_SECRET_ACCESS_KEY"
export RCLONE_CONFIG_VOIDFS_REGION=us-east-1
export RCLONE_CONFIG_VOIDFS_FORCE_PATH_STYLE=true

bucket="interop-rclone-$(LC_ALL=C tr -dc 'a-z0-9' < /dev/urandom | head -c 10 || true)"
remote="voidfs:$bucket"
passed=0
rc() { rclone --retries 1 --low-level-retries 1 --stats 0 "$@"; }
ok() { passed=$((passed + 1)); echo "ok    $1"; }
fail() { echo "FAIL  $1" >&2; exit 1; }

# A folder with small, nested and multipart-sized files.
src="$work/src"
mkdir -p "$src/dir/deep"
printf 'hello world\n' > "$src/a.txt"
head -c 1048576 /dev/urandom > "$src/dir/b.bin"
printf 'deep\n' > "$src/dir/deep/c.txt"
head -c 20971520 /dev/urandom > "$src/big.bin"

rc mkdir "$remote" && rc lsd voidfs: | grep -q " $bucket\$" && ok "mkdir and lsd" || fail "mkdir and lsd"

# Multipart for the 20 MiB file: parts of 5 MiB above an 8 MiB cutoff.
rc copy "$src" "$remote/tree" --s3-upload-cutoff 8M --s3-chunk-size 5M || fail "copy"
ok "copy (with a multipart upload)"
rc check "$src" "$remote/tree" --download || fail "check after copy"
ok "check --download"
[[ "$(rc size "$remote/tree" --json)" == *'"count":4'* ]] && ok "size" || fail "size: $(rc size "$remote/tree" --json)"

# Sync: a changed file, a deleted one and a new one.
printf 'hello again\n' > "$src/a.txt"
rm "$src/dir/deep/c.txt"
printf 'new\n' > "$src/new.txt"
rc sync "$src" "$remote/tree" || fail "sync"
rc check "$src" "$remote/tree" --download || fail "check after sync"
ok "sync and check"

# Server-side copy and move, and streaming from stdin.
rc copyto "$remote/tree/a.txt" "$remote/copy/a.txt" || fail "copyto"
[[ "$(rc cat "$remote/copy/a.txt")" == "hello again" ]] && ok "server-side copyto" || fail "copyto content"
rc moveto "$remote/copy/a.txt" "$remote/moved/a.txt" || fail "moveto"
[[ "$(rc cat "$remote/moved/a.txt")" == "hello again" ]] && ! rc lsf "$remote/copy/" 2> /dev/null | grep -q . && ok "moveto" || fail "moveto result"
printf 'streamed\n' | rc rcat "$remote/rcat.txt" || fail "rcat"
[[ "$(rc cat "$remote/rcat.txt")" == "streamed" ]] && ok "rcat and cat" || fail "rcat content"

rc deletefile "$remote/rcat.txt" && ! rc lsf "$remote/" | grep -q '^rcat.txt$' && ok "deletefile" || fail "deletefile"
# Not `rclone purge`: on a versioned bucket it deletes every version, and voidfs keeps history
# (DeleteObject with versionId is 501, protocol §3). Delete, then remove the drive.
rc delete "$remote" && rc rmdir "$remote" || fail "delete and rmdir"
! rc lsd voidfs: | grep -q " $bucket\$" && ok "delete and rmdir remove the bucket" || fail "rmdir left the bucket"

echo "$passed passed"
