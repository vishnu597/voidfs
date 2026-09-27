#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Runs all 49 scenarios for the real comparison, from the client VM. See bench/README.md,
# "The real run", for the machines and the bucket this expects.
#
#   BENCH_TOPOLOGY=client-host bench/scripts/cloud-run.sh      # SpaceFS's setup
#   BENCH_TOPOLOGY=server-us-east-1 bench/scripts/cloud-run.sh
#
# BENCH_TOPOLOGY says where voidfs-server runs:
#   client-host       on this VM, beside the harness, over the same bucket. This is how
#                     SpaceFS ran their layer ("s3sdk on the client host over that same bucket,
#                     512 MB shard cache"), so only this run gets the column that sets voidfs's
#                     speed-up against theirs.
#   server-us-east-1  on a machine in AWS us-east-1 next to the bucket, as the plan in
#                     docs/PARITY.md §7 has it. VOIDFS_ENDPOINT points at it.
#
# Environment (secrets from the environment only):
#   VOIDFS_ENDPOINT                default http://127.0.0.1:9000
#   VOIDFS_ACCESS_KEY_ID, VOIDFS_SECRET_ACCESS_KEY           the server's admin key
#   VOIDFS_S3_BUCKET, VOIDFS_S3_ACCESS_KEY_ID, VOIDFS_S3_SECRET_ACCESS_KEY, VOIDFS_S3_REGION
#                                  the same bucket, reached directly (the bare target)
#   BENCH_SERVER_HOST              for server-us-east-1: a description, e.g. "EC2 m7i.2xlarge"
#   BENCH_BUCKET_DESC              the bucket, when it is not AWS S3 (e.g. "Cloudflare R2"). The
#                                  run is then not SpaceFS's setup, and gets no parity column
#   BENCH_NAME                     base name of the result files
# Extra arguments go to `voidfs-bench run` (for example --scenario, --ops-scale).

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
topology="${BENCH_TOPOLOGY:?set BENCH_TOPOLOGY to client-host or server-us-east-1}"
for v in VOIDFS_ACCESS_KEY_ID VOIDFS_SECRET_ACCESS_KEY VOIDFS_S3_BUCKET VOIDFS_S3_ACCESS_KEY_ID VOIDFS_S3_SECRET_ACCESS_KEY; do
    [[ -n "${!v:-}" ]] || { echo "$v is not set" >&2; exit 1; }
done
export VOIDFS_ENDPOINT="${VOIDFS_ENDPOINT:-http://127.0.0.1:9000}"
export VOIDFS_S3_REGION="${VOIDFS_S3_REGION:-us-east-1}"

# Describe the client from the GCE metadata server when there is one.
meta() { curl -sf -m 2 -H 'Metadata-Flavor: Google' "http://metadata.google.internal/computeMetadata/v1/instance/$1" | sed 's#.*/##'; }
machine="$(meta machine-type || true)"
zone="$(meta zone || true)"
os="$(. /etc/os-release 2> /dev/null && echo "$PRETTY_NAME" || uname -sr)"
client="${machine:+GCP $machine, $zone, }$os, $(nproc 2> /dev/null || sysctl -n hw.ncpu) vCPUs"

case "$topology" in
    client-host)
        server="voidfs-server on the client host, over that same bucket, 512 MiB shard cache"
        extra=()
        [[ -z "${BENCH_BUCKET_DESC:-}" ]] && extra=(--like-spacefs)
        ;;
    server-us-east-1)
        server="voidfs-server in AWS us-east-1 beside the bucket (${BENCH_SERVER_HOST:-host not described}), reached at $VOIDFS_ENDPOINT"
        extra=()
        ;;
    *)
        echo "BENCH_TOPOLOGY must be client-host or server-us-east-1" >&2
        exit 1
        ;;
esac

cargo build --release --quiet --manifest-path "$root/Cargo.toml" -p voidfs-bench
curl -s -o /dev/null -m 5 "$VOIDFS_ENDPOINT/" || { echo "no voidfs server answers at $VOIDFS_ENDPOINT" >&2; exit 1; }

commit="$(git -C "$root" rev-parse --short HEAD)"
git -C "$root" diff --quiet HEAD -- crates || commit="$commit (with uncommitted changes)"
"$root/target/release/voidfs-bench" run \
    --out-dir "$root/bench/results" \
    --redact \
    --name "${BENCH_NAME:-cloud-$topology-$(date -u +%Y%m%dT%H%M%SZ)}" \
    --label "client=$client" \
    --label "bucket=${BENCH_BUCKET_DESC:-AWS S3, $VOIDFS_S3_REGION, native conditional writes}" \
    --label "voidfs server=$server" \
    --label "commit=$commit" \
    ${extra[@]+"${extra[@]}"} \
    "$@"
