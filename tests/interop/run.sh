#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Starts voidfs-server over one kind of store, with throwaway keys, and runs the conformance
# suite (path-style and virtual-host style), the stock-client checks and the admin listener's
# checks against it. CI runs it for each store; so can you:
#
#   cargo build -p voidfs-server -p voidfs-conformance
#   tests/interop/run.sh memory        # or fs, versitygw, minio
#
# versitygw and minio must be on PATH for those stores. rclone is needed; boto3 is used if
# python3 can import it (CI sets VOIDFS_INTEROP_REQUIRE_ALL=1 so that nothing is skipped).
#
#   VOIDFS_SERVER_BIN, VOIDFS_CONFORMANCE_BIN   binaries (default: target/debug)
#   S3_PORT, VOIDFS_PORT, ADMIN_PORT            loopback ports (default 7070, 9000 and 9001)
#   VOIDFS_INTEROP_FEATURES   features the pool is created with, which the conformance runner is
#                             told (default inline-data,multi-object-versions; empty for none)

set -euo pipefail

store="${1:?usage: run.sh memory|fs|versitygw|minio}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
server="${VOIDFS_SERVER_BIN:-$root/target/debug/voidfs-server}"
conformance="${VOIDFS_CONFORMANCE_BIN:-$root/target/debug/voidfs-conformance}"
s3_port="${S3_PORT:-7070}"
voidfs_port="${VOIDFS_PORT:-9000}"
admin_port="${ADMIN_PORT:-$((voidfs_port + 1))}"
require_all="${VOIDFS_INTEROP_REQUIRE_ALL:-0}"
features="${VOIDFS_INTEROP_FEATURES-inline-data,multi-object-versions}"

