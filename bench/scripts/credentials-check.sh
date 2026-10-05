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
#   bench/scripts/credentials-check.sh .env.r2 | tee bench/results/storage-credentials/r2-check.txt
#
# The env file holds VOIDFS_S3_BUCKET, VOIDFS_S3_REGION, VOIDFS_S3_ACCESS_KEY_ID and
# VOIDFS_S3_SECRET_ACCESS_KEY (and VOIDFS_S3_ENDPOINT for a service other than AWS), and for AWS
# VOIDFS_STORAGE_CREDENTIALS_ROLE: the role storage credentials are minted from, which may read the
# bucket's voidfs-bench/ and trusts those keys to assume it.
# R2 uses VOIDFS_R2_API_TOKEN (or the older VOIDFS_TOKEN_VALUE), with account-level R2 admin
# read and write permission, and static S3 keys. Its region defaults to auto.
#
#   VOIDFS_PORT   loopback port (default 9400)

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
env_file="${1:?usage: credentials-check.sh <env file>}"
port="${VOIDFS_PORT:-9400}"
bench="${VOIDFS_BENCH_BIN:-$root/target/release/voidfs-bench}"
server="${VOIDFS_SERVER_BIN:-$root/target/release/voidfs-server}"
conformance="${VOIDFS_CONFORMANCE_BIN:-$root/target/release/voidfs-conformance}"
work="$(mktemp -d "${TMPDIR:-/tmp}/voidfs-credentials-check.XXXXXX")"
pid=""
needs_purge=0
purge_pool() {
    echo "purge: $("$bench" purge --prefix "$prefix/" 2>&1 | tail -1)"
    "$bench" purge --prefix "$prefix/" --yes > "$work/purge.log" 2>&1 || return 1
    echo "purge: $(tail -1 "$work/purge.log")"
    needs_purge=0
    echo "after: $("$bench" purge --prefix voidfs-bench/ 2>&1 | tail -1)"
}
cleanup() {
    local status=$?
    [[ -n "$pid" ]] && kill "$pid" 2> /dev/null || true
    wait 2> /dev/null || true
    if [[ "$needs_purge" == 1 ]]; then
        purge_pool || echo "could not purge this check's pool" >&2
    fi
    rm -rf "${work:?}"
    exit "$status"
}
trap cleanup EXIT
random() { (set +o pipefail; LC_ALL=C tr -dc "$1" < /dev/urandom 2> /dev/null | head -c "$2"); }
require_env() {
    local name="$1"
    if [[ -z "${!name:-}" ]]; then echo "$2" >&2; exit 1; fi
}

set -a
# shellcheck disable=SC1090
. "$env_file"
set +a
require_env VOIDFS_S3_BUCKET "the env file must name VOIDFS_S3_BUCKET"
case "${VOIDFS_S3_ENDPOINT:-}" in
    *'.r2.cloudflarestorage.com'*)
        export VOIDFS_S3_REGION="${VOIDFS_S3_REGION:-auto}"
        export VOIDFS_R2_API_TOKEN="${VOIDFS_R2_API_TOKEN:-${VOIDFS_TOKEN_VALUE:-}}"
        require_env VOIDFS_R2_API_TOKEN "the R2 env file must name VOIDFS_R2_API_TOKEN (or VOIDFS_TOKEN_VALUE)"
        require_env VOIDFS_S3_ACCESS_KEY_ID "the R2 env file must name a static VOIDFS_S3_ACCESS_KEY_ID"
        ;;
    '' | *'.amazonaws.com'*)
        require_env VOIDFS_STORAGE_CREDENTIALS_ROLE "the AWS env file must name VOIDFS_STORAGE_CREDENTIALS_ROLE, the role storage credentials are minted from"
        ;;
esac
prefix="voidfs-bench/credentials-check-$(date -u +%Y%m%dT%H%M%SZ)"

echo "# $(date -u +%Y-%m-%dT%H:%M:%SZ), $(git -C "$root" describe --always --dirty), a pool at $prefix/ in the bucket the env file names"
echo "before: $("$bench" purge --prefix voidfs-bench/ 2>&1 | tail -1)"

export VOIDFS_ACCESS_KEY_ID="VF$(random 'A-Z2-7' 18)" VOIDFS_SECRET_ACCESS_KEY="$(random 'A-Za-z0-9' 40)"
export VOIDFS_READ_ACCESS_KEY_ID="VR$(random 'A-Z2-7' 18)" VOIDFS_READ_SECRET_ACCESS_KEY="$(random 'A-Za-z0-9' 40)"
needs_purge=1
RUST_LOG=warn,voidfs_server=info "$server" --store "s3:$VOIDFS_S3_BUCKET/$prefix" --listen "127.0.0.1:$port" \
    --key "$VOIDFS_READ_ACCESS_KEY_ID:$VOIDFS_READ_SECRET_ACCESS_KEY:read" > "$work/voidfs.log" 2>&1 &
pid=$!
ready=0
for _ in $(seq 1 300); do
    if curl -s -o /dev/null "http://127.0.0.1:$port/"; then
        ready=1
        break
    fi
    if ! kill -0 "$pid" 2> /dev/null; then
        echo "the server stopped:" >&2
        sed -E 's/(ASIA|AKIA)[A-Z0-9]{12,}/<key id>/g' "$work/voidfs.log" | tail -5 >&2
        exit 1
    fi
    sleep 0.1
done
[[ "$ready" == 1 ]] || { echo "the server did not answer within 30 seconds" >&2; exit 1; }
echo "start-up: $(grep -h 'storage credentials' "$work/voidfs.log" | sed -E 's/^.*(storage credentials)/\1/' | head -1)"
failed=0
if ! grep -q "Storage credentials are offered" "$work/voidfs.log"; then
    echo "FAIL: this check requires the server to offer storage credentials" >&2
    failed=1
fi
if ! VOIDFS_ENDPOINT="http://127.0.0.1:$port" "$conformance" --case storage-credentials | tee "$work/conformance.log"; then
    failed=1
fi
for case_id in storage-credentials-describe-the-drives-storage storage-credentials-read-the-drive storage-credentials-reach-no-further storage-credentials-for-read-keys; do
    if ! grep -Eq "^PASS[[:space:]]+$case_id[[:space:]]" "$work/conformance.log"; then
        echo "FAIL: $case_id must pass against this bucket" >&2
        failed=1
    fi
done
kill "$pid" 2> /dev/null || true
wait "$pid" 2> /dev/null || true
pid=""
if ! "$server" --store "s3:$VOIDFS_S3_BUCKET/$prefix" probe > "$work/probe.log" 2>&1; then
    failed=1
fi
grep -E '^(pool|storage credentials|create-if-absent) ' "$work/probe.log" || true
if ! grep -q "Storage credentials are offered" "$work/probe.log"; then
    echo "FAIL: probe must confirm that storage credentials are offered" >&2
    failed=1
fi
purge_pool || failed=1
exit "$failed"
