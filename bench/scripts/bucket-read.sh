#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Measures reading straight from the bucket with storage credentials (step 4, item 6; protocol
# §5.5) against reading through the server: a file read cold, from start to end, alternately each
# way. The client reaches the server through a relay that stands for its own link, and the bucket
# through another such relay, which the server's own requests to the bucket cross too: through the
# server the bytes cross both, from the bucket one.
#
#   cargo build --release -p voidfs-server -p voidfs-bench
#   cargo build --release -p voidfs-client --example bucketread
#   bench/scripts/bucket-read.sh                 # MinIO (on PATH) behind relays
#   BENCH_AWS=1 bench/scripts/bucket-read.sh     # the pool in the AWS bucket .env.aws names
#
# Environment:
#   BENCH_ONE_WAY_MS   delay each way on each relay (default 12)
#   BENCH_DOWNLINK     MB/s down, per connection and in all, on each relay (default 40)
#   BENCH_SIZE_MIB     the file (default 64); BENCH_ROUNDS (default 3: each way three times)
#   BENCH_AWS=1        keep the pool in the AWS bucket that BENCH_ENV names (default the checkout's
#                      .env.aws: VOIDFS_S3_*, and VOIDFS_STORAGE_CREDENTIALS_ROLE, the role storage
#                      credentials are minted from), under voidfs-bench/bucket-read-<time>/,
#                      instead of MinIO. The server and the client then reach the bucket over this
#                      machine's internet connection, with no relay. Nothing from the env file is
#                      printed. Purge it afterwards: voidfs-bench purge --prefix voidfs-bench/ --yes
#   S3_PORT, VOIDFS_PORT   loopback ports (default 7270 and 9300; the relays use the next ones)

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
s3_port="${S3_PORT:-7270}"
voidfs_port="${VOIDFS_PORT:-9300}"
one_way="${BENCH_ONE_WAY_MS:-12}"
downlink="${BENCH_DOWNLINK:-40}"
work="$(mktemp -d "${TMPDIR:-/tmp}/voidfs-bucket-read.XXXXXX")"
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
    for _ in $(seq 1 300); do
        curl -s -o /dev/null "$1" && return 0
        sleep 0.05
    done
    echo "nothing answers at $1" >&2
    exit 1
}
relay() { # listen-port upstream-port
    "$root/target/release/voidfs-bench" delay --listen "127.0.0.1:$1" --upstream "127.0.0.1:$2" \
        --one-way-ms "$one_way" --bandwidth="$downlink/-/$downlink" > "$work/logs/delay-$1.log" 2>&1 &
    pids+=($!)
    wait_for "http://127.0.0.1:$1/"
}
vf_key="VF$(random 'A-Z2-7' 18)"
vf_secret="$(random 'A-Za-z0-9' 40)"

s3_args=()
if [[ "${BENCH_AWS:-0}" == 1 ]]; then
    set -a
    # shellcheck disable=SC1090
    . "${BENCH_ENV:-$root/.env.aws}"
    set +a
    : "${VOIDFS_STORAGE_CREDENTIALS_ROLE:?the env file must name VOIDFS_STORAGE_CREDENTIALS_ROLE, the role storage credentials are minted from}"
    store="s3:$VOIDFS_S3_BUCKET/voidfs-bench/bucket-read-$(date -u +%Y%m%dT%H%M%SZ)"
    bucket="the AWS bucket the env file names, over this machine's internet connection, for the server and the client alike"
else
    s3_key="LOCAL$(random 'A-Z2-7' 15)"
    s3_secret="$(random 'A-Za-z0-9' 40)"
    MINIO_ROOT_USER="$s3_key" MINIO_ROOT_PASSWORD="$s3_secret" \
        minio server --quiet --address "127.0.0.1:$s3_port" "$work/bucket" > "$work/logs/s3.log" 2>&1 &
    pids+=($!)
    for _ in $(seq 1 300); do
        curl -s -f -o /dev/null "http://127.0.0.1:$s3_port/minio/health/ready" && break
        sleep 0.1
    done
    curl -sS -f -o /dev/null --aws-sigv4 "aws:amz:us-east-1:s3" --user "$s3_key:$s3_secret" \
        -H "x-amz-content-sha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" \
        -X PUT "http://127.0.0.1:$s3_port/bench"
    # The bucket behind a relay: the credentials name its endpoint, so the client's reads cross it.
    relay $((s3_port + 1)) "$s3_port"
    store="s3:bench/voidfs-pool"
    s3_args=(--s3-endpoint "http://127.0.0.1:$((s3_port + 1))" --s3-region us-east-1 --s3-access-key-id "$s3_key" --s3-secret-access-key "$s3_secret")
    bucket="MinIO behind a relay, ${one_way} ms each way and ${downlink} MB/s down, for the server and the client alike"
fi

RUST_LOG=warn,voidfs_server=info "$root/target/release/voidfs-server" \
    --store "$store" --listen "127.0.0.1:$voidfs_port" ${s3_args[@]+"${s3_args[@]}"} \
    --access-key-id "$vf_key" --secret-access-key "$vf_secret" \
    > "$work/logs/voidfs.log" 2>&1 &
server_pid=$!
pids+=($server_pid)
wait_for "http://127.0.0.1:$voidfs_port/"
# The client's link to the server.
relay $((voidfs_port + 1)) "$voidfs_port"
if ! grep -q "Storage credentials are offered" "$work/logs/voidfs.log"; then
    grep "storage credentials" "$work/logs/voidfs.log" | sed 's/^.*storage credentials/storage credentials/' >&2
    exit 1
fi

echo "# $(date -u +%Y-%m-%dT%H:%M:%SZ), $(git -C "$root" rev-parse --short HEAD), the client's link: ${one_way} ms each way, ${downlink} MB/s down; the bucket: $bucket"
VOIDFS_ENDPOINT="http://127.0.0.1:$((voidfs_port + 1))" VOIDFS_ACCESS_KEY_ID="$vf_key" VOIDFS_SECRET_ACCESS_KEY="$vf_secret" \
    "$root/target/release/examples/bucketread" --drive bench --size-mib "${BENCH_SIZE_MIB:-64}" --rounds "${BENCH_ROUNDS:-3}" --server-pid "$server_pid"
