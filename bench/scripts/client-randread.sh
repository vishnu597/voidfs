#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Measures the client core's block cache (step 4, item 3) on one machine: versitygw serves a
# directory as a bucket, voidfs-server keeps its pool there, and the randread example reads a
# 1 GiB file through it, directly and through the cache, as the Mac head-to-head read through
# the two apps (bench/results/mac-head-to-head).
#
#   cargo build --release -p voidfs-server -p voidfs-bench
#   cargo build --release -p voidfs-client --example randread
#   bench/scripts/client-randread.sh                      # bucket on loopback
#   BENCH_ONE_WAY_MS=6 BENCH_BANDWIDTH=s3 bench/scripts/client-randread.sh
#   BENCH_R2=1 bench/scripts/client-randread.sh           # the pool in the bucket that .env names
#
# Environment:
#   BENCH_ONE_WAY_MS, BENCH_BANDWIDTH   put `voidfs-bench delay` between voidfs-server and the
#                 bucket, as bench/scripts/local.sh does
#   BENCH_READS   random reads per pass (default 300); BENCH_SEED (default 1)
#   BENCH_R2=1    keep the pool in the bucket .env names (VOIDFS_S3_*, region auto), under
#                 voidfs-bench/client-randread-<time>/, instead of versitygw; nothing from .env is
#                 printed. Purge it afterwards: voidfs-bench purge --prefix voidfs-bench/ --yes
#   S3_PORT, VOIDFS_PORT   loopback ports (default 7070 and 9000; the relay uses S3_PORT + 1)

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
s3_port="${S3_PORT:-7070}"
voidfs_port="${VOIDFS_PORT:-9000}"
work="$(mktemp -d "${TMPDIR:-/tmp}/voidfs-randread.XXXXXX")"
mkdir -p "$work/bucket" "$work/logs"
pids=()
cleanup() {
    for p in ${pids[@]+"${pids[@]}"}; do
        kill "$p" 2> /dev/null || true
    done
    wait 2> /dev/null || true
    rm -rf "${work:?}"
}
trap cleanup EXIT

random() { (set +o pipefail; LC_ALL=C tr -dc "$1" < /dev/urandom 2> /dev/null | head -c "$2"); }
wait_for() {
    for _ in $(seq 1 200); do
        curl -s -o /dev/null "$1" && return 0
        sleep 0.05
    done
    echo "nothing answers at $1" >&2
    exit 1
}
s3_key="LOCAL$(random 'A-Z2-7' 15)"
s3_secret="$(random 'A-Za-z0-9' 40)"
vf_key="VF$(random 'A-Z2-7' 18)"
vf_secret="$(random 'A-Za-z0-9' 40)"

if [[ "${BENCH_R2:-0}" == 1 ]]; then
    set -a
    # shellcheck disable=SC1091
    . "$root/.env"
    set +a
    # The server reads the endpoint and key from these, so that none of it is on a command line.
    export VOIDFS_S3_REGION=auto
    store="s3:$VOIDFS_S3_BUCKET/voidfs-bench/client-randread-$(date -u +%Y%m%dT%H%M%SZ)"
    distance="the pool in the R2 bucket .env names, over this Mac's internet connection"
else
    ROOT_ACCESS_KEY_ID="$s3_key" ROOT_SECRET_ACCESS_KEY="$s3_secret" \
        versitygw --port "127.0.0.1:$s3_port" --keep-alive --quiet posix "$work/bucket" > "$work/logs/s3.log" 2>&1 &
    pids+=($!)
    wait_for "http://127.0.0.1:$s3_port/"
    curl -sS -f -o /dev/null --aws-sigv4 "aws:amz:us-east-1:s3" --user "$s3_key:$s3_secret" \
        -H "x-amz-content-sha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" \
        -X PUT "http://127.0.0.1:$s3_port/bench"
    store="s3:bench/voidfs-pool"
    s3_endpoint="http://127.0.0.1:$s3_port"
    s3_region=us-east-1
    distance="none (loopback)"
fi

if [[ "${BENCH_R2:-0}" != 1 && ( -n "${BENCH_ONE_WAY_MS:-}" || -n "${BENCH_BANDWIDTH:-}" ) ]]; then
    bucket_port=$((s3_port + 1))
    relay_args=(--one-way-ms "${BENCH_ONE_WAY_MS:-0}")
    [[ -n "${BENCH_BANDWIDTH:-}" ]] && relay_args+=(--bandwidth "$BENCH_BANDWIDTH")
    "$root/target/release/voidfs-bench" delay --listen "127.0.0.1:$bucket_port" \
        --upstream "127.0.0.1:$s3_port" "${relay_args[@]}" > "$work/logs/delay.log" 2>&1 &
    pids+=($!)
    wait_for "http://127.0.0.1:$bucket_port/"
    s3_endpoint="http://127.0.0.1:$bucket_port"
    distance="voidfs-bench delay ${relay_args[*]} between voidfs-server and the bucket"
fi

s3_args=()
if [[ "${BENCH_R2:-0}" != 1 ]]; then
    s3_args=(--s3-endpoint "$s3_endpoint" --s3-region "$s3_region" --s3-access-key-id "$s3_key" --s3-secret-access-key "$s3_secret")
fi
RUST_LOG=warn "$root/target/release/voidfs-server" \
    --store "$store" --listen "127.0.0.1:$voidfs_port" ${s3_args[@]+"${s3_args[@]}"} \
    --access-key-id "$vf_key" --secret-access-key "$vf_secret" \
    > "$work/logs/voidfs.log" 2>&1 &
server=$!
pids+=($server)
wait_for "http://127.0.0.1:$voidfs_port/"

echo "# $(date -u +%Y-%m-%dT%H:%M:%SZ), $(git -C "$root" rev-parse --short HEAD), $(sysctl -n hw.ncpu 2> /dev/null || nproc) CPUs, distance to the bucket: $distance"
VOIDFS_ENDPOINT="http://127.0.0.1:$voidfs_port" VOIDFS_ACCESS_KEY_ID="$vf_key" VOIDFS_SECRET_ACCESS_KEY="$vf_secret" \
    "$root/target/release/examples/randread" --drive bench --server-pid "$server" \
    --reads "${BENCH_READS:-300}" --seed "${BENCH_SEED:-1}" --state "$work/state"
