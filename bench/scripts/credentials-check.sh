#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Checks storage credentials (step 4, item 6; protocol §5.5) against a real bucket: a server on a
# fresh pool under voidfs-bench/ in the bucket an env file names, its check at start of what minted
# credentials reach, the storage-credentials conformance cases against it, and `probe`. The pool is
# purged afterwards; what voidfs-bench/ holds is counted before and after. Nothing from the env file
# is printed, nor any secret the server mints.
#
#   cargo build --release -p voidfs-server -p voidfs-conformance -p voidfs-bench
#   bench/scripts/credentials-check.sh .env.aws | tee bench/results/storage-credentials/aws-check.txt
#
# The env file holds VOIDFS_S3_BUCKET, VOIDFS_S3_REGION, VOIDFS_S3_ACCESS_KEY_ID and
# VOIDFS_S3_SECRET_ACCESS_KEY (and VOIDFS_S3_ENDPOINT for a service other than AWS), and for AWS
# VOIDFS_STORAGE_CREDENTIALS_ROLE: the role storage credentials are minted from, which may read the
# bucket's voidfs-bench/ and trusts those keys to assume it.
#
#   VOIDFS_PORT   loopback port (default 9400)

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
env_file="${1:?usage: credentials-check.sh <env file>}"
port="${VOIDFS_PORT:-9400}"
work="$(mktemp -d "${TMPDIR:-/tmp}/voidfs-credentials-check.XXXXXX")"
pid=""
cleanup() {
    [[ -n "$pid" ]] && kill "$pid" 2> /dev/null || true
    wait 2> /dev/null || true
    rm -rf "${work:?}"
}
trap cleanup EXIT
random() { (set +o pipefail; LC_ALL=C tr -dc "$1" < /dev/urandom 2> /dev/null | head -c "$2"); }

set -a
# shellcheck disable=SC1090
. "$env_file"
set +a
: "${VOIDFS_S3_BUCKET:?the env file must name VOIDFS_S3_BUCKET}"
prefix="voidfs-bench/credentials-check-$(date -u +%Y%m%dT%H%M%SZ)"
bench="$root/target/release/voidfs-bench"

echo "# $(date -u +%Y-%m-%dT%H:%M:%SZ), $(git -C "$root" rev-parse --short HEAD), a pool at $prefix/ in the bucket the env file names"
echo "before: $("$bench" purge --prefix voidfs-bench/ 2>&1 | tail -1)"

export VOIDFS_ACCESS_KEY_ID="VF$(random 'A-Z2-7' 18)" VOIDFS_SECRET_ACCESS_KEY="$(random 'A-Za-z0-9' 40)"
export VOIDFS_READ_ACCESS_KEY_ID="VR$(random 'A-Z2-7' 18)" VOIDFS_READ_SECRET_ACCESS_KEY="$(random 'A-Za-z0-9' 40)"
RUST_LOG=warn,voidfs_server=info "$root/target/release/voidfs-server" --store "s3:$VOIDFS_S3_BUCKET/$prefix" --listen "127.0.0.1:$port" \
    --key "$VOIDFS_READ_ACCESS_KEY_ID:$VOIDFS_READ_SECRET_ACCESS_KEY:read" > "$work/voidfs.log" 2>&1 &
pid=$!
for _ in $(seq 1 300); do
    curl -s -o /dev/null "http://127.0.0.1:$port/" && break
    if ! kill -0 "$pid" 2> /dev/null; then
        echo "the server stopped:" >&2
        sed -E 's/(ASIA|AKIA)[A-Z0-9]{12,}/<key id>/g' "$work/voidfs.log" | tail -5 >&2
        exit 1
    fi
    sleep 0.1
done
echo "start-up: $(grep -h 'storage credentials' "$work/voidfs.log" | sed -E 's/^.*(storage credentials)/\1/' | head -1)"
VOIDFS_ENDPOINT="http://127.0.0.1:$port" "$root/target/release/voidfs-conformance" --case storage-credentials || true
kill "$pid" 2> /dev/null || true
wait "$pid" 2> /dev/null || true
pid=""
"$root/target/release/voidfs-server" --store "s3:$VOIDFS_S3_BUCKET/$prefix" probe 2>&1 | grep -E '^(pool|storage credentials|create-if-absent) ' || true

echo "purge: $("$bench" purge --prefix "$prefix/" 2>&1 | tail -1)"
echo "purge: $("$bench" purge --prefix "$prefix/" --yes 2>&1 | tail -1)"
echo "after: $("$bench" purge --prefix voidfs-bench/ 2>&1 | tail -1)"
