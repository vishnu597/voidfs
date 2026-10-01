#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Starts voidfs-server for a benchmark run, with its pool under a prefix of the bucket the
# bare side of the benchmark also uses. Runs in the foreground; stop it with Ctrl-C.
#
# Credentials come from the environment only, so they never show in `ps`:
#   VOIDFS_S3_BUCKET              the bucket (the same one the harness's bare target uses)
#   VOIDFS_S3_ACCESS_KEY_ID, VOIDFS_S3_SECRET_ACCESS_KEY   bucket credentials
#   VOIDFS_S3_REGION              default us-east-1
#   VOIDFS_S3_ENDPOINT            only for S3-compatible services such as R2; unset for AWS
#   VOIDFS_ACCESS_KEY_ID, VOIDFS_SECRET_ACCESS_KEY         the admin key clients sign with
#                                 (VF + 18 characters of A-Z2-7, and a 40-character secret)
# Options:
#   BENCH_LISTEN        default 127.0.0.1:9000. Use 0.0.0.0:9000 only behind a firewall rule
#                       that admits the client machine alone: the traffic is plain HTTP.
#   BENCH_POOL_PREFIX   default voidfs-bench/pool
#   BENCH_CACHE_MIB     default 512, as in SpaceFS's run
#   BENCH_ADMIN_LISTEN  serve /metrics there too, for example 127.0.0.1:9001, which cold runs
#                       (BENCH_COLD=1 cloud-run.sh) read. Off by default; not authenticated, so
#                       keep it on loopback
#
# voidfs has no garbage collection yet: everything the benchmark writes through voidfs stays
# under the pool prefix until you delete it (see bench/README.md, "Clean up").

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
for v in VOIDFS_S3_BUCKET VOIDFS_S3_ACCESS_KEY_ID VOIDFS_S3_SECRET_ACCESS_KEY VOIDFS_ACCESS_KEY_ID VOIDFS_SECRET_ACCESS_KEY; do
    [[ -n "${!v:-}" ]] || { echo "$v is not set" >&2; exit 1; }
done

cargo build --release --quiet --manifest-path "$root/Cargo.toml" -p voidfs-server
export VOIDFS_S3_REGION="${VOIDFS_S3_REGION:-us-east-1}"
export RUST_LOG="${RUST_LOG:-warn}"
admin=()
[[ -n "${BENCH_ADMIN_LISTEN:-}" ]] && admin=(--admin-listen "$BENCH_ADMIN_LISTEN")
# exec: the server keeps this script's process id, which a cold run signals.
exec "$root/target/release/voidfs-server" \
    --store "s3:$VOIDFS_S3_BUCKET/${BENCH_POOL_PREFIX:-voidfs-bench/pool}" \
    --listen "${BENCH_LISTEN:-127.0.0.1:9000}" \
    --cache-mib "${BENCH_CACHE_MIB:-512}" \
    ${admin[@]+"${admin[@]}"}
