#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Exercises a voidfs server with stock boto3, the way real programs use S3.

    VOIDFS_ENDPOINT=http://127.0.0.1:9000 VOIDFS_ACCESS_KEY_ID=... VOIDFS_SECRET_ACCESS_KEY=... \
        python3 tests/interop/boto3_smoke.py

Needs `pip install boto3`. Exits non-zero at the first failure.
"""

import hashlib
import io
import os
import sys
import tempfile
import urllib.request
import uuid

import boto3
from boto3.s3.transfer import TransferConfig
from botocore.config import Config
from botocore.exceptions import ClientError

endpoint = os.environ["VOIDFS_ENDPOINT"]
s3 = boto3.client(
    "s3",
    endpoint_url=endpoint,
    aws_access_key_id=os.environ["VOIDFS_ACCESS_KEY_ID"],
    aws_secret_access_key=os.environ["VOIDFS_SECRET_ACCESS_KEY"],
    region_name="us-east-1",
    # voidfs accepts only Signature Version 4, which boto3 does not use for presigned URLs
    # unless asked to.
    config=Config(signature_version="s3v4", s3={"addressing_style": "path"}, retries={"max_attempts": 1}),
)
bucket = f"interop-{uuid.uuid4().hex[:10]}"
passed = 0


def check(name, cond, detail=""):
    global passed
    if not cond:
        print(f"FAIL  {name} {detail}")
        sys.exit(1)
    passed += 1
    print(f"ok    {name}")


def error_code(fn):
    try:
        fn()
    except ClientError as e:
        return e.response["Error"]["Code"]
    return None


s3.create_bucket(Bucket=bucket)
s3.head_bucket(Bucket=bucket)
check("create and head bucket", True)
check("list buckets", bucket in [b["Name"] for b in s3.list_buckets()["Buckets"]])
check("versioning enabled", s3.get_bucket_versioning(Bucket=bucket)["Status"] == "Enabled")

r = s3.put_object(Bucket=bucket, Key="hello.txt", Body=b"hello world", ContentType="text/plain", Metadata={"owner": "me"})
v1 = r["VersionId"]
g = s3.get_object(Bucket=bucket, Key="hello.txt")
check("put and get", g["Body"].read() == b"hello world" and g["ContentType"] == "text/plain" and g["Metadata"] == {"owner": "me"})
check("ranged get", s3.get_object(Bucket=bucket, Key="hello.txt", Range="bytes=6-10")["Body"].read() == b"world")
h = s3.head_object(Bucket=bucket, Key="hello.txt")
check("head", h["ContentLength"] == 11 and h["VersionId"] == v1)

for algo in ["CRC32", "CRC32C", "SHA256", "SHA1", "CRC64NVME"]:
    try:
        s3.put_object(Bucket=bucket, Key=f"sum-{algo}.bin", Body=os.urandom(3000), ChecksumAlgorithm=algo)
        check(f"put with {algo} checksum", True)
    except ClientError as e:
        check(f"put with {algo} checksum", False, str(e))
    except Exception as e:  # older botocore may not know an algorithm
        print(f"skip  put with {algo} checksum ({type(e).__name__})")

check("conditional put refused", error_code(lambda: s3.put_object(Bucket=bucket, Key="hello.txt", Body=b"x", IfNoneMatch="*")) == "PreconditionFailed")

data = os.urandom(20 * 1024 * 1024 + 12345)
with tempfile.TemporaryDirectory() as tmp:
    src = os.path.join(tmp, "big.bin")
    with open(src, "wb") as f:
        f.write(data)
    cfg = TransferConfig(multipart_threshold=8 * 1024 * 1024, multipart_chunksize=8 * 1024 * 1024)
    s3.upload_file(src, bucket, "media/big.bin", Config=cfg)
    dst = os.path.join(tmp, "back.bin")
    s3.download_file(bucket, "media/big.bin", dst, Config=cfg)
    with open(dst, "rb") as f:
        back = f.read()
check("multipart upload and ranged download of 20 MiB", hashlib.sha256(back).digest() == hashlib.sha256(data).digest())
check("multipart is one version", len(s3.list_object_versions(Bucket=bucket, Prefix="media/big.bin")["Versions"]) == 1)

for i in range(5):
    s3.put_object(Bucket=bucket, Key=f"list/item-{i}.txt", Body=b"x")
pages = s3.get_paginator("list_objects_v2").paginate(Bucket=bucket, Prefix="list/", PaginationConfig={"PageSize": 2})
keys = [o["Key"] for p in pages for o in p.get("Contents", [])]
check("paginated listing", keys == [f"list/item-{i}.txt" for i in range(5)], str(keys))
top = s3.list_objects_v2(Bucket=bucket, Delimiter="/")
check("delimited listing", {p["Prefix"] for p in top.get("CommonPrefixes", [])} == {"list/", "media/"})

s3.copy_object(Bucket=bucket, Key="copy.txt", CopySource={"Bucket": bucket, "Key": "hello.txt"})
check("copy", s3.get_object(Bucket=bucket, Key="copy.txt")["Body"].read() == b"hello world")

s3.put_object(Bucket=bucket, Key="hello.txt", Body=b"second version")
vs = s3.list_object_versions(Bucket=bucket, Prefix="hello.txt")["Versions"]
check("version listing", len(vs) == 2 and any(v["IsLatest"] for v in vs))
check("read old version", s3.get_object(Bucket=bucket, Key="hello.txt", VersionId=v1)["Body"].read() == b"hello world")

url = s3.generate_presigned_url("get_object", Params={"Bucket": bucket, "Key": "copy.txt"}, ExpiresIn=300)
check("presigned GET", urllib.request.urlopen(url).read() == b"hello world")

s3.delete_object(Bucket=bucket, Key="copy.txt")
check("delete", error_code(lambda: s3.head_object(Bucket=bucket, Key="copy.txt")) in ("404", "NoSuchKey"))
r = s3.delete_objects(Bucket=bucket, Delete={"Objects": [{"Key": f"list/item-{i}.txt"} for i in range(5)]})
check("delete objects", len(r.get("Deleted", [])) == 5)

up = s3.create_multipart_upload(Bucket=bucket, Key="aborted.bin")
s3.upload_part(Bucket=bucket, Key="aborted.bin", UploadId=up["UploadId"], PartNumber=1, Body=b"x" * 100)
s3.abort_multipart_upload(Bucket=bucket, Key="aborted.bin", UploadId=up["UploadId"])
check("abort multipart", error_code(lambda: s3.head_object(Bucket=bucket, Key="aborted.bin")) in ("404", "NoSuchKey"))

s3.delete_bucket(Bucket=bucket)
check("delete bucket", error_code(lambda: s3.head_bucket(Bucket=bucket)) in ("404", "NoSuchBucket"))
print(f"\n{passed} checks passed")
