#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Uploads `aws-chunked` bodies in the three forms protocol §2 requires, signed by hand, since
SDKs send them only over HTTPS. Also checks that a signature must cover `host`, and that header
authentication must send `x-amz-content-sha256`, which SDKs never leave out.

    VOIDFS_ENDPOINT=http://127.0.0.1:9000 VOIDFS_ACCESS_KEY_ID=... VOIDFS_SECRET_ACCESS_KEY=... \
        python3 tests/interop/aws_chunked.py

With VOIDFS_ADDRESSING=virtual, requests go to `<drive>.<endpoint host>`, for a server started
with `--virtual-host-domain <endpoint host>`. Names under `localhost` connect to the loopback
address, as in curl and browsers, so `http://s3.localhost:9000` needs no DNS. Needs only Python 3. Exits non-zero at the first failure.
"""

import base64
import datetime
import hashlib
import hmac
import http.client
import os
import sys
import urllib.parse
import uuid
import zlib

endpoint = urllib.parse.urlsplit(os.environ["VOIDFS_ENDPOINT"])
key_id = os.environ["VOIDFS_ACCESS_KEY_ID"]
secret = os.environ["VOIDFS_SECRET_ACCESS_KEY"]
virtual = os.environ.get("VOIDFS_ADDRESSING", "path") == "virtual"
drive = f"chunked-{uuid.uuid4().hex[:10]}"
EMPTY = hashlib.sha256(b"").hexdigest()
passed = 0


def check(name, cond, detail=""):
    global passed
    if not cond:
        print(f"FAIL  {name} {detail}")
        sys.exit(1)
    passed += 1
    print(f"ok    {name}")


def hm(k, m):
    return hmac.new(k, m.encode(), hashlib.sha256).digest()


def request(method, key, body=b"", headers=None, payload=None, chunks=None, trailer=None, tamper=False, sign_host=True, payload_header=True):
    """Signs and sends one request. With `chunks`, the body is aws-chunked in the form `payload`
    names, with `trailer` as (name, value) if given; `tamper` corrupts one chunk signature;
    `sign_host=False` leaves `host` out of the signature; `payload_header=False` signs the
    payload's hash without sending x-amz-content-sha256, as curl 7.88 does."""
    host = f"{drive}.{endpoint.netloc}" if virtual else endpoint.netloc
    path = "/" + urllib.parse.quote(key) if virtual else f"/{drive}" + ("/" + urllib.parse.quote(key) if key else "")
    now = datetime.datetime.now(datetime.timezone.utc)
    amz_date, day = now.strftime("%Y%m%dT%H%M%SZ"), now.strftime("%Y%m%d")
    scope = f"{day}/us-east-1/s3/aws4_request"
    h = {"host": host, "x-amz-date": amz_date, **(headers or {})}
    if chunks is not None:
        h["content-encoding"] = "aws-chunked"
        h["x-amz-decoded-content-length"] = str(sum(len(c) for c in chunks))
        if trailer:
            h["x-amz-trailer"] = trailer[0]
    payload_hash = payload or hashlib.sha256(body).hexdigest()
    if payload_header:
        h["x-amz-content-sha256"] = payload_hash
    names = sorted(n for n in h if sign_host or n != "host")
    canonical = "\n".join([method, path, "", *(f"{n}:{h[n]}" for n in names), "", ";".join(names), payload_hash])
    signing_key = hm(hm(hm(hm(("AWS4" + secret).encode(), day), "us-east-1"), "s3"), "aws4_request")
    to_sign = f"AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hashlib.sha256(canonical.encode()).hexdigest()}"
    seed = hmac.new(signing_key, to_sign.encode(), hashlib.sha256).hexdigest()
    h["authorization"] = f"AWS4-HMAC-SHA256 Credential={key_id}/{scope}, SignedHeaders={';'.join(names)}, Signature={seed}"
    if chunks is not None:
        signed = payload.startswith("STREAMING-AWS4-HMAC-SHA256")
        prev, out = seed, b""
        for i, c in enumerate([*chunks, b""]):
            ext = ""
            if signed:
                s = f"AWS4-HMAC-SHA256-PAYLOAD\n{amz_date}\n{scope}\n{prev}\n{EMPTY}\n{hashlib.sha256(c).hexdigest()}"
                prev = hmac.new(signing_key, s.encode(), hashlib.sha256).hexdigest()
                ext = f";chunk-signature={'0' * 64 if tamper and i == 0 else prev}"
            out += f"{len(c):x}{ext}\r\n".encode() + c + (b"\r\n" if c else b"")
        if trailer:
            line = f"{trailer[0]}:{trailer[1]}"
            out += line.encode() + b"\r\n"
            if signed:
                s = f"AWS4-HMAC-SHA256-TRAILER\n{amz_date}\n{scope}\n{prev}\n{hashlib.sha256((line + chr(10)).encode()).hexdigest()}"
                out += f"x-amz-trailer-signature:{hmac.new(signing_key, s.encode(), hashlib.sha256).hexdigest()}\r\n".encode()
        body = out + b"\r\n"
    address = "127.0.0.1" if endpoint.hostname.endswith(".localhost") else endpoint.hostname
    conn = http.client.HTTPConnection(address, endpoint.port or 80, timeout=30)
    conn.request(method, path, body=body, headers=h)
    r = conn.getresponse()
    data = r.read()
    conn.close()
    return r.status, data