work="$(mktemp -d "${TMPDIR:-/tmp}/voidfs-interop.XXXXXX")"
pids=()
cleanup() {
    for p in ${pids[@]+"${pids[@]}"}; do
        kill "$p" 2> /dev/null || true
    done
    wait 2> /dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT

# `tr` gets SIGPIPE when `head` has enough; that is expected, so not under pipefail.
random() { (set +o pipefail; LC_ALL=C tr -dc "$1" < /dev/urandom 2> /dev/null | head -c "$2"); }
wait_for() {
    for _ in $(seq 1 100); do
        curl -s -o /dev/null "$1" && return 0
        sleep 0.1
    done
    echo "nothing answered at $1; logs:" >&2
    tail -n 20 "$work"/*.log >&2 || true
    exit 1
}

s3_key="LOCAL$(random 'A-Z2-7' 15)"
s3_secret="$(random 'A-Za-z0-9' 40)"
store_args=()
case "$store" in
    memory) store_args=(--store memory) ;;
    fs) store_args=(--store "fs:$work/pool") ;;
    versitygw | minio)
        mkdir -p "$work/bucket"
        if [[ "$store" == versitygw ]]; then
            ROOT_ACCESS_KEY_ID="$s3_key" ROOT_SECRET_ACCESS_KEY="$s3_secret" \
                versitygw --port "127.0.0.1:$s3_port" --keep-alive --quiet posix "$work/bucket" > "$work/s3.log" 2>&1 &
        else
            MINIO_ROOT_USER="$s3_key" MINIO_ROOT_PASSWORD="$s3_secret" \
                minio server --quiet --address "127.0.0.1:$s3_port" "$work/bucket" > "$work/s3.log" 2>&1 &
        fi
        pids+=($!)
        wait_for "http://127.0.0.1:$s3_port/"
        # MinIO answers, and then resets connections, until it is ready.
        if [[ "$store" == minio ]]; then
            for _ in $(seq 1 300); do
                curl -s -f -o /dev/null "http://127.0.0.1:$s3_port/minio/health/ready" && break
                sleep 0.1
            done
        fi
        # Retry the bucket's creation briefly all the same.
        # Older curl doesn't send the payload hash, which versitygw requires: this is the empty body's.
        created=0
        for _ in $(seq 1 50); do
            if curl -sS -f -o /dev/null --aws-sigv4 "aws:amz:us-east-1:s3" --user "$s3_key:$s3_secret" \
                -H "x-amz-content-sha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" \
                -X PUT "http://127.0.0.1:$s3_port/interop" 2> "$work/create.err"; then
                created=1
                break
            fi
            sleep 0.2
        done
        if [[ "$created" != 1 ]]; then
            echo "could not create the bucket: $(cat "$work/create.err")" >&2
            exit 1
        fi
        store_args=(--store s3:interop/pool --s3-endpoint "http://127.0.0.1:$s3_port")
        export VOIDFS_S3_ACCESS_KEY_ID="$s3_key" VOIDFS_S3_SECRET_ACCESS_KEY="$s3_secret"
        ;;
    *)
        echo "unknown store $store" >&2
        exit 1
        ;;
esac

feature_args=()
for f in ${features//,/ }; do
    feature_args+=(--new-pool-feature "$f")
done
if [[ "$store" != memory && -n "$features" ]]; then
    # There is no pool yet to add a feature to.
    if "$server" "${store_args[@]}" pool enable "${features%%,*}" > "$work/enable.log" 2>&1; then
        echo "pool enable succeeded without a pool" >&2
        exit 1
    fi
    grep -q "no pool here yet" "$work/enable.log" || { cat "$work/enable.log" >&2; exit 1; }
fi

export VOIDFS_ACCESS_KEY_ID="VF$(random 'A-Z2-7' 18)"
export VOIDFS_SECRET_ACCESS_KEY="$(random 'A-Za-z0-9' 40)"
export VOIDFS_READ_ACCESS_KEY_ID="VR$(random 'A-Z2-7' 18)"
export VOIDFS_READ_SECRET_ACCESS_KEY="$(random 'A-Za-z0-9' 40)"
export VOIDFS_ENDPOINT="http://127.0.0.1:$voidfs_port"
if "$server" "${store_args[@]}" --listen "127.0.0.1:$voidfs_port" --admin-listen "0.0.0.0:$voidfs_port" > "$work/same-port.log" 2>&1; then
    echo "the server started with the admin listener on the S3 port" >&2
    exit 1
fi
grep -q "would share the S3 port" "$work/same-port.log" || { cat "$work/same-port.log" >&2; exit 1; }
# Names under localhost reach the loopback address without DNS (curl and these checks see to it).
RUST_LOG=warn,voidfs_server=info "$server" "${store_args[@]}" ${feature_args[@]+"${feature_args[@]}"} --listen "127.0.0.1:$voidfs_port" --virtual-host-domain s3.localhost \
    --admin-listen "127.0.0.1:$admin_port" \
    --key "$VOIDFS_READ_ACCESS_KEY_ID:$VOIDFS_READ_SECRET_ACCESS_KEY:read" > "$work/voidfs.log" 2>&1 &
server_pid=$!
pids+=($server_pid)
wait_for "$VOIDFS_ENDPOINT/"
# Whether the server offers direct uploads (protocol §4.11), and why: what the bucket's presigned
# PUTs bind, which it checks at start.
echo "== direct uploads ($store): $(grep -h -E 'presigned PUTs|direct uploads' "$work/voidfs.log" | sed -E 's/^.*(presigned PUTs|direct uploads)/\1/' | head -1)"

echo "== admin listener ($store)"
"$root/tests/interop/admin.sh" "http://127.0.0.1:$admin_port"
echo "== conformance ($store)"
export VOIDFS_FEATURES="$features"
"$conformance"
echo "== conformance, virtual-host style ($store)"
"$conformance" --virtual-host s3.localhost
echo "== aws-chunked, both styles ($store)"
python3 "$root/tests/interop/aws_chunked.py"
VOIDFS_ADDRESSING=virtual VOIDFS_ENDPOINT="http://s3.localhost:$voidfs_port" python3 "$root/tests/interop/aws_chunked.py"
echo "== virtual-host with curl ($store)"
VOIDFS_ENDPOINT="http://s3.localhost:$voidfs_port" "$root/tests/interop/virtual_host.sh"
echo "== rclone ($store)"
"$root/tests/interop/rclone_smoke.sh"
echo "== boto3 ($store)"
if python3 -c 'import boto3' 2> /dev/null; then
    python3 "$root/tests/interop/boto3_smoke.py"
elif [[ "$require_all" == 1 ]]; then
    echo "boto3 is not installed" >&2
    exit 1
else
    echo "skipped: boto3 is not installed (pip install boto3)"
fi
echo "== SIGTERM stops the server gracefully ($store)"
kill -TERM "$server_pid"
for _ in $(seq 1 50); do
    kill -0 "$server_pid" 2> /dev/null || break
    sleep 0.1
done
if kill -0 "$server_pid" 2> /dev/null; then
    echo "the server did not stop within 5 s of SIGTERM" >&2
    exit 1
fi
status=0
wait "$server_pid" || status=$?
if [[ "$status" != 0 ]]; then
    echo "the server exited with status $status on SIGTERM" >&2
    exit 1
fi
echo "stopped, with status 0"
if [[ "$store" != memory && -n "$features" ]]; then
    echo "== the pool lists ${features%%,*} ($store)"
    "$server" "${store_args[@]}" pool enable "${features%%,*}" | tee "$work/enable.log"
    grep -q "already lists" "$work/enable.log"
fi
