#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Runs the benchmark on one machine, with no cloud account: a local S3 server (versitygw, or
# MinIO where it runs) serves a directory as a bucket, voidfs-server keeps its pool in that
# bucket, and the harness measures voidfs and the bare bucket back to back.
#
#   bench/scripts/local.sh                        # all 49 scenarios
#   bench/scripts/local.sh --scenario get-4k      # extra arguments go to `voidfs-bench run`
#   BENCH_S3=minio bench/scripts/local.sh --ops-scale 0.1
#   BENCH_ONE_WAY_MS=6 bench/scripts/local.sh     # the bucket 12 ms away (round trip)
#
# Environment:
#   BENCH_S3      versitygw (default) or minio
#   BENCH_WORK    working directory for the bucket and logs (default: a new temporary one,
#                 deleted afterwards unless BENCH_KEEP=1)
#   BENCH_OUT     where results go (default: bench/results)
#   BENCH_NAME    base name of the result files (default: local-<server>-<run id>)
#   BENCH_ONE_WAY_MS  emulate a distant bucket: both voidfs-server and the harness's bare
#                 target reach it through `voidfs-bench delay`, which adds this many ms each way.
#                 The harness reaches voidfs-server directly, as SpaceFS's harness reached its
#                 layer on the client host.
#   BENCH_BANDWIDTH  also cap the bandwidth through that relay, for both: `s3`, fitted to the
#                 bare bucket in SpaceFS's run, or DOWN/UP/TOTAL in MB/s (per connection down
#                 and up, and in all each way; - for no limit). Off by default
#   BENCH_COLD=1  measure cold reads: the harness's --cold, which drops voidfs-server's caches
#                 (SIGUSR1) before each wave of operations, and confirms each drop from its
#                 metrics (its admin listener on VOIDFS_PORT + 1)
#   BENCH_SERVER_BIN  a voidfs-server binary to use instead of this checkout's release build,
#                 for comparing server changes
#   BENCH_POOL_FEATURES  features of the on-bucket format the pool is created with, for example
#                 inline-data. Passed as VOIDFS_NEW_POOL_FEATURES, which a server that does not
#                 implement it ignores: comparing builds, set it for every run, and each run's
#                 new pool gets it only from a build that has it. The results label what it got
#   BENCH_BUCKET_REQUESTS=1  also record voidfs's requests to the bucket in each scenario's
#                 measured rounds, from the server's metrics (its admin listener on VOIDFS_PORT + 1)
#   S3_PORT, VOIDFS_PORT   loopback ports (default 7070 and 9000; the relay uses S3_PORT + 1)
#
# Loopback numbers are not comparable with SpaceFS's cloud run: there is no network latency,
# and the bucket is a local disk. See bench/README.md.

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
s3="${BENCH_S3:-versitygw}"
s3_port="${S3_PORT:-7070}"
voidfs_port="${VOIDFS_PORT:-9000}"
out="${BENCH_OUT:-$root/bench/results}"
if [[ -n "${BENCH_WORK:-}" ]]; then
    work="$BENCH_WORK"
    mkdir -p "$work"
else
    work="$(mktemp -d "${TMPDIR:-/tmp}/voidfs-bench.XXXXXX")"
fi
mkdir -p "$work/bucket" "$work/logs"

# `tr` gets SIGPIPE when `head` has enough; that is expected, so not under pipefail.
random() { (set +o pipefail; LC_ALL=C tr -dc "$1" < /dev/urandom 2> /dev/null | head -c "$2"); }
s3_key="LOCAL$(random 'A-Z2-7' 15)"
s3_secret="$(random 'A-Za-z0-9' 40)"
vf_key="VF$(random 'A-Z2-7' 18)"
vf_secret="$(random 'A-Za-z0-9' 40)"

pids=()
cleanup() {
    for p in "${pids[@]:-}"; do
        [[ -n "$p" ]] && kill "$p" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    if [[ "${BENCH_KEEP:-0}" != 1 && -z "${BENCH_WORK:-}" ]]; then
        rm -rf "$work"
    else
        echo "kept $work" >&2
    fi
}
trap cleanup EXIT

wait_for() {
    for _ in $(seq 1 100); do
        curl -s -o /dev/null "$1" && return 0
        sleep 0.1
    done
    echo "nothing answered at $1; see $work/logs" >&2
    exit 1
}

echo "building release binaries" >&2
cargo build --release --quiet --manifest-path "$root/Cargo.toml" -p voidfs-server -p voidfs-bench

case "$s3" in
    versitygw)
        command -v versitygw > /dev/null || { echo "needs versitygw: brew install versitygw" >&2; exit 1; }
        s3_version="versitygw $(versitygw --version | awk '/Version/ {print $3}')"
        # Keep-alive is off by default in versitygw; SpaceFS's bare side used keep-alive.
        ROOT_ACCESS_KEY_ID="$s3_key" ROOT_SECRET_ACCESS_KEY="$s3_secret" \
            versitygw --port "127.0.0.1:$s3_port" --keep-alive --quiet posix "$work/bucket" \
            > "$work/logs/s3.log" 2>&1 &
        ;;
    minio)
        command -v minio > /dev/null || { echo "needs minio" >&2; exit 1; }
        s3_version="$(minio --version | head -1)"
        MINIO_ROOT_USER="$s3_key" MINIO_ROOT_PASSWORD="$s3_secret" \
            minio server --quiet --address "127.0.0.1:$s3_port" "$work/bucket" > "$work/logs/s3.log" 2>&1 &
        ;;
    *)
        echo "BENCH_S3 must be versitygw or minio" >&2
        exit 1
        ;;
