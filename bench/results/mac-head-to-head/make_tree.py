#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""seed.py's tree, written to a local folder so it can be copied into any drive.
    make_tree.py <dest>    # creates <dest>/h2h/...
Same names and contents as apps/macos/scripts/seed.py, except tagged.txt's Finder tag is a
valid binary plist (["h2h"]) so Get Info has something to show."""
import hashlib, os, plistlib, subprocess, sys, unicodedata
root = os.path.join(sys.argv[1], "h2h")
def put(rel, data):
    p = os.path.join(root, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb") as f:
        f.write(data)
    return p
put("hello.txt", b"hello from voidfs\n")
put("docs/nested/deep/readme.md", b"# deep\n")
put(unicodedata.normalize("NFC", "ünïcödé – 名前.txt"), b"unicode\n")
t = put("tagged.txt", b"has xattrs\n")
subprocess.check_call(["/usr/bin/xattr", "-w", "user.voidfs.test", "hello", t])
tags = plistlib.dumps(["h2h"], fmt=plistlib.FMT_BINARY).hex()
subprocess.check_call(["/usr/bin/xattr", "-wx", "com.apple.metadata:_kMDItemUserTags", tags, t])
for prefix, count, width in (("many/", 1000, 4), ("many10k/", 10000, 5)):
    for i in range(count):
        put(f"{prefix}f{i:0{width}d}.txt", f"file {i} of {count}\n".encode())
big = os.path.join(root, "media", "big.bin")
os.makedirs(os.path.dirname(big), exist_ok=True)
h = hashlib.sha256()
with open(big, "wb") as f:
    for _ in range((1 << 30) // (8 << 20)):
        b = os.urandom(8 << 20)
        h.update(b)
        f.write(b)
print("media/big.bin sha256", h.hexdigest())
