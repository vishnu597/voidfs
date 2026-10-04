#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Measures direct uploads (step 4, item 5; protocol §4.11) against ordinary puts: a large file
# re-uploaded with one region changed, alternately as a put and as a direct upload. The client
# reaches the server, and the bucket's presigned URLs, through relays that stand for the client's
# own link, as a client away from the server would; the server reaches the bucket through the same
# relay as the client's direct uploads do.
#
#   cargo build --release -p voidfs-server -p voidfs-bench
#   cargo build --release -p voidfs-sdk --example reupload
#   bench/scripts/direct-reupload.sh                        # 12 ms each way, 20 MB/s up
#   BENCH_FIRST_MULTIPART=1 BENCH_SIZE_MIB=128 bench/scripts/direct-reupload.sh
#   BENCH_R2=1 bench/scripts/direct-reupload.sh             # the pool in the bucket .env names
#
# Environment:
#   BENCH_ONE_WAY_MS   delay each way on the client's link (default 12)
#   BENCH_UPLINK       the client's upload rate in MB/s, per connection and in all (default 20)
#   BENCH_SIZE_MIB     the file (default 32); BENCH_ROUNDS (default 6: 6 puts and 6 direct)
#   BENCH_FIRST_MULTIPART=1   upload the file first in 16 MiB parts
#   BENCH_R2=1         keep the pool in the bucket .env names (VOIDFS_S3_*, region auto), under
#                      voidfs-bench/direct-reupload-<time>/, instead of versitygw: the bucket is
#                      then reached over this Mac's internet connection, by the server and by the
#                      client's direct uploads alike. Nothing from .env is printed. Purge it
#                      afterwards: voidfs-bench purge --prefix voidfs-bench/ --yes. BENCH_ENV names
#                      another env file than the checkout's .env (a worktree has none).
#   S3_PORT, VOIDFS_PORT   loopback ports (default 7170 and 9100; the relays use the next ones)

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
s3_port="${S3_PORT:-7170}"
voidfs_port="${VOIDFS_PORT:-9100}"
one_way="${BENCH_ONE_WAY_MS:-12}"
uplink="${BENCH_UPLINK:-20}"
work="$(mktemp -d "${TMPDIR:-/tmp}/voidfs-reupload.XXXXXX")"
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
relay() { # listen-port upstream-port
    "$root/target/release/voidfs-bench" delay --listen "127.0.0.1:$1" --upstream "127.0.0.1:$2" \
        --one-way-ms "$one_way" --bandwidth="-/$uplink/$uplink" > "$work/logs/delay-$1.log" 2>&1 &
    pids+=($!)
    wait_for "http://127.0.0.1:$1/"
}
s3_key="LOCAL$(random 'A-Z2-7' 15)"
s3_secret="$(random 'A-Za-z0-9' 40)"
vf_key="VF$(random 'A-Z2-7' 18)"
vf_secret="$(random 'A-Za-z0-9' 40)"

s3_args=()
if [[ "${BENCH_R2:-0}" == 1 ]]; then
    set -a
    # shellcheck disable=SC1090
    . "${BENCH_ENV:-$root/.env}"
    set +a
    export VOIDFS_S3_REGION=auto
    store="s3:$VOIDFS_S3_BUCKET/voidfs-bench/direct-reupload-$(date -u +%Y%m%dT%H%M%SZ)"
    bucket="the R2 bucket .env names, over this Mac's internet connection, for the server and the direct uploads alike"
else
    ROOT_ACCESS_KEY_ID="$s3_key" ROOT_SECRET_ACCESS_KEY="$s3_secret" \
        versitygw --port "127.0.0.1:$s3_port" --keep-alive --quiet posix "$work/bucket" > "$work/logs/s3.log" 2>&1 &
    pids+=($!)
    wait_for "http://127.0.0.1:$s3_port/"
    curl -sS -f -o /dev/null --aws-sigv4 "aws:amz:us-east-1:s3" --user "$s3_key:$s3_secret" \
        -H "x-amz-content-sha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" \
        -X PUT "http://127.0.0.1:$s3_port/bench"
    # The bucket behind a relay: the URLs the server presigns name it, so direct uploads cross it.
    relay $((s3_port + 1)) "$s3_port"
    store="s3:bench/voidfs-pool"
    s3_args=(--s3-endpoint "http://127.0.0.1:$((s3_port + 1))" --s3-region us-east-1 --s3-access-key-id "$s3_key" --s3-secret-access-key "$s3_secret")
    bucket="versitygw behind a relay, ${one_way} ms each way and ${uplink} MB/s up, for the server and the direct uploads alike"
fi

RUST_LOG=warn "$root/target/release/voidfs-server" \
    --store "$store" --listen "127.0.0.1:$voidfs_port" ${s3_args[@]+"${s3_args[@]}"} \
    --access-key-id "$vf_key" --secret-access-key "$vf_secret" \
    > "$work/logs/voidfs.log" 2>&1 &
pids+=($!)
wait_for "http://127.0.0.1:$voidfs_port/"
# The client's link to the server.
relay $((voidfs_port + 1)) "$voidfs_port"
# A server that doesn't offer direct uploads says why, as a warning.
if grep -q "presigned PUTs" "$work/logs/voidfs.log"; then
    grep "presigned PUTs" "$work/logs/voidfs.log" | sed 's/^.*presigned PUTs/presigned PUTs/' >&2
    exit 1
fi

echo "# $(date -u +%Y-%m-%dT%H:%M:%SZ), $(git -C "$root" rev-parse --short HEAD), the client's link: ${one_way} ms each way, ${uplink} MB/s up; the bucket: $bucket"
extra=()
[[ "${BENCH_FIRST_MULTIPART:-0}" == 1 ]] && extra+=(--first-multipart)
VOIDFS_ENDPOINT="http://127.0.0.1:$((voidfs_port + 1))" VOIDFS_ACCESS_KEY_ID="$vf_key" VOIDFS_SECRET_ACCESS_KEY="$vf_secret" \
    "$root/target/release/examples/reupload" --size-mib "${BENCH_SIZE_MIB:-32}" --rounds "${BENCH_ROUNDS:-6}" ${extra[@]+"${extra[@]}"}
