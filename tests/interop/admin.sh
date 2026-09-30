#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Checks the admin listener (`--admin-listen`): /healthz and /readyz, that /metrics has the key
# series, and that it counts what a put and two reads of the same object do. run.sh runs it; by
# hand:
#
#     voidfs-server --store memory --listen 127.0.0.1:9100 --admin-listen 127.0.0.1:9101 ...
#     VOIDFS_ENDPOINT=http://127.0.0.1:9100 VOIDFS_ACCESS_KEY_ID=... VOIDFS_SECRET_ACCESS_KEY=... \
#         tests/interop/admin.sh http://127.0.0.1:9101
#
# Needs curl 8.5 or later (see virtual_host.sh). Exits non-zero at the first failure.

set -euo pipefail

admin=${1:?usage: admin.sh <admin listener URL>}
drive=admin-$(head -c 5 /dev/urandom | od -An -tx1 | tr -d ' \n')
passed=0
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT

# fetch URL [curl options...]: status in $status, body in $out/body, headers in $out/headers.
fetch() {
    local url=$1
    shift
    status=$(curl -sS -o "$out/body" -D "$out/headers" -w '%{http_code}' "$@" "$url")
}

# s3 METHOD PATH [curl options...]: a signed request to the S3 port.
s3() {
    local method=$1 path=$2
    shift 2
    fetch "$VOIDFS_ENDPOINT$path" -X "$method" --aws-sigv4 "aws:amz:us-east-1:s3" \
        --user "$VOIDFS_ACCESS_KEY_ID:$VOIDFS_SECRET_ACCESS_KEY" -H "x-amz-content-sha256: UNSIGNED-PAYLOAD" "$@"
}

check() {
    if ! eval "$2"; then
        echo "FAIL  $1 (status $status): $(head -c 300 "$out/body")"
        exit 1
    fi
    passed=$((passed + 1))
    echo "ok    $1"
}

# scrape: /metrics into $out/metrics. metric SERIES: its value there, or nothing.
scrape() {
    fetch "$admin/metrics"
    cp "$out/body" "$out/metrics"
}
metric() {
    awk -v s="$1" '$1 == s { print $2 }' "$out/metrics"
}

if [[ "${admin#*://}" == "${VOIDFS_ENDPOINT#*://}" ]]; then
    echo "FAIL  the admin listener is the S3 endpoint ($admin)"
    exit 1
fi

fetch "$admin/healthz"
check "/healthz answers" '[ "$status" = 200 ] && [ "$(cat "$out/body")" = ok ]'
fetch "$admin/readyz"
check "/readyz: ready" '[ "$status" = 200 ] && [ "$(cat "$out/body")" = ready ]'
fetch "$VOIDFS_ENDPOINT/healthz"
check "/healthz on the S3 port is a drive's path, not the health check" '[ "$status" != 200 ]'
fetch "$admin/"
check "the admin port does not serve S3" '[ "$status" = 404 ]'

scrape
check "/metrics answers in the text format" '[ "$status" = 200 ] && grep -qi "^content-type: text/plain; version=0.0.4" "$out/headers"'
for series in \
    voidfs_s3_requests_total voidfs_s3_request_duration_seconds_bucket \
    voidfs_bucket_requests_total voidfs_bucket_request_errors_total voidfs_bucket_request_duration_seconds_bucket \
    voidfs_cache_hits_total voidfs_cache_misses_total voidfs_cache_evictions_total voidfs_cache_bytes \
    voidfs_commit_transactions_bucket voidfs_commit_log_write_seconds_bucket voidfs_commits_total \
    voidfs_gc_phase voidfs_drives voidfs_uptime_seconds voidfs_build_info; do
    check "/metrics has $series" 'grep -q "^$series[{ ]" "$out/metrics"'
done
check "every series is voidfs_" '! grep -v "^#" "$out/metrics" | grep -v "^voidfs_" | grep -q .'

s3 PUT "/$drive"
check "create a drive" '[ "$status" = 200 ]'
scrape
hits=$(metric 'voidfs_cache_hits_total{cache="shard"}')
gets=$(metric 'voidfs_s3_requests_total{op="get",status="2xx"}')
puts=$(metric 'voidfs_s3_requests_total{op="put",status="2xx"}')
bucket_gets=$(metric 'voidfs_bucket_requests_total{op="get"}')
commits=$(metric 'voidfs_commits_total{outcome="written"}')
# Over 4 KiB, so that it is stored in a shard even where small files are held in the log
# (inline-data): the reads then show in the shard cache.
printf 'counted by the metrics %.0s' $(seq 1 200) > "$out/counted"
s3 PUT "/$drive/counted.txt" --data-binary @"$out/counted"
check "put an object" '[ "$status" = 200 ]'
for i in 1 2; do
    s3 GET "/$drive/counted.txt"
    check "get it ($i)" '[ "$status" = 200 ] && cmp -s "$out/body" "$out/counted"'
done
# Read into variables first: bash 3.2 brace-expands `{a,b}` in a command substitution under eval.
scrape
hits2=$(metric 'voidfs_cache_hits_total{cache="shard"}')
gets2=$(metric 'voidfs_s3_requests_total{op="get",status="2xx"}')
puts2=$(metric 'voidfs_s3_requests_total{op="put",status="2xx"}')
bucket_gets2=$(metric 'voidfs_bucket_requests_total{op="get"}')
commits2=$(metric 'voidfs_commits_total{outcome="written"}')
live=$(metric 'voidfs_drives{state="live"}')
check "the put is counted" '[ "$puts2" = $((puts + 1)) ]'
check "the gets are counted" '[ "$gets2" = $((gets + 2)) ]'
check "both gets hit the shard cache" '[ "$hits2" -ge $((hits + 2)) ]'
# The server reads gc/pending.json once a minute, which one GET may be.
check "neither went to the bucket" '[ "$bucket_gets2" -le $((bucket_gets + 1)) ]'
check "the put wrote a log entry" '[ "$commits2" = $((commits + 1)) ]'
check "a drive is open" '[ "$live" -ge 1 ]'

s3 DELETE "/$drive/counted.txt"
s3 DELETE "/$drive" -H 'x-voidfs-hard-delete: true'
check "remove the drive" '[ "$status" = 204 ]'
echo "$passed checks passed"
