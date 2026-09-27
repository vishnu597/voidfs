#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Fills a drive with the test tree the FSKit spike measures against.

    VOIDFS_ENDPOINT=http://127.0.0.1:9000 VOIDFS_ACCESS_KEY_ID=... VOIDFS_SECRET_ACCESS_KEY=... \
        python3 apps/macos/scripts/seed.py [drive]

Needs `pip install boto3`. Creates, in the drive (default `spike`):

    hello.txt                    a small text file
    many/f0000.txt … f0999.txt   1,000 small files (the listing benchmark)
    many10k/f00000.txt …         10,000 small files (listing across 10 pages)
    media/big.bin                1 GiB of incompressible bytes (the throughput benchmark)
    docs/nested/deep/readme.md   a deep path
    tagged.txt                   a file with extended attributes
    ünïcödé – 名前.txt            a non-ASCII name, stored NFC
"""

import concurrent.futures
import hashlib
import json
import os
import sys
import unicodedata
import urllib.error
import urllib.request

import boto3
from boto3.s3.transfer import TransferConfig
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.config import Config
from botocore.credentials import Credentials

endpoint = os.environ["VOIDFS_ENDPOINT"]
key_id = os.environ["VOIDFS_ACCESS_KEY_ID"]
secret = os.environ["VOIDFS_SECRET_ACCESS_KEY"]
drive = sys.argv[1] if len(sys.argv) > 1 else "spike"
s3 = boto3.client(
    "s3",
    endpoint_url=endpoint,
    aws_access_key_id=key_id,
    aws_secret_access_key=secret,
    region_name="us-east-1",
    config=Config(signature_version="s3v4", s3={"addressing_style": "path"}, max_pool_connections=32),
)


def signed(method, path, body=None, headers=None):
    """Sends a SigV4-signed request for an extension boto3 has no call for."""
    req = AWSRequest(method=method, url=endpoint + path, data=body, headers=headers or {})
    S3SigV4Auth(Credentials(key_id, secret), "s3", "us-east-1").add_auth(req)
    raw = urllib.request.Request(req.url, data=body, method=method, headers=dict(req.headers))
    try:
        with urllib.request.urlopen(raw) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def put_many(prefix, count, width):
    def put(i):
        name = f"{prefix}f{i:0{width}d}.txt"
        s3.put_object(Bucket=drive, Key=name, Body=f"file {i} of {count}\n".encode())

    with concurrent.futures.ThreadPoolExecutor(16) as pool:
        list(pool.map(put, range(count)))
    print(f"{prefix}: {count} files")


try:
    s3.create_bucket(Bucket=drive)
except s3.exceptions.BucketAlreadyOwnedByYou:
    pass

s3.put_object(Bucket=drive, Key="hello.txt", Body=b"hello from voidfs\n")
s3.put_object(Bucket=drive, Key="docs/nested/deep/readme.md", Body=b"# deep\n")
s3.put_object(Bucket=drive, Key=unicodedata.normalize("NFC", "ünïcödé – 名前.txt"), Body=b"unicode\n")
s3.put_object(Bucket=drive, Key="tagged.txt", Body=b"has xattrs\n")
status, body = signed(
    "POST",
    f"/{drive}/tagged.txt?x-voidfs-attrs",
    json.dumps({"xattrs": {"set": {"user.voidfs.test": "aGVsbG8=", "com.apple.metadata:_kMDItemUserTags": "YnBsaXN0MDA="}}}).encode(),
    {"content-type": "application/json"},
)
assert status == 200, (status, body)

put_many("many/", 1000, 4)
put_many("many10k/", 10000, 5)

size = 1 << 30
big = os.path.join(os.environ.get("TMPDIR", "/tmp"), "voidfs-seed-big.bin")
if not os.path.exists(big) or os.path.getsize(big) != size:
    with open(big, "wb") as f:
        for _ in range(size // (8 << 20)):
            f.write(os.urandom(8 << 20))
digest = hashlib.sha256()
with open(big, "rb") as f:
    for block in iter(lambda: f.read(8 << 20), b""):
        digest.update(block)
s3.upload_file(big, drive, "media/big.bin", Config=TransferConfig(multipart_chunksize=64 << 20, max_concurrency=8))
print(f"media/big.bin: {size} bytes, sha256 {digest.hexdigest()}")
print(f"local copy kept at {big} for comparison")