def crc32(data):
    return base64.b64encode(zlib.crc32(data).to_bytes(4, "big")).decode()


status, body = request("PUT", "")
check("create drive", status == 200, body)
content = os.urandom(150_000)
chunks = [content[:65536], content[65536:131072], content[131072:]]
forms = [
    ("STREAMING-UNSIGNED-PAYLOAD-TRAILER", ("x-amz-checksum-crc32", crc32(content))),
    ("STREAMING-AWS4-HMAC-SHA256-PAYLOAD", None),
    ("STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER", ("x-amz-checksum-crc32", crc32(content))),
]
for payload, trailer in forms:
    key = f"up/{payload.lower()}.bin"
    status, body = request("PUT", key, payload=payload, chunks=chunks, trailer=trailer)
    check(f"put {payload}", status == 200, body)
    status, body = request("GET", key)
    check(f"read back {payload}", status == 200 and body == content)
status, body = request("PUT", "up/bad.bin", payload="STREAMING-AWS4-HMAC-SHA256-PAYLOAD", chunks=chunks, tamper=True)
check("a bad chunk signature is refused", status == 403 and b"SignatureDoesNotMatch" in body, body)
status, body = request("PUT", "up/bad.bin", payload="STREAMING-UNSIGNED-PAYLOAD-TRAILER", chunks=chunks, trailer=("x-amz-checksum-crc32", crc32(b"other")))
check("a wrong trailing checksum is refused", status == 400 and b"BadDigest" in body, body)
status, _ = request("GET", "up/bad.bin")
check("and nothing was written", status == 404)
status, body = request("PUT", "up/nohost.txt", body=b"x", sign_host=False)
check("a signature that leaves out host is refused, as S3 refuses it", status == 403 and b"AccessDenied" in body, body)
status, _ = request("GET", "up/nohost.txt")
check("and nothing was written", status == 404)
status, body = request("PUT", "up/nohash.txt", body=b"x", payload_header=False)
check("header auth without x-amz-content-sha256 is refused, as S3 refuses it", status == 400 and b"InvalidRequest" in body, body)
status, _ = request("GET", "up/nohash.txt")
check("and nothing was written", status == 404)
status, _ = request("DELETE", "", headers={"x-voidfs-hard-delete": "true"})
check("hard-delete drive", status == 204)
print(f"\n{passed} checks passed")