esac
pids+=($!)
wait_for "http://127.0.0.1:$s3_port/"
# Older curl doesn't send the payload hash, which versitygw requires: this is the empty body's.
curl -sS -f -o /dev/null --aws-sigv4 "aws:amz:us-east-1:s3" --user "$s3_key:$s3_secret" \
    -H "x-amz-content-sha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" \
    -X PUT "http://127.0.0.1:$s3_port/bench"

bucket_port="$s3_port"
distance="none (loopback)"
if [[ -n "${BENCH_ONE_WAY_MS:-}" || -n "${BENCH_BANDWIDTH:-}" ]]; then
    bucket_port=$((s3_port + 1))
    relay_args=(--one-way-ms "${BENCH_ONE_WAY_MS:-0}")
    [[ -n "${BENCH_BANDWIDTH:-}" ]] && relay_args+=(--bandwidth "$BENCH_BANDWIDTH")
    "$root/target/release/voidfs-bench" delay --listen "127.0.0.1:$bucket_port" \
        --upstream "127.0.0.1:$s3_port" "${relay_args[@]}" > "$work/logs/delay.log" 2>&1 &
    pids+=($!)
    wait_for "http://127.0.0.1:$bucket_port/"
    distance="emulated with \`voidfs-bench delay ${relay_args[*]}\` between the bucket and both voidfs-server and the harness's bare target (timer granularity adds about 2 ms each way; the bare head row shows the real round trip). The harness reaches voidfs-server over loopback, as SpaceFS's reached its layer on the client host"
fi
labels=()
if [[ -n "${BENCH_BANDWIDTH:-}" ]]; then
    # The relay describes the cap once it listens.
    for _ in $(seq 1 20); do
        rate="$(sed -n 's/.*ms each way, //p' "$work/logs/delay.log")"
        [[ -n "$rate" ]] && break
        sleep 0.05
    done
    labels+=(--label "bandwidth to the bucket=capped by that relay (--bandwidth $BENCH_BANDWIDTH): ${rate:-see its log}")
fi

admin_args=()
bench_args=()
if [[ "${BENCH_BUCKET_REQUESTS:-0}" == 1 || "${BENCH_COLD:-0}" == 1 ]]; then
    admin_port=$((voidfs_port + 1))
    admin_args=(--admin-listen "127.0.0.1:$admin_port")
    bench_args=(--voidfs-metrics "http://127.0.0.1:$admin_port/metrics")
fi

VOIDFS_NEW_POOL_FEATURES="${BENCH_POOL_FEATURES:-}" RUST_LOG=warn "${BENCH_SERVER_BIN:-$root/target/release/voidfs-server}" \
    --store s3:bench/voidfs-pool --listen "127.0.0.1:$voidfs_port" \
    --s3-endpoint "http://127.0.0.1:$bucket_port" --s3-region us-east-1 \
    --s3-access-key-id "$s3_key" --s3-secret-access-key "$s3_secret" \
    --access-key-id "$vf_key" --secret-access-key "$vf_secret" \
    ${admin_args[@]+"${admin_args[@]}"} \
    > "$work/logs/voidfs.log" 2>&1 &
pids+=($!)
[[ "${BENCH_COLD:-0}" == 1 ]] && bench_args+=(--cold --voidfs-pid "$!")
wait_for "http://127.0.0.1:$voidfs_port/"

# The features the pool was created with, from its descriptor in the bucket's directory.
pool_features=unknown
if [[ -f "$work/bucket/bench/voidfs-pool/voidfs.json" ]]; then
    pool_features="$(tr -d ' \n' < "$work/bucket/bench/voidfs-pool/voidfs.json" | sed -n 's/.*"incompatible":\[\([^]]*\)\].*/\1/p' | tr -d '"')"
    pool_features="${pool_features:-none}"
fi

commit="$(git -C "$root" rev-parse --short HEAD)"
git -C "$root" diff --quiet HEAD -- crates || commit="$commit (with uncommitted changes)"
cpus="$(sysctl -n hw.ncpu 2> /dev/null || nproc)"
name="${BENCH_NAME:-local-$s3-$(date -u +%Y%m%dT%H%M%SZ)}"

VOIDFS_ENDPOINT="http://127.0.0.1:$voidfs_port" VOIDFS_ACCESS_KEY_ID="$vf_key" VOIDFS_SECRET_ACCESS_KEY="$vf_secret" \
VOIDFS_S3_ENDPOINT="http://127.0.0.1:$bucket_port" VOIDFS_S3_BUCKET=bench \
VOIDFS_S3_ACCESS_KEY_ID="$s3_key" VOIDFS_S3_SECRET_ACCESS_KEY="$s3_secret" \
    "$root/target/release/voidfs-bench" run \
    --out-dir "$out" --name "$name" \
    --label "setup=one machine over loopback: harness, voidfs-server and the S3 server ($cpus CPUs, $(uname -sm))" \
    --label "bucket=$s3_version on local disk, 127.0.0.1:$s3_port" \
    --label "distance to the bucket=$distance" \
    ${labels[@]+"${labels[@]}"} \
    --label "voidfs server=voidfs-server release build, s3: store in that bucket, 512 MiB shard cache" \
    --label "pool features=$pool_features" \
    --label "commit=$commit" \
    ${bench_args[@]+"${bench_args[@]}"} \
    "$@"
