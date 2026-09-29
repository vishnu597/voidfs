#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Exercises virtual-host addressing (protocol §2) with curl's own SigV4 signing: the extensions,
# forks, copies and drive lifecycle at `<drive>.<domain>`, and path style beside it.
#
#     voidfs-server --store memory --listen 127.0.0.1:9100 --virtual-host-domain s3.localhost ...
#     VOIDFS_ENDPOINT=http://127.0.0.1:9100 VOIDFS_ACCESS_KEY_ID=... VOIDFS_SECRET_ACCESS_KEY=... \
#         tests/interop/virtual_host.sh [domain]
#
# The domain defaults to s3.localhost. Every name under it connects to the endpoint
# (`curl --connect-to`), so no DNS is needed. Needs curl 8.5 or later: 7.88, in Debian 12, signs
# query strings in a way servers reject. Exits non-zero at the first failure.

set -euo pipefail

domain=${1:-s3.localhost}
addr=${VOIDFS_ENDPOINT#*://}
addr=${addr%%/*}
port=${addr##*:}
drive=vh-$(head -c 5 /dev/urandom | od -An -tx1 | tr -d ' \n')
fork=$drive-fork
dotted=$drive.dotted
passed=0
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT

# req METHOD HOST PATH [curl options...]: status in $status, body in $out/body, headers in $out/headers.
req() {
    local method=$1 host=$2 path=$3
    shift 3
    # curl before 8 signs the payload's hash without sending it; name the payload explicitly.
    status=$(curl -sS -o "$out/body" -D "$out/headers" -w '%{http_code}' -X "$method" \
        --aws-sigv4 "aws:amz:us-east-1:s3" --user "$VOIDFS_ACCESS_KEY_ID:$VOIDFS_SECRET_ACCESS_KEY" \
        -H "x-amz-content-sha256: UNSIGNED-PAYLOAD" --connect-to "::$addr" "$@" "http://$host:$port$path")
}

header() {
    tr -d '\r' < "$out/headers" | awk -v name="$1" 'tolower($1) == name ":" { sub(/^[^:]*: /, ""); v = $0 } END { print v }'
}

check() {
    if ! eval "$2"; then
        echo "FAIL  $1 (status $status): $(head -c 300 "$out/body")"
        exit 1
    fi
    passed=$((passed + 1))
    echo "ok    $1"
}

vh=$drive.$domain

req PUT "$vh" /
check "create a drive at its host" '[ "$status" = 200 ] && [ "$(header location)" = / ]'
id=$(header x-voidfs-drive-id)
req HEAD "$vh" /
check "head the drive" '[ "$status" = 200 ]'
req PUT "$vh" "/cuts/a%20b.txt" --data-binary 'hello world' -H 'content-type: text/plain'
check "put an object" '[ "$status" = 200 ]'
req GET "$vh" "/cuts/a%20b.txt"
check "get it at its host" '[ "$status" = 200 ] && [ "$(cat "$out/body")" = "hello world" ]'
req GET "$domain" "/$drive/cuts/a%20b.txt"
check "get it path-style at the bare domain" '[ "$status" = 200 ] && [ "$(cat "$out/body")" = "hello world" ]'
req GET "127.0.0.1" "/$drive/cuts/a%20b.txt"
check "get it path-style at another host" '[ "$status" = 200 ] && [ "$(cat "$out/body")" = "hello world" ]'
req GET "$(echo "$vh" | tr a-z A-Z)." "/cuts/a%20b.txt"
check "host case and a trailing dot" '[ "$status" = 200 ] && [ "$(cat "$out/body")" = "hello world" ]'
req GET "$id.$domain" "/cuts/a%20b.txt"
check "get it at the drive id's host" '[ "$status" = 200 ] && [ "$(cat "$out/body")" = "hello world" ]'
req GET "$vh" "/?list-type=2&prefix=cuts/"
check "list objects" '[ "$status" = 200 ] && grep -q "<Name>$drive</Name>" "$out/body" && grep -q "<Key>cuts/a b.txt</Key>" "$out/body"'
req GET "$domain" /
check "list drives at the bare domain" '[ "$status" = 200 ] && grep -q "<Name>$drive</Name>" "$out/body"'

req PUT "$vh" "/cuts/a%20b.txt?x-voidfs-write" --data-binary 'W' -H 'x-voidfs-offset: 0'
check "write at an offset (§4.1)" '[ "$status" = 200 ]'
req GET "$vh" "/cuts/a%20b.txt?x-voidfs-versions"
check "history (§4.4)" '[ "$status" = 200 ] && grep -q "\"operation\":\"write\"" "$out/body"'
req PUT "$vh" "/cuts/renamed.txt?x-voidfs-rename" -H 'x-voidfs-source: cuts/a%20b.txt'
check "rename (§4.7)" '[ "$status" = 200 ]'
req GET "$vh" "/cuts/renamed.txt"
check "read the renamed object" '[ "$(cat "$out/body")" = "Wello world" ]'
req GET "$vh" "/?x-voidfs-list&prefix=cuts/"
check "list with attributes (§4.9)" '[ "$status" = 200 ] && grep -q "\"name\":\"renamed.txt\"" "$out/body"'
req GET "$vh" "/?x-voidfs-drive"
check "describe (§5.4)" '[ "$status" = 200 ] && grep -q "\"driveId\":\"$id\"" "$out/body"'
req GET "$vh" "/?x-voidfs-changes&since=0"
check "change feed (§5.6)" '[ "$status" = 200 ] && grep -q "\"fromKey\":\"cuts/a b.txt\"" "$out/body"'

req PUT "$fork.$domain" / -H "x-voidfs-fork-source: $drive"
check "fork (§5.2)" '[ "$status" = 200 ] && [ "$(header x-voidfs-fork-source-id)" = "$id" ]'
req PUT "$fork.$domain" /copy.txt -H "x-amz-copy-source: /$drive/cuts/renamed.txt"
check "copy from another drive" '[ "$status" = 200 ]'
req GET "$fork.$domain" /copy.txt
check "read the copy" '[ "$(cat "$out/body")" = "Wello world" ]'

req POST "$vh" "/big.bin?uploads"
upload=$(sed -n 's:.*<UploadId>\(.*\)</UploadId>.*:\1:p' "$out/body")
check "create a multipart upload" '[ "$status" = 200 ] && [ -n "$upload" ] && grep -q "<Bucket>$drive</Bucket>" "$out/body"'
req PUT "$vh" "/big.bin?partNumber=1&uploadId=$upload" --data-binary 'part one'
etag=$(header etag)
req POST "$vh" "/big.bin?uploadId=$upload" --data-binary "<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>$etag</ETag></Part></CompleteMultipartUpload>"
check "complete it: Location is on the drive's host" '[ "$status" = 200 ] && grep -q "<Location>/big.bin</Location>" "$out/body"'

req PUT "$dotted.$domain" /
check "create a drive whose name has dots" '[ "$status" = 200 ]'
req GET "$domain" "/?x-id=ListBuckets"
check "it is the drive named with dots" 'grep -q "<Name>$dotted</Name>" "$out/body"'

req DELETE "$vh" /
check "delete the drive (§5.3)" '[ "$status" = 204 ]'
req POST "$vh" "/?x-voidfs-undelete"
check "undelete it" '[ "$status" = 200 ]'
for d in "$drive" "$fork" "$dotted"; do
    req DELETE "$d.$domain" / -H 'x-voidfs-hard-delete: true'
    check "hard-delete $d" '[ "$status" = 204 ]'
done
echo
echo "$passed checks passed"
