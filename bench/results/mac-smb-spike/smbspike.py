#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""A small read-only SMB 2.1 / 3.x server for step 5's SMB feasibility spike (item 6).

Not a product: just enough of MS-SMB2 for macOS's smbfs to mount a drive on loopback, so that the
client's caches can be measured with full control over what the server tells it (change
notifications, lease breaks, directory times). Standard library only, plus CommonCrypto's AES-CMAC
through ctypes for SMB 3.x signing.

    smbspike.py --port P --user U --password-file F --local DIR --control SOCK [options]
    smbspike.py --port P --user U --password-file F --daemon SOCK --drive NAME [options]

`--local` serves a folder; another machine's changes are made in the folder and announced on the
control socket as `<action> <path>` lines (`localpeer.py` does both, with `mount_peer`'s commands).
`--daemon` serves a voidfs daemon's `/v1/fs` session on the drive; changes come from its watch, as
they do for the FSKit extension. Mount with
`mount_smbfs -o nobrowse //U:PASSWORD@127.0.0.1:P/<share> <dir>`.

It binds 127.0.0.1 only and authenticates one user with NTLMv2. Every request is logged with a
timestamp, which shows what the client asked the server and what it answered from its caches.
"""

import argparse
import asyncio
import ctypes
import fnmatch
import hashlib
import hmac
import json
import os
import struct
import sys
import time
import unicodedata
import urllib.parse
import uuid

# ---------------------------------------------------------------------------------------------
# Constants

NEGOTIATE, SESSION_SETUP, LOGOFF, TREE_CONNECT, TREE_DISCONNECT, CREATE, CLOSE, FLUSH, READ, WRITE, \
    LOCK, IOCTL, CANCEL, ECHO, QUERY_DIRECTORY, CHANGE_NOTIFY, QUERY_INFO, SET_INFO, OPLOCK_BREAK = range(19)
NAMES = ["NEGOTIATE", "SESSION_SETUP", "LOGOFF", "TREE_CONNECT", "TREE_DISCONNECT", "CREATE", "CLOSE", "FLUSH",
         "READ", "WRITE", "LOCK", "IOCTL", "CANCEL", "ECHO", "QUERY_DIRECTORY", "CHANGE_NOTIFY", "QUERY_INFO",
         "SET_INFO", "OPLOCK_BREAK"]

FLAG_RESPONSE, FLAG_ASYNC, FLAG_RELATED, FLAG_SIGNED = 0x1, 0x2, 0x4, 0x8

S_OK = 0
S_PENDING = 0x00000103
S_NOTIFY_CLEANUP = 0x0000010B
S_NOTIFY_ENUM_DIR = 0x0000010C
S_BUFFER_OVERFLOW = 0x80000005
S_NO_MORE_FILES = 0x80000006
S_INVALID_INFO_CLASS = 0xC0000003
S_INFO_LENGTH_MISMATCH = 0xC0000004
S_INVALID_HANDLE = 0xC0000008
S_INVALID_PARAMETER = 0xC000000D
S_NO_SUCH_FILE = 0xC000000F
S_INVALID_DEVICE_REQUEST = 0xC0000010
S_END_OF_FILE = 0xC0000011
S_MORE_PROCESSING = 0xC0000016
S_ACCESS_DENIED = 0xC0000022
S_OBJECT_NAME_INVALID = 0xC0000033
S_OBJECT_NAME_NOT_FOUND = 0xC0000034
S_OBJECT_NAME_COLLISION = 0xC0000035
S_OBJECT_PATH_NOT_FOUND = 0xC000003A
S_LOGON_FAILURE = 0xC000006D
S_MEDIA_WRITE_PROTECTED = 0xC00000A2
S_FILE_IS_A_DIRECTORY = 0xC00000BA
S_NOT_SUPPORTED = 0xC00000BB
S_BAD_NETWORK_NAME = 0xC00000CC
S_NOT_A_DIRECTORY = 0xC0000103
S_CANCELLED = 0xC0000120
S_FILE_CLOSED = 0xC0000128
S_IO_DEVICE_ERROR = 0xC0000185
S_FS_DRIVER_REQUIRED = 0xC000019C
S_USER_SESSION_DELETED = 0xC0000203
S_NETWORK_UNREACHABLE = 0xC000023C
S_NOT_FOUND = 0xC0000225
S_RETRY = 0xC000022D  # STATUS_RETRY

LEASE_R, LEASE_H, LEASE_W = 0x1, 0x2, 0x4
NOTIFY_ADDED, NOTIFY_REMOVED, NOTIFY_MODIFIED, NOTIFY_RENAMED_OLD, NOTIFY_RENAMED_NEW = 1, 2, 3, 4, 5

ATTR_READONLY, ATTR_DIRECTORY, ATTR_ARCHIVE = 0x1, 0x10, 0x20
READ_ACCESS = 0x001200A9  # FILE_READ_DATA | READ_EA | EXECUTE | READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE
WRITE_BITS = 0x00000002 | 0x00000004 | 0x00000010 | 0x00000100 | 0x00010000 | 0x00040000 | 0x00080000 | 0x40000000 | 0x10000000

EPOCH_DIFF = 11644473600

MECH_NTLM = bytes.fromhex("060a2b06010401823702020a")
OID_SPNEGO = bytes.fromhex("06062b0601050502")

LOG = None
T0 = time.time()


def log(*parts):
    line = f"{time.time():.3f} " + " ".join(str(p) for p in parts)
    print(line, file=LOG, flush=True)


def filetime(ns):
    return (ns // 100) + EPOCH_DIFF * 10_000_000


# ---------------------------------------------------------------------------------------------
# Crypto: NTLMv2, RC4, SMB 2.1 HMAC-SHA256 and SMB 3.x AES-CMAC signing.

_cc = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
_cc.CCAESCmac.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_char_p]
_cc.CCAESCmac.restype = None


def aes_cmac(key, data):
    out = ctypes.create_string_buffer(16)
    _cc.CCAESCmac(key, data, len(data), out)
    return out.raw


def hmac_md5(key, data):
    return hmac.new(key, data, "md5").digest()


class RC4:
    def __init__(self, key):
        s = list(range(256))
        j = 0
        for i in range(256):
            j = (j + s[i] + key[i % len(key)]) & 0xFF
            s[i], s[j] = s[j], s[i]
        self.s, self.i, self.j = s, 0, 0

    def crypt(self, data):
        s, i, j, out = self.s, self.i, self.j, bytearray()
        for b in data:
            i = (i + 1) & 0xFF
            j = (j + s[i]) & 0xFF
            s[i], s[j] = s[j], s[i]
            out.append(b ^ s[(s[i] + s[j]) & 0xFF])
        self.i, self.j = i, j
        return bytes(out)


def kdf(key, label, context):
    return hmac.new(key, struct.pack(">I", 1) + label + b"\x00" + context + struct.pack(">I", 128), "sha256").digest()[:16]


# ---------------------------------------------------------------------------------------------
# DER, for SPNEGO.

def der(tag, content):
    n = len(content)
    if n < 0x80:
        length = bytes([n])
    else:
        b = n.to_bytes((n.bit_length() + 7) // 8, "big")
        length = bytes([0x80 | len(b)]) + b
    return bytes([tag]) + length + content


def der_read(buf, pos):
    tag = buf[pos]
    n = buf[pos + 1]
    pos += 2
    if n & 0x80:
        k = n & 0x7F
        n = int.from_bytes(buf[pos:pos + k], "big")
        pos += k
    return tag, buf[pos:pos + n], pos + n


def der_children(content):
    out, pos = [], 0
    while pos < len(content):
        start = pos
        tag, value, pos = der_read(content, pos)
        out.append((tag, value, content[start:pos]))
    return out


def spnego_init():
    return der(0x60, OID_SPNEGO + der(0xA0, der(0x30, der(0xA0, der(0x30, MECH_NTLM)))))


def spnego_resp(state, token=None, mic=None, mech=True):
    parts = der(0xA0, der(0x0A, bytes([state])))
    if mech:
        parts += der(0xA1, MECH_NTLM)
    if token:
        parts += der(0xA2, der(0x04, token))
    if mic:
        parts += der(0xA3, der(0x04, mic))
    return der(0xA1, der(0x30, parts))


def spnego_parse(blob):
    """(ntlm token, client mechTypes encoding or None, client MIC or None)."""
    if blob.startswith(b"NTLMSSP\x00"):
        return blob, None, None
    tag, value, _ = der_read(blob, 0)
    mech_types = token = mic = None
    if tag == 0x60:
        kids = der_children(value)
        seq = der_children([k for k in kids if k[0] == 0xA0][0][1])[0][1]
    else:
        seq = der_children(value)[0][1]
    for t, v, _raw in der_children(seq):
        if t == 0xA0 and tag == 0x60:
            mech_types = der_children(v)[0][2]
        elif t == 0xA2:
            token = der_children(v)[0][1]
        elif t == 0xA3:
            mic = der_children(v)[0][1]
    return token, mech_types, mic


# ---------------------------------------------------------------------------------------------
# NTLM.

NTLM_UNICODE, NTLM_REQUEST_TARGET, NTLM_SIGN, NTLM_SEAL = 0x1, 0x4, 0x10, 0x20
NTLM_NTLM, NTLM_ALWAYS_SIGN, NTLM_TARGET_SERVER, NTLM_ESS = 0x200, 0x8000, 0x20000, 0x80000
NTLM_TARGET_INFO, NTLM_VERSION, NTLM_128, NTLM_KEY_EXCH, NTLM_56 = 0x800000, 0x2000000, 0x20000000, 0x40000000, 0x80000000


def ntlm_challenge(client_flags, challenge):
    flags = NTLM_UNICODE | NTLM_REQUEST_TARGET | NTLM_NTLM | NTLM_ALWAYS_SIGN | NTLM_TARGET_SERVER | NTLM_ESS | NTLM_TARGET_INFO | NTLM_VERSION
    flags |= client_flags & (NTLM_SIGN | NTLM_SEAL | NTLM_128 | NTLM_KEY_EXCH | NTLM_56)
    name = "SPIKE".encode("utf-16-le")
    av = b""
    for kind, value in ((2, name), (1, name), (4, "localhost".encode("utf-16-le")), (3, "localhost".encode("utf-16-le")),
                        (7, struct.pack("<Q", filetime(time.time_ns())))):
        av += struct.pack("<HH", kind, len(value)) + value
    av += struct.pack("<HH", 0, 0)
    head_len = 56
    msg = b"NTLMSSP\x00" + struct.pack("<I", 2)
    msg += struct.pack("<HHI", len(name), len(name), head_len)
    msg += struct.pack("<I", flags) + challenge + b"\x00" * 8
    msg += struct.pack("<HHI", len(av), len(av), head_len + len(name))
    msg += bytes([10, 0]) + struct.pack("<H", 20348) + b"\x00\x00\x00\x0f"
    return msg + name + av, flags


def ntlm_field(msg, off):
    n, _, o = struct.unpack_from("<HHI", msg, off)
    return msg[o:o + n]


def ntlm_authenticate(msg, challenge, users):
    """(user, exported session key, flags) or None when the response doesn't verify."""
    nt = ntlm_field(msg, 20)
    domain = ntlm_field(msg, 28).decode("utf-16-le")
    user = ntlm_field(msg, 36).decode("utf-16-le")
    enc_key = ntlm_field(msg, 52)
    flags = struct.unpack_from("<I", msg, 60)[0]
    password = users.get(user.lower())
    if password is None or len(nt) <= 24:
        return None
    nthash = hashlib.new("md4", password.encode("utf-16-le")).digest()
    ntowf = hmac_md5(nthash, (user.upper() + domain).encode("utf-16-le"))
    proof = hmac_md5(ntowf, challenge + nt[16:])
    if not hmac.compare_digest(proof, nt[:16]):
        return None
    base = hmac_md5(ntowf, proof)
    exported = RC4(base).crypt(enc_key) if (flags & NTLM_KEY_EXCH) and enc_key else base
    return user, exported, flags


def ntlm_server_mic(exported, flags, message):
    sign = hashlib.md5(exported + b"session key to server-to-client signing key magic constant\x00").digest()
    seal = hashlib.md5(exported + b"session key to server-to-client sealing key magic constant\x00").digest()
    checksum = hmac_md5(sign, struct.pack("<I", 0) + message)[:8]
    if flags & NTLM_KEY_EXCH:
        checksum = RC4(seal).crypt(checksum)
    return struct.pack("<I", 1) + checksum + struct.pack("<I", 0)


# ---------------------------------------------------------------------------------------------
# Backends: what is served.

class Missing(Exception):
    """The name isn't there (or, with `path`, a folder on the way isn't)."""

    def __init__(self, path=False):
        self.path = path


class Failed(Exception):
    def __init__(self, status, why=""):
        self.status, self.why = status, why


class Attr:
    __slots__ = ("is_dir", "size", "mtime_ns", "ctime_ns", "btime_ns", "ino")

    def __init__(self, is_dir, size, mtime_ns, ctime_ns, btime_ns, ino):
        self.is_dir, self.size, self.mtime_ns, self.ctime_ns, self.btime_ns, self.ino = is_dir, size, mtime_ns, ctime_ns, btime_ns, ino


class LocalBackend:
    """A local folder. Opens keep the file they opened: another machine's change replaces a file
    (write elsewhere, rename over), as a put makes a new version."""

    def __init__(self, root, frozen_dirs=False):
        self.root = os.path.realpath(root)
        self.frozen_dirs = frozen_dirs

    def _path(self, rel):
        parts = [p for p in rel.split("/") if p not in ("", ".")]
        if any(p == ".." for p in parts):
            raise Failed(S_OBJECT_NAME_INVALID)
        return os.path.join(self.root, *parts)

    def _attr(self, st):
        import stat as st_mod
        if self.frozen_dirs and st_mod.S_ISDIR(st.st_mode):
            return Attr(True, st.st_size, 0, 0, 0, st.st_ino)  # as the core reports a folder without a time: 1970
        return Attr(st_mod.S_ISDIR(st.st_mode), st.st_size, st.st_mtime_ns, st.st_ctime_ns,
                    int(getattr(st, "st_birthtime", st.st_ctime) * 1e9), st.st_ino)

    async def stat(self, rel):
        p = self._path(rel)
        try:
            return self._attr(os.stat(p))
        except FileNotFoundError:
            raise Missing(not os.path.isdir(os.path.dirname(p)))
        except NotADirectoryError:
            raise Missing(True)

    async def list(self, rel):
        p = self._path(rel)
        out = []
        for name in sorted(os.listdir(p)):
            try:
                out.append((name, self._attr(os.stat(os.path.join(p, name)))))
            except FileNotFoundError:
                pass
        return out

    async def open(self, rel):
        fd = os.open(self._path(rel), os.O_RDONLY)
        return fd, self._attr(os.fstat(fd))

    async def read(self, handle, offset, length):
        return os.pread(handle, length, offset)

    async def close(self, handle):
        os.close(handle)


class PathIds:
    """`--path-ids`: a file's ID is made from its inode and its path, so a file another machine
    renames turns up with a new ID. smbfs finds a file's node by its ID; with the same ID under a
    new name it opens the node's old name, which is gone."""

    def __init__(self, backend):
        self.backend = backend

    def __getattr__(self, name):
        return getattr(self.backend, name)

    @staticmethod
    def _id(rel, a):
        if rel == "":
            return a
        h = int.from_bytes(hashlib.blake2b(f"{a.ino}/{rel}".encode(), digest_size=8).digest(), "little") & ((1 << 63) - 1)
        return Attr(a.is_dir, a.size, a.mtime_ns, a.ctime_ns, a.btime_ns, h or 1)

    async def stat(self, rel):
        return self._id(rel, await self.backend.stat(rel))

    async def list(self, rel):
        return [(name, self._id((rel + "/" if rel else "") + name, a)) for name, a in await self.backend.list(rel)]

    async def open(self, rel):
        handle, a = await self.backend.open(rel)
        return handle, self._id(rel, a)


class AdvanceDirTimes:
    """`--advance-dir-times`: a folder's time is at least the last time the server learned of a
    change in it. smbfs drops a folder's cached misses when it sees the folder's time move, and
    the core gives a folder the time of its first object (the top folder 1970)."""

    def __init__(self, backend, times):
        self.backend, self.times = backend, times

    def __getattr__(self, name):
        return getattr(self.backend, name)

    def _bump(self, rel, a):
        t = self.times.get(rel, 0) if a.is_dir else 0
        if t <= a.mtime_ns:
            return a
        return Attr(True, a.size, t, max(a.ctime_ns, t), a.btime_ns, a.ino)

    async def stat(self, rel):
        return self._bump(rel, await self.backend.stat(rel))

    async def list(self, rel):
        return [(name, self._bump((rel + "/" if rel else "") + name, a)) for name, a in await self.backend.list(rel)]


class DaemonBackend:
    """A voidfs daemon's `/v1/fs` session on one drive (HTTP/1.1 over its Unix socket)."""

    def __init__(self, socket_path, drive):
        self.socket_path, self.drive = socket_path, drive
        self.pool = []
        # The daemon answers Busy past MAX_CALLS (32) calls at once; smbfs keeps up to ~40 READs
        # in flight. Reads stay below it, wait out a Busy, and give up on a reply after 10 s.
        self.reads = asyncio.Semaphore(int(os.environ.get("SPIKE_DAEMON_READS", "24")))
        self.info = None
        self.inodes = {"": None}  # path -> ino, filled by lookups

    async def _request(self, method, path, body=None, ctype="application/json", raw=False):
        if self.pool:
            reader, writer = self.pool.pop()
        else:
            reader, writer = await asyncio.open_unix_connection(self.socket_path)
        data = b"" if body is None else (json.dumps(body).encode() if ctype == "application/json" else body)
        head = f"{method} {path} HTTP/1.1\r\nhost: localhost\r\ncontent-length: {len(data)}\r\ncontent-type: {ctype}\r\n"
        if self.info:
            head += f"x-voidfs-generation: {self.info['generation']}\r\n"
        writer.write(head.encode() + b"\r\n" + data)
        await writer.drain()
        status_line = await reader.readline()
        status = int(status_line.split()[1])
        length, chunked, close = None, False, False
        while True:
            line = await reader.readline()
            if line in (b"\r\n", b""):
                break
            k, _, v = line.decode().partition(":")
            k, v = k.strip().lower(), v.strip()
            if k == "content-length":
                length = int(v)
            elif k == "transfer-encoding" and "chunked" in v:
                chunked = True
            elif k == "connection" and v.lower() == "close":
                close = True
        if chunked:
            payload = b""
            while True:
                n = int((await reader.readline()).strip(), 16)
                if n == 0:
                    await reader.readline()
                    break
                payload += await reader.readexactly(n)
                await reader.readline()
        else:
            payload = await reader.readexactly(length or 0)
        if close:
            writer.close()
        else:
            self.pool.append((reader, writer))
        if status >= 300:
            try:
                err = json.loads(payload)["error"]
            except Exception:
                err = {"code": str(status), "errno": 5, "message": payload[:200]}
            raise DaemonError(err)
        return payload if raw else (json.loads(payload) if payload else {})

    async def start(self):
        self.info = await self._request("POST", "/v1/fs/sessions", {"version": 1, "drive": self.drive, "readOnly": True})
        self.inodes[""] = self.info["root"]
        log("daemon session", self.info["id"][:8], "root", self.info["root"], "generation", self.info["generation"])

    def _route(self, op):
        return f"/v1/fs/{self.info['id']}/{op}"

    def _attr(self, a):
        def ns(t):
            # RFC 3339 → ns since the epoch (the daemon writes UTC with a Z and up to 9 digits).
            import datetime
            base, _, frac = t.rstrip("Z").partition(".")
            dt = datetime.datetime.strptime(base, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=datetime.timezone.utc)
            return int(dt.timestamp()) * 1_000_000_000 + int((frac + "000000000")[:9])
        is_dir = a.get("kind") == "folder"
        m = ns(a["mtime"]) if a.get("mtime") else 0
        return Attr(is_dir, a.get("size", 0), m, m, m, a["ino"])

    async def _ino(self, rel):
        if rel in self.inodes:
            return self.inodes[rel]
        parent, _, name = rel.rpartition("/")
        pino = await self._ino(parent)
        a = await self._lookup(pino, name, parent_missing_is_path=True)
        return a["ino"]

    async def _lookup(self, pino, name, parent_missing_is_path=False):
        try:
            return await self._request("POST", self._route("lookup"), {"parent": pino, "name": name})
        except DaemonError as e:
            if e.errno == 2:
                raise Missing(parent_missing_is_path)
            if e.errno == 20:
                raise Missing(True)
            raise

    async def stat(self, rel):
        if rel == "":
            return self._attr(await self._request("POST", self._route("getattr"), {"ino": self.info["root"]}))
        parent, _, name = rel.rpartition("/")
        try:
            pino = await self._ino(parent)
        except Missing:
            raise Missing(True)
        a = await self._lookup(pino, name)
        self.inodes[rel] = a["ino"]
        return self._attr(a)

    async def list(self, rel):
        ino = await self._ino(rel)
        out, after = [], None
        while True:
            try:
                page = await self._request("POST", self._route("readdir"), {"ino": ino, "after": after, "limit": 256})
            except DaemonError as e:
                if e.errno == 35:  # EAGAIN: a change raced the page; start again
                    out, after = [], None
                    continue
                raise
            for name, a in page["entries"]:
                out.append((name, self._attr(a)))
                self.inodes[(rel + "/" if rel else "") + name] = a["ino"]
            if len(page["entries"]) < 256:
                return out
            after = page["entries"][-1][0]

    async def open(self, rel):
        ino = await self._ino(rel)
        r = await self._request("POST", self._route("open"), {"ino": ino, "write": False})
        return r["fh"], self._attr(r["attr"])

    async def read(self, handle, offset, length):
        q = urllib.parse.urlencode({"fh": handle, "offset": offset, "length": length})
        async with self.reads:
            for attempt in range(100):
                try:
                    return await asyncio.wait_for(self._request("GET", self._route("read") + "?" + q, raw=True), 10)
                except asyncio.TimeoutError:
                    log(f"daemon read fh={handle} off={offset} len={length}: no reply in 10 s (attempt {attempt + 1})")
                except DaemonError as e:
                    if e.errno != 35:
                        raise
                    log(f"daemon read fh={handle} off={offset}: busy (attempt {attempt + 1})")
                    await asyncio.sleep(0.01)
            raise DaemonError({"code": "Busy", "errno": 35, "message": "still busy"})

    async def close(self, handle):
        await self._request("POST", self._route("close"), {"fh": handle})

    def forget(self, prefix):
        for k in [k for k in self.inodes if k and (k == prefix or k.startswith(prefix + "/") or prefix == "")]:
            del self.inodes[k]

    async def watch(self, server):
        """Follow the session's invalidations and hand them to the server as changes."""
        reader, writer = await asyncio.open_unix_connection(self.socket_path)
        head = f"GET {self._route('watch')} HTTP/1.1\r\nhost: localhost\r\nx-voidfs-generation: {self.info['generation']}\r\n\r\n"
        writer.write(head.encode())
        await writer.drain()
        await reader.readline()
        chunked = False
        while True:
            line = await reader.readline()
            if line in (b"\r\n", b""):
                break
            if line.lower().startswith(b"transfer-encoding") and b"chunked" in line.lower():
                chunked = True
        buf = b""
        while True:
            if chunked:
                n = int((await reader.readline()).strip(), 16)
                if n == 0:
                    break
                buf += await reader.readexactly(n)
                await reader.readline()
            else:
                piece = await reader.read(65536)
                if not piece:
                    break
                buf += piece
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                if not line.strip():
                    continue
                event = json.loads(line)
                log("watch", json.dumps(event)[:300])
                if event.get("resync"):
                    self.forget("")
                    server.changed_all()
                    continue
                for inv in event.get("invalidations", []):
                    if inv["kind"] == "all":
                        self.forget("")
                        server.changed_all()
                    else:
                        key = inv["key"].strip("/")
                        self.forget(key)
                        server.changed(key, NOTIFY_MODIFIED, subtree=inv["kind"] == "subtree")
        log("watch ended")


class DaemonError(Exception):
    ERRNO = {2: S_OBJECT_NAME_NOT_FOUND, 13: S_ACCESS_DENIED, 20: S_NOT_A_DIRECTORY, 21: S_FILE_IS_A_DIRECTORY,
             30: S_MEDIA_WRITE_PROTECTED, 35: S_RETRY, 50: S_NETWORK_UNREACHABLE, 70: S_FILE_CLOSED, 45: S_NOT_SUPPORTED,
             9: S_INVALID_HANDLE, 92: S_OBJECT_NAME_INVALID}

    def __init__(self, err):
        super().__init__(f"{err.get('code')}: {err.get('message')}")
        self.errno = err.get("errno") or 5
        self.status = self.ERRNO.get(self.errno, S_IO_DEVICE_ERROR)


# ---------------------------------------------------------------------------------------------
# Wire helpers.

def error_body():
    return struct.pack("<HBBI", 9, 0, 0, 0) + b"\x00"


def utf16(s):
    return s.encode("utf-16-le")


def pad8(b):
    return b + b"\x00" * (-len(b) % 8)


def basic_info(a):
    attrs = ATTR_DIRECTORY if a.is_dir else ATTR_ARCHIVE
    return struct.pack("<QQQQII", filetime(a.btime_ns), filetime(a.mtime_ns), filetime(a.mtime_ns), filetime(a.ctime_ns), attrs, 0)


def alloc(a):
    return 0 if a.is_dir else (a.size + 4095) // 4096 * 4096


def standard_info(a):
    return struct.pack("<QQIBBH", alloc(a), 0 if a.is_dir else a.size, 1, 0, 1 if a.is_dir else 0, 0)


def network_open_info(a):
    return basic_info(a)[:32] + struct.pack("<QQII", alloc(a), 0 if a.is_dir else a.size, ATTR_DIRECTORY if a.is_dir else ATTR_ARCHIVE, 0)


def dir_entry(cls, name, a, index):
    n = utf16(name)
    times = struct.pack("<QQQQ", filetime(a.btime_ns), filetime(a.mtime_ns), filetime(a.mtime_ns), filetime(a.ctime_ns))
    sizes = struct.pack("<QQI", 0 if a.is_dir else a.size, alloc(a), ATTR_DIRECTORY if a.is_dir else ATTR_ARCHIVE)
    if cls == 1:  # FileDirectoryInformation
        body = times + sizes + struct.pack("<I", len(n))
    elif cls == 2:  # FileFullDirectoryInformation
        body = times + sizes + struct.pack("<II", len(n), 0)
    elif cls == 3:  # FileBothDirectoryInformation
        body = times + sizes + struct.pack("<IIBB", len(n), 0, 0, 0) + b"\x00" * 24
    elif cls == 37:  # FileIdBothDirectoryInformation
        body = times + sizes + struct.pack("<IIBB", len(n), 0, 0, 0) + b"\x00" * 24 + struct.pack("<HQ", 0, a.ino)
    elif cls == 38:  # FileIdFullDirectoryInformation
        body = times + sizes + struct.pack("<IIIQ", len(n), 0, 0, a.ino)
    elif cls == 12:  # FileNamesInformation
        return struct.pack("<III", 0, index, len(n)) + n
    else:
        raise Failed(S_INVALID_INFO_CLASS)
    return struct.pack("<II", 0, index) + body + n


def chain_entries(entries):
    """Join directory or notify entries, 8-byte aligned, setting each NextEntryOffset."""
    out = b""
    for i, e in enumerate(entries):
        if i < len(entries) - 1:
            e = pad8(e)
            e = struct.pack("<I", len(e)) + e[4:]
        out += e
    return out


def context_blob(contexts):
    out = b""
    for i, (name, data) in enumerate(contexts):
        c = struct.pack("<IHHHHI", 0, 16, 4, 0, 24 if data else 0, len(data)) + name + b"\x00" * 4 + data
        if i < len(contexts) - 1:
            c = pad8(c)
            c = struct.pack("<I", len(c)) + c[4:]
        out += c
    return out


def parse_contexts(buf):
    out, pos = {}, 0
    while True:
        nxt, noff, nlen, _, doff, dlen = struct.unpack_from("<IHHHHI", buf, pos)
        out[bytes(buf[pos + noff:pos + noff + nlen])] = bytes(buf[pos + doff:pos + doff + dlen])
        if not nxt:
            return out
        pos += nxt


# ---------------------------------------------------------------------------------------------
# State.

class Lease:
    def __init__(self, key, state, version, is_dir, path, parent_key=b""):
        self.key, self.state, self.version, self.is_dir, self.path = key, state, version, is_dir, path
        self.epoch, self.parent_key, self.opens, self.breaking = 1, parent_key, 0, False


class Open:
    def __init__(self, fid, tree, path, attr, handle, is_dir):
        self.fid, self.tree, self.path, self.attr, self.handle, self.is_dir = fid, tree, path, attr, handle, is_dir
        self.listing, self.cursor, self.pattern = None, 0, None
        self.lease = None
        self.notify = None  # (message id, async id, session, filter, tree flag, max length, signed)
        self.pending_changes = []  # changes while no notify request is outstanding


class Session:
    def __init__(self, sid):
        self.sid, self.user, self.key, self.signing_key, self.done = sid, None, None, None, False
        self.challenge, self.negotiate_msg, self.challenge_msg, self.flags = None, None, None, 0
        self.preauth = None


class Server:
    def __init__(self, args, backend):
        self.args, self.backend = args, backend
        self.users = {args.user.lower(): open(args.password_file).read().strip()}
        self.guid = uuid.uuid4().bytes
        self.start = filetime(time.time_ns())
        self.conns = []
        self.next_conn = 1
        self.dir_times = {}  # folder -> when the server last learned of a change in it

    def changed(self, path, action, old=None, subtree=False):
        log("changed", action, path, f"old={old}" if old else "", "subtree" if subtree else "")
        now = time.time_ns()
        for p in (path, old) if old else (path,):
            self.dir_times[p.rpartition("/")[0]] = now
        for c in self.conns:
            c.changed(path, action, old, subtree)

    def changed_all(self):
        log("changed all")
        for c in self.conns:
            c.changed_all()


class Conn:
    def __init__(self, server, reader, writer):
        self.server, self.reader, self.writer = server, reader, writer
        self.id = server.next_conn
        server.next_conn += 1
        self.dialect = None
        self.preauth = b"\x00" * 64
        self.sessions, self.trees, self.opens, self.leases = {}, {}, {}, {}
        self.next_sid, self.next_tid, self.next_fid, self.next_async = 0x1000, 1, 1, 1
        self.wlock = asyncio.Lock()
        self.pending = {}  # async id -> Open with a notify outstanding
        self.handle_refs = {}  # backend handle -> opens sharing it (--hold-open-version)
        self.args = server.args

    # -- transport

    async def send(self, data):
        async with self.wlock:
            self.writer.write(len(data).to_bytes(4, "big") + data)
            await self.writer.drain()

    async def run(self):
        peer = self.writer.get_extra_info("peername")
        log(f"conn {self.id} from {peer}")
        try:
            while True:
                head = await self.reader.readexactly(4)
                n = int.from_bytes(head[1:], "big")
                data = await self.reader.readexactly(n)
                if data[:4] == b"\xffSMB":
                    await self.smb1_negotiate(data)
                elif data[:4] == b"\xfeSMB":
                    command = struct.unpack_from("<H", data, 12)[0]
                    if command in (NEGOTIATE, SESSION_SETUP):
                        await self.handle(data)
                    else:
                        asyncio.ensure_future(self.handle(data))
                else:
                    log(f"conn {self.id} unsupported frame {data[:4]!r}")
                    break
        except (asyncio.IncompleteReadError, ConnectionResetError, BrokenPipeError):
            pass
        finally:
            log(f"conn {self.id} closed")
            for o in list(self.opens.values()):
                await self.release(o)
            if self in self.server.conns:
                self.server.conns.remove(self)
            self.writer.close()

    # -- SMB1 to SMB2 upgrade

    async def smb1_negotiate(self, data):
        dialects = [d.decode(errors="replace") for d in data[35:].split(b"\x02") if d]
        dialects = [d.rstrip("\x00") for d in dialects]
        log(f"conn {self.id} SMB1 NEGOTIATE {dialects}")
        if "SMB 2.???" in dialects:
            await self.send(self.header(NEGOTIATE, 0, 0, 0, S_OK, 1, 0) + self.negotiate_body(0x02FF, []))
        elif "SMB 2.002" in dialects:
            self.dialect = 0x0202
            await self.send(self.header(NEGOTIATE, 0, 0, 0, S_OK, 1, 0) + self.negotiate_body(0x0202, []))

    # -- SMB2

    def header(self, command, mid, tid, sid, status, credits, flags, async_id=None):
        flags |= FLAG_RESPONSE
        if async_id is not None:
            flags |= FLAG_ASYNC
            middle = struct.pack("<Q", async_id)
        else:
            middle = struct.pack("<II", 0, tid)
        return b"\xfeSMB" + struct.pack("<HHIHHII", 64, 0, status, command, credits, flags, 0) + struct.pack("<Q", mid) + middle + struct.pack("<Q", sid) + b"\x00" * 16

    def sign(self, msg, session):
        if session is None or session.signing_key is None:
            return msg
        msg = bytearray(msg)
        struct.pack_into("<I", msg, 16, struct.unpack_from("<I", msg, 16)[0] | FLAG_SIGNED)
        msg[48:64] = b"\x00" * 16
        if self.dialect >= 0x0300:
            sig = aes_cmac(session.signing_key, bytes(msg))
        else:
            sig = hmac.new(session.signing_key, bytes(msg), "sha256").digest()[:16]
        msg[48:64] = sig
        return bytes(msg)

    async def handle(self, data):
        pos, out = 0, []
        last_fid = last_tid = last_sid = None
        last_err = None
        while True:
            (_, _, charge, _, command, credits, flags, nxt, mid) = struct.unpack_from("<4sHHIHHIIQ", data, pos)
            end = pos + nxt if nxt else len(data)
            msg = data[pos:end]
            if flags & FLAG_ASYNC:
                tid = 0
                async_id = struct.unpack_from("<Q", msg, 32)[0]
            else:
                tid = struct.unpack_from("<I", msg, 36)[0]
                async_id = None
            sid = struct.unpack_from("<Q", msg, 40)[0]
            related = bool(flags & FLAG_RELATED)
            if related:
                tid = tid if last_tid is None else last_tid
                sid = sid if last_sid is None else last_sid
            ctx = {"mid": mid, "tid": tid, "sid": sid, "related": related, "last_fid": last_fid, "async_id": async_id,
                   "signed": bool(flags & FLAG_SIGNED), "raw": msg, "credits": max(credits, 1)}
            if command == CANCEL:
                self.cancel(ctx)
                status, body, extra = None, None, None
            elif related and last_err is not None:
                status, body, extra = last_err, error_body(), {}
            else:
                try:
                    status, body, extra = await self.dispatch(command, msg[64:], ctx)
                except Failed as e:
                    status, body, extra = e.status, error_body(), {}
                except Missing as e:
                    status, body, extra = (S_OBJECT_PATH_NOT_FOUND if e.path else S_OBJECT_NAME_NOT_FOUND), error_body(), {}
                except DaemonError as e:
                    status, body, extra = e.status, error_body(), {}
                except FileNotFoundError:
                    status, body, extra = S_OBJECT_NAME_NOT_FOUND, error_body(), {}
                except NotADirectoryError:
                    status, body, extra = S_OBJECT_PATH_NOT_FOUND, error_body(), {}
                except Exception as e:  # report, don't drop the connection
                    log(f"conn {self.id} {NAMES[command]} error {e!r}")
                    import traceback
                    traceback.print_exc(file=LOG)
                    status, body, extra = S_INVALID_PARAMETER, error_body(), {}
            if status is not None:
                if command == CREATE:
                    if status == S_OK:
                        last_fid = extra.get("fid")
                    else:
                        last_err = status
                last_tid, last_sid = extra.get("tid", ctx["tid"]), extra.get("sid", ctx["sid"])
                session = self.sessions.get(extra.get("sid", sid))
                if status == S_PENDING:
                    hdr = self.header(command, mid, 0, ctx["sid"], S_PENDING, ctx["credits"], FLAG_RELATED if related else 0, async_id=extra["async_id"])
                else:
                    hdr = self.header(command, mid, extra.get("tid", ctx["tid"]), extra.get("sid", ctx["sid"]), status, ctx["credits"], FLAG_RELATED if related else 0)
                out.append((hdr + body, session, ctx["signed"] or extra.get("sign", False), command))
            if not nxt:
                break
            pos += nxt
        if not out:
            return
        pieces = []
        for i, (msg, session, signed, command) in enumerate(out):
            if i < len(out) - 1:
                msg = pad8(msg)
                msg = msg[:20] + struct.pack("<I", len(msg)) + msg[24:]
            if signed and session is not None and session.signing_key is not None and command != NEGOTIATE:
                msg = self.sign(msg, session)
            pieces.append(msg)
        response = b"".join(pieces)
        if out[0][3] == NEGOTIATE and self.dialect == 0x0311:
            self.preauth = hashlib.sha512(self.preauth + response).digest()
        await self.send(response)

    async def dispatch(self, command, body, ctx):
        if command == NEGOTIATE:
            return self.negotiate(body, ctx)
        if command == SESSION_SETUP:
            return self.session_setup(body, ctx)
        session = self.sessions.get(ctx["sid"])
        if command == ECHO:
            return S_OK, struct.pack("<HH", 4, 0), {}
        if session is None or not session.done:
            raise Failed(S_USER_SESSION_DELETED)
        if command == LOGOFF:
            self.sessions.pop(ctx["sid"], None)
            return S_OK, struct.pack("<HH", 4, 0), {}
        if command == TREE_CONNECT:
            return self.tree_connect(body, ctx)
        if command == TREE_DISCONNECT:
            self.trees.pop(ctx["tid"], None)
            return S_OK, struct.pack("<HH", 4, 0), {}
        tree = self.trees.get(ctx["tid"])
        if tree is None:
            raise Failed(S_BAD_NETWORK_NAME)
        handler = {CREATE: self.create, CLOSE: self.close, READ: self.read, QUERY_INFO: self.query_info,
                   QUERY_DIRECTORY: self.query_directory, CHANGE_NOTIFY: self.change_notify, IOCTL: self.ioctl,
                   FLUSH: self.flush, LOCK: self.lock, OPLOCK_BREAK: self.oplock_break}.get(command)
        if handler is None:  # WRITE, SET_INFO: a read-only drive
            log(f"conn {self.id} {NAMES[command]} refused (read-only)")
            raise Failed(S_ACCESS_DENIED)
        return await handler(body, ctx, tree)

    # -- negotiate and authenticate

    def negotiate_body(self, dialect, contexts):
        caps = 0x4  # LARGE_MTU
        if self.args.leases != "none":
            caps |= 0x2  # LEASING
            if dialect >= 0x0300 and self.args.leases == "all":
                caps |= 0x20  # DIRECTORY_LEASING
        token = spnego_init()
        mode = 0x1 | (0x2 if self.args.signing_required else 0)
        fixed = struct.pack("<HHHH", 65, mode, dialect, len(contexts)) + self.server.guid
        fixed += struct.pack("<IIIIQQHHI", caps, 8 << 20, 8 << 20, 8 << 20, filetime(time.time_ns()), self.server.start,
                             128, len(token), 0)
        body = fixed + token
        if contexts:
            off = 64 + len(body)
            off += -off % 8
            body = body + b"\x00" * (off - 64 - len(body))
            blob = b""
            for i, (kind, data) in enumerate(contexts):
                c = struct.pack("<HHI", kind, len(data), 0) + data
                if i < len(contexts) - 1:
                    c = pad8(c)
                blob += c
            body = body[:60] + struct.pack("<I", off) + body[64:] + blob
        return body

    def negotiate(self, body, ctx):
        count, mode, _, caps = struct.unpack_from("<HHHI", body, 2)
        dialects = list(struct.unpack_from(f"<{count}H", body, 36))
        allowed = [d for d in (0x0311, 0x0302, 0x0300, 0x0210, 0x0202) if d <= self.args.max_dialect and d in dialects]
        if not allowed:
            raise Failed(S_NOT_SUPPORTED)
        self.dialect = allowed[0]
        log(f"conn {self.id} NEGOTIATE offered={[hex(d) for d in dialects]} chosen={hex(self.dialect)} client_mode={mode} caps={caps:#x}")
        contexts = []
        if self.dialect == 0x0311:
            self.preauth = hashlib.sha512(self.preauth + ctx["raw"]).digest()
            ctx_off, ctx_count = struct.unpack_from("<IH", body, 28)
            pos = ctx_off - 64
            offered = {}
            for _ in range(ctx_count):
                kind, n = struct.unpack_from("<HH", body, pos)
                offered[kind] = body[pos + 8:pos + 8 + n]
                pos += 8 + n
                pos += -pos % 8
            log(f"conn {self.id} negotiate contexts {sorted(offered)}")
            contexts.append((1, struct.pack("<HHH", 1, 32, 1) + os.urandom(32)))
            if 2 in offered and self.args.cipher_zero:
                contexts.append((2, struct.pack("<HH", 1, 0)))
            if 8 in offered:
                contexts.append((8, struct.pack("<HH", 1, 1)))
        return S_OK, self.negotiate_body(self.dialect, contexts), {}

    def session_setup(self, body, ctx):
        _, flags, mode, caps, _, off, n, prev = struct.unpack_from("<HBBIIHHQ", body, 0)
        blob = ctx["raw"][off:off + n]
        sid = ctx["sid"]
        session = self.sessions.get(sid)
        if session is None or session.done:
            sid = self.next_sid
            self.next_sid += 1
            session = Session(sid)
            session.preauth = self.preauth
            self.sessions[sid] = session
        if self.dialect == 0x0311:
            session.preauth = hashlib.sha512(session.preauth + ctx["raw"]).digest()
        token, mech_types, client_mic = spnego_parse(blob)
        kind = struct.unpack_from("<I", token, 8)[0] if token else 0
        if kind == 1:
            session.negotiate_msg = token
            session.challenge = os.urandom(8)
            client_flags = struct.unpack_from("<I", token, 12)[0]
            msg, session.flags = ntlm_challenge(client_flags, session.challenge)
            session.challenge_msg = msg
            session.mech_types = mech_types
            resp = spnego_resp(1, token=msg) if not blob.startswith(b"NTLMSSP") else msg
            out = struct.pack("<HHHH", 9, 0, 72, len(resp)) + resp
            if self.dialect == 0x0311:
                hdr = self.header(SESSION_SETUP, ctx["mid"], ctx["tid"], sid, S_MORE_PROCESSING, ctx["credits"], 0)
                session.preauth = hashlib.sha512(session.preauth + hdr + out).digest()
            return S_MORE_PROCESSING, out, {"sid": sid}
        if kind == 3:
            result = ntlm_authenticate(token, session.challenge, self.server.users)
            if result is None:
                log(f"conn {self.id} SESSION_SETUP logon failure")
                self.sessions.pop(sid, None)
                raise Failed(S_LOGON_FAILURE)
            session.user, exported, nflags = result
            session.key = exported[:16]
            if self.dialect == 0x0311:
                session.signing_key = kdf(session.key, b"SMBSigningKey\x00", session.preauth)
            elif self.dialect >= 0x0300:
                session.signing_key = kdf(session.key, b"SMB2AESCMAC\x00", b"SmbSign\x00")
            else:
                session.signing_key = session.key
            session.done = True
            mic = ntlm_server_mic(exported, nflags, session.mech_types) if client_mic and session.mech_types else None
            resp = spnego_resp(0, mic=mic, mech=False) if not blob.startswith(b"NTLMSSP") else b""
            log(f"conn {self.id} SESSION_SETUP user={session.user} sid={sid:#x} client_mode={mode} signed_req={ctx['signed']} mic={'yes' if client_mic else 'no'}")
            # SMB 3.1.1 signs the final response; earlier dialects sign it when signing is on.
            sign = self.dialect >= 0x0300 or ctx["signed"] or self.args.signing_required
            return S_OK, struct.pack("<HHHH", 9, 0, 72, len(resp)) + resp, {"sid": sid, "sign": sign}
        raise Failed(S_INVALID_PARAMETER)

    def tree_connect(self, body, ctx):
        _, _, off, n = struct.unpack_from("<HHHH", body, 0)
        path = ctx["raw"][off:off + n].decode("utf-16-le")
        share = path.rsplit("\\", 1)[-1]
        log(f"conn {self.id} TREE_CONNECT {path}")
        if share.upper() == "IPC$":
            kind = 2
        elif share.lower() == self.args.share.lower():
            kind = 1
        else:
            raise Failed(S_BAD_NETWORK_NAME)
        tid = self.next_tid
        self.next_tid += 1
        self.trees[tid] = share
        return S_OK, struct.pack("<HBBIII", 16, kind, 0, 0x30 if self.args.no_caching else 0, 0, READ_ACCESS), {"tid": tid}

    # -- files

    def name_of(self, ctx, body, off, n):
        raw = ctx["raw"][off:off + n].decode("utf-16-le")
        name = raw.replace("\\", "/").strip("/")
        if ":" in name:
            base, _, stream = name.partition(":")
            if stream.lower() in ("", ":$data", "$data"):
                return base
            raise Failed(S_OBJECT_NAME_NOT_FOUND, "stream")
        return unicodedata.normalize("NFC", name) if self.args.nfc else name

    async def create(self, body, ctx, tree):
        (_, _, oplock, _, _, _, access, fattrs, share_access, disposition, options, noff, nlen, coff, clen) = \
            struct.unpack_from("<HBBIQQIIIIIHHII", body, 0)
        if tree.upper() == "IPC$":
            raise Failed(S_ACCESS_DENIED)
        path = self.name_of(ctx, body, noff, nlen)
        contexts = parse_contexts(ctx["raw"][coff:coff + clen]) if clen else {}
        names = ",".join(k.decode(errors="replace") for k in contexts)
        try:
            attr = await self.server.backend.stat(path)
        except Missing as e:
            log(f"conn {self.id} CREATE {path!r} disp={disposition} opts={options:#x} access={access:#x} ctx={names} -> {'PATH' if e.path else 'NAME'}_NOT_FOUND")
            if disposition in (0, 2, 3, 5) and not e.path:
                raise Failed(S_MEDIA_WRITE_PROTECTED)
            raise
        if disposition == 2:
            raise Failed(S_OBJECT_NAME_COLLISION)
        if disposition in (0, 4, 5) or access & WRITE_BITS or options & 0x1000:
            log(f"conn {self.id} CREATE {path!r} disp={disposition} access={access:#x} opts={options:#x} -> ACCESS_DENIED (read-only)")
            raise Failed(S_ACCESS_DENIED)
        if options & 0x1 and not attr.is_dir:
            raise Failed(S_NOT_A_DIRECTORY)
        if options & 0x40 and attr.is_dir:
            raise Failed(S_FILE_IS_A_DIRECTORY)
        handle = None
        held = self.held_version(path) if not attr.is_dir else None
        if held:
            # This connection still has an older version open: the new open shares it, and gets
            # no lease, so nothing caches that version past its close.
            handle, attr = held.handle, held.attr
        elif not attr.is_dir:
            handle, attr = await self.server.backend.open(path)
        if handle is not None:
            self.handle_refs[handle] = self.handle_refs.get(handle, 0) + 1
        fid = self.next_fid
        self.next_fid += 1
        o = Open(fid, tree, path, attr, handle, attr.is_dir)
        self.opens[fid] = o
        out_contexts = []
        level = 0
        if b"RqLs" in contexts and not held:
            level, lease_ctx = self.grant_lease(o, contexts[b"RqLs"])
            if lease_ctx:
                out_contexts.append((b"RqLs", lease_ctx))
        if b"MxAc" in contexts:
            out_contexts.append((b"MxAc", struct.pack("<II", 0, READ_ACCESS)))
        if b"QFid" in contexts:
            out_contexts.append((b"QFid", struct.pack("<QQ", attr.ino, 0x5649) + b"\x00" * 16))
        blob = context_blob(out_contexts)
        fixed = struct.pack("<HBBI", 89, level, 0, 1) + basic_info(attr)[:32]
        fixed += struct.pack("<QQII", alloc(attr), 0 if attr.is_dir else attr.size, ATTR_DIRECTORY if attr.is_dir else ATTR_ARCHIVE, 0)
        fixed += struct.pack("<QQ", fid, fid)
        coff_out = 64 + len(fixed) + 8 if blob else 0
        fixed += struct.pack("<II", coff_out, len(blob))
        lease_s = f" lease={o.lease.state:#x}" if o.lease else ""
        log(f"conn {self.id} CREATE {path!r} disp={disposition} opts={options:#x} access={access:#x} ctx={names} -> fid={fid} {'dir' if attr.is_dir else f'size={attr.size}'}{lease_s}")
        return S_OK, fixed + (blob if blob else b"\x00"), {"fid": fid}

    def held_version(self, path):
        """`--hold-open-version`: the newest open of `path` on this connection, whose version the
        path shows while it is open, as FSKit shows an item in use."""
        if not self.args.hold_open_version:
            return None
        for o in reversed(list(self.opens.values())):
            if o.path == path and not o.is_dir and o.handle is not None:
                return o
        return None

    def grant_lease(self, o, data):
        key, state, flags = data[:16], *struct.unpack_from("<II", data, 16)
        v2 = len(data) >= 52
        parent = data[32:48] if v2 else b""
        want = state & (LEASE_R | LEASE_H)
        if self.args.leases == "none" or (o.is_dir and (self.args.leases != "all" or not v2 or self.dialect < 0x0300)):
            want = 0
        lease = self.leases.get(key)
        if lease is None:
            lease = Lease(key, want, 2 if v2 else 1, o.is_dir, o.path, parent)
            if want:
                self.leases[key] = lease
        elif lease.path != o.path:
            want = 0
        else:
            lease.state &= want if lease.breaking else (want | lease.state)
        if not want:
            return 0, None
        lease.opens += 1
        o.lease = lease
        if v2:
            blob = key + struct.pack("<IIQ", lease.state, 0x4 if parent.strip(b"\x00") else 0, 0) + parent + struct.pack("<HH", lease.epoch, 0)
        else:
            blob = key + struct.pack("<IIQ", lease.state, 0, 0)
        return 0xFF, blob

    def fid_of(self, body, off, ctx):
        persistent, volatile = struct.unpack_from("<QQ", body, off)
        if volatile == 0xFFFFFFFFFFFFFFFF and ctx["related"]:
            volatile = ctx["last_fid"]
        o = self.opens.get(volatile)
        if o is None:
            raise Failed(S_FILE_CLOSED)
        return o

    async def release(self, o):
        self.opens.pop(o.fid, None)
        if o.notify:
            await self.complete_notify(o, S_NOTIFY_CLEANUP, b"")
        if o.lease:
            o.lease.opens -= 1
            if o.lease.opens <= 0:
                self.leases.pop(o.lease.key, None)
        if o.handle is not None:
            self.handle_refs[o.handle] = self.handle_refs.get(o.handle, 1) - 1
            if self.handle_refs[o.handle] > 0:
                return
            self.handle_refs.pop(o.handle, None)
            try:
                await self.server.backend.close(o.handle)
            except Exception as e:
                log(f"conn {self.id} close {o.path!r}: {e!r}")

    async def close(self, body, ctx, tree):
        flags = struct.unpack_from("<H", body, 2)[0]
        o = self.fid_of(body, 8, ctx)
        if not o.is_dir or self.args.log_info:
            log(f"conn {self.id} CLOSE {o.path!r} fid={o.fid}")
        await self.release(o)
        if flags & 1:
            try:
                a = await self.server.backend.stat(o.path)
            except (Missing, Failed, DaemonError):
                a = o.attr
            tail = basic_info(a)[:32] + struct.pack("<QQI", alloc(a), 0 if a.is_dir else a.size, ATTR_DIRECTORY if a.is_dir else ATTR_ARCHIVE)
        else:
            tail = b"\x00" * 52
        return S_OK, struct.pack("<HHI", 60, flags & 1, 0) + tail, {}

    async def read(self, body, ctx, tree):
        _, pad, flags, length, offset = struct.unpack_from("<HBBIQ", body, 0)
        o = self.fid_of(body, 16, ctx)
        if o.is_dir:
            raise Failed(S_INVALID_DEVICE_REQUEST)
        if offset >= o.attr.size:
            raise Failed(S_END_OF_FILE)
        if self.args.log_reads:
            log(f"conn {self.id} READ> {o.path!r} off={offset} len={length}")
        try:
            data = await self.server.backend.read(o.handle, offset, min(length, o.attr.size - offset))
        except DaemonError as e:
            if self.args.log_reads:
                log(f"conn {self.id} READ {o.path!r} off={offset} len={length} -> {e} status={e.status:#x}")
            raise
        if not data:
            raise Failed(S_END_OF_FILE)
        if self.args.log_reads:
            log(f"conn {self.id} READ {o.path!r} off={offset} len={length} -> {len(data)}")
        return S_OK, struct.pack("<HBBIII", 17, 80, 0, len(data), 0, 0) + data, {}

    async def query_info(self, body, ctx, tree):
        _, kind, cls, maxlen, ioff, _, ilen, addl, flags = struct.unpack_from("<HBBIHHIII", body, 0)
        o = self.fid_of(body, 24, ctx)
        a = o.attr if o.handle is not None else await self.server.backend.stat(o.path)
        o.attr = a
        if kind == 1:
            name = utf16("\\" + o.path.replace("/", "\\"))
            if cls == 4:
                data = basic_info(a)
            elif cls == 5:
                data = standard_info(a)
            elif cls == 6:
                data = struct.pack("<Q", a.ino)
            elif cls == 7:
                data = struct.pack("<I", 0)
            elif cls == 8:
                data = struct.pack("<I", READ_ACCESS)
            elif cls == 14:
                data = struct.pack("<Q", 0)
            elif cls == 16:
                data = struct.pack("<I", 0)
            elif cls == 17:
                data = struct.pack("<I", 0)
            elif cls == 18:
                data = basic_info(a) + standard_info(a) + struct.pack("<QIIQII", a.ino, 0, READ_ACCESS, 0, 0, 0) + struct.pack("<I", len(name)) + name
            elif cls in (9, 48):
                data = struct.pack("<I", len(name)) + name
            elif cls == 22:
                data = b"" if a.is_dir else struct.pack("<IIQQ", 0, 14, a.size, alloc(a)) + utf16("::$DATA")
            elif cls == 34:
                data = network_open_info(a)
            elif cls == 35:
                data = struct.pack("<II", ATTR_DIRECTORY if a.is_dir else ATTR_ARCHIVE, 0)
            elif cls == 59:
                data = struct.pack("<Q", 0x5649) + struct.pack("<QQ", a.ino, 0)
            else:
                log(f"conn {self.id} QUERY_INFO file class {cls} unsupported")
                raise Failed(S_INVALID_INFO_CLASS)
        elif kind == 2:
            if cls == 1:
                label = utf16(self.args.share)
                data = struct.pack("<QIIBB", self.server.start, 0x5649, len(label), 0, 0) + label
            elif cls == 3:
                data = struct.pack("<QQII", 1 << 28, 1 << 27, 8, 512)
            elif cls == 7:
                data = struct.pack("<QQQII", 1 << 28, 1 << 27, 1 << 27, 8, 512)
            elif cls == 4:
                data = struct.pack("<II", 7, 0)
            elif cls == 5:
                fsname = utf16("NTFS")
                attrs = 0x1 | 0x2 | 0x4 | 0x80000  # case-sensitive search, case-preserved, Unicode, read-only volume
                data = struct.pack("<III", attrs, 255, len(fsname)) + fsname
            elif cls == 11:
                data = struct.pack("<IIIIIII", 512, 512, 512, 512, 0, 0, 0)
            else:
                log(f"conn {self.id} QUERY_INFO fs class {cls} unsupported")
                raise Failed(S_INVALID_INFO_CLASS)
        else:
            log(f"conn {self.id} QUERY_INFO type {kind} class {cls} unsupported")
            raise Failed(S_ACCESS_DENIED if kind == 3 else S_NOT_SUPPORTED)
        status = S_OK
        if len(data) > maxlen:
            data, status = data[:maxlen], S_BUFFER_OVERFLOW
        if self.args.log_info:
            log(f"conn {self.id} QUERY_INFO {o.path!r} type={kind} class={cls} -> size={a.size} mtime={a.mtime_ns}")
        return status, struct.pack("<HHI", 9, 72, len(data)) + data, {}

    async def query_directory(self, body, ctx, tree):
        _, cls, flags, index = struct.unpack_from("<HBBI", body, 0)
        o = self.fid_of(body, 8, ctx)
        noff, nlen, maxlen = struct.unpack_from("<HHI", body, 24)
        pattern = ctx["raw"][noff:noff + nlen].decode("utf-16-le") if nlen else "*"
        if not o.is_dir:
            raise Failed(S_INVALID_PARAMETER)
        first = o.listing is None or flags & (0x1 | 0x10) or pattern != o.pattern
        if first:
            entries = await self.server.backend.list(o.path)
            if o.path:
                me = await self.server.backend.stat(o.path)
                entries = [(".", me), ("..", me)] + entries
            if self.args.hold_open_version:
                entries = [(n, h.attr if (h := self.held_version((o.path + "/" if o.path else "") + n)) else a) for n, a in entries]
            if pattern not in ("*", "<"):
                wild = any(c in pattern for c in "*?<>\"")
                entries = [(n, a) for n, a in entries if (fnmatch.fnmatchcase(n, pattern.replace("<", "*").replace(">", "?")) if wild else n == pattern)]
            o.listing, o.cursor, o.pattern = entries, 0, pattern
            log(f"conn {self.id} QUERY_DIRECTORY {o.path!r} pattern={pattern!r} class={cls} -> {len(entries)} entries")
            if not entries:
                raise Failed(S_NO_SUCH_FILE)
        if o.cursor >= len(o.listing):
            raise Failed(S_NO_MORE_FILES)
        out, size = [], 0
        while o.cursor < len(o.listing):
            name, a = o.listing[o.cursor]
            e = dir_entry(cls, name, a, o.cursor)
            if size + len(e) > maxlen:
                break
            out.append(e)
            size += len(pad8(e))
            o.cursor += 1
            if flags & 0x2:
                break
        if not out:
            raise Failed(S_INFO_LENGTH_MISMATCH)
        data = chain_entries(out)
        return S_OK, struct.pack("<HHI", 9, 72, len(data)) + data, {}

    async def flush(self, body, ctx, tree):
        return S_OK, struct.pack("<HH", 4, 0), {}

    async def lock(self, body, ctx, tree):
        return S_OK, struct.pack("<HH", 4, 0), {}

    async def ioctl(self, body, ctx, tree):
        code = struct.unpack_from("<I", body, 4)[0]
        log(f"conn {self.id} IOCTL {code:#010x} on {tree}")
        if code == 0x00140204:  # FSCTL_VALIDATE_NEGOTIATE_INFO
            caps = 0x4 | (0x2 if self.args.leases != "none" else 0) | (0x20 if self.args.leases == "all" and self.dialect >= 0x0300 else 0)
            out = struct.pack("<I", caps) + self.server.guid + struct.pack("<HH", 0x1 | (0x2 if self.args.signing_required else 0), self.dialect)
            fixed = struct.pack("<HHIQQIIIIII", 49, 0, code, 0xFFFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFF, 112, 0, 112, len(out), 0, 0)
            return S_OK, fixed + out, {"sign": True}
        if code == 0x00060194:  # FSCTL_DFS_GET_REFERRALS
            raise Failed(S_NOT_FOUND)
        raise Failed(S_NOT_SUPPORTED)

    async def change_notify(self, body, ctx, tree):
        _, flags, maxlen = struct.unpack_from("<HHI", body, 0)
        o = self.fid_of(body, 8, ctx)
        cfilter = struct.unpack_from("<I", body, 24)[0]
        if not o.is_dir:
            raise Failed(S_INVALID_PARAMETER)
        if not self.args.notify:
            log(f"conn {self.id} CHANGE_NOTIFY {o.path!r} refused (notifications off)")
            raise Failed(S_NOT_SUPPORTED)
        async_id = self.next_async
        self.next_async += 1
        o.notify = (ctx["mid"], async_id, ctx["sid"], cfilter, bool(flags & 1), maxlen, ctx["signed"])
        self.pending[async_id] = o
        log(f"conn {self.id} CHANGE_NOTIFY {o.path!r} filter={cfilter:#x} tree={bool(flags & 1)} async={async_id} queued={len(o.pending_changes)}")
        if o.pending_changes:
            changes, o.pending_changes = o.pending_changes, []
            asyncio.get_event_loop().call_soon(lambda: asyncio.ensure_future(self.deliver(o, changes)))
        return S_PENDING, error_body(), {"async_id": async_id}

    def cancel(self, ctx):
        o = self.pending.get(ctx["async_id"]) if ctx["async_id"] is not None else next((p for p in self.pending.values() if p.notify and p.notify[0] == ctx["mid"]), None)
        log(f"conn {self.id} CANCEL async={ctx['async_id']} mid={ctx['mid']} -> {'found' if o else 'nothing'}")
        if o is not None:
            asyncio.ensure_future(self.complete_notify(o, S_CANCELLED, b""))

    async def complete_notify(self, o, status, data):
        if not o.notify:
            return
        mid, async_id, sid, _, _, _, signed = o.notify
        o.notify = None
        self.pending.pop(async_id, None)
        body = struct.pack("<HHI", 9, 72, len(data)) + data if (status == S_OK and data) else error_body()
        msg = self.header(CHANGE_NOTIFY, mid, 0, sid, status, 0, 0, async_id=async_id) + body
        session = self.sessions.get(sid)
        if signed and session is not None:
            msg = self.sign(msg, session)
        log(f"conn {self.id} CHANGE_NOTIFY done {o.path!r} status={status:#x} bytes={len(data)}")
        await self.send(msg)

    async def deliver(self, o, changes):
        entries = [struct.pack("<III", 0, action, len(utf16(name))) + utf16(name) for action, name in changes]
        data = chain_entries(entries)
        if len(data) > o.notify[5] or not entries:
            await self.complete_notify(o, S_NOTIFY_ENUM_DIR, b"")
        else:
            await self.complete_notify(o, S_OK, data)

    # -- changes elsewhere → notifications and lease breaks

    def changed(self, path, action, old=None, subtree=False):
        for p, act in ([(old, NOTIFY_RENAMED_OLD), (path, NOTIFY_RENAMED_NEW)] if old else [(path, action)]):
            parent, _, name = p.rpartition("/")
            for o in list(self.opens.values()):
                if not o.is_dir:
                    continue
                rel = None
                if o.path == parent:
                    rel = name
                elif o.notify and o.notify[4] and (o.path == "" or p.startswith(o.path + "/")):
                    rel = p[len(o.path) + 1:] if o.path else p
                if rel is None:
                    continue
                if o.notify:
                    asyncio.ensure_future(self.deliver(o, [(act, rel.replace("/", "\\"))]))
                elif len(o.pending_changes) < 64:
                    o.pending_changes.append((act, rel.replace("/", "\\")))
            for lease in list(self.leases.values()):
                if not lease.state:
                    continue
                hit = (lease.path == p) or (lease.is_dir and lease.path == parent) or (subtree and (lease.path == p or lease.path.startswith(p + "/")))
                if hit:
                    asyncio.ensure_future(self.break_lease(lease, lease.state & ~LEASE_R if lease.is_dir else 0))
            if self.args.break_ancestor and not any(l.state and l.is_dir and l.path == parent for l in self.leases.values()):
                # The folder holds no lease (this Mac never listed it): break the nearest folder
                # above it that does, so the Mac looks the folder up again and sees its new time.
                up = parent
                while up:
                    up = up.rpartition("/")[0]
                    held = [l for l in self.leases.values() if l.state and l.is_dir and l.path == up]
                    if held:
                        for lease in held:
                            asyncio.ensure_future(self.break_lease(lease, lease.state & ~LEASE_R))
                        break

    def changed_all(self):
        for o in list(self.opens.values()):
            if o.is_dir and o.notify:
                asyncio.ensure_future(self.complete_notify(o, S_NOTIFY_ENUM_DIR, b""))
        for lease in list(self.leases.values()):
            if lease.state:
                asyncio.ensure_future(self.break_lease(lease, 0))

    async def break_lease(self, lease, new_state):
        if not self.args.breaks:
            log(f"conn {self.id} lease break skipped (breaks off) {lease.path!r} {lease.state:#x}")
            return
        current = lease.state
        ack = bool(current & (LEASE_H | LEASE_W))
        lease.epoch += 1
        lease.breaking = ack
        lease.state = new_state
        body = struct.pack("<HHI", 44, lease.epoch if lease.version == 2 else 0, 1 if ack else 0) + lease.key + struct.pack("<IIIII", current, new_state, 0, 0, 0)
        hdr = b"\xfeSMB" + struct.pack("<HHIHHII", 64, 0, 0, OPLOCK_BREAK, 0, FLAG_RESPONSE, 0) + struct.pack("<QQQ", 0xFFFFFFFFFFFFFFFF, 0, 0) + b"\x00" * 16
        log(f"conn {self.id} LEASE_BREAK {lease.path!r} {current:#x} -> {new_state:#x} ack={ack}")
        await self.send(hdr + body)

    async def oplock_break(self, body, ctx, tree):
        size = struct.unpack_from("<H", body, 0)[0]
        if size == 36:
            key, state = body[8:24], struct.unpack_from("<I", body, 24)[0]
            lease = self.leases.get(key)
            log(f"conn {self.id} LEASE_BREAK ack {lease.path if lease else '?'!r} state={state:#x}")
            if lease:
                lease.state, lease.breaking = state, False
            return S_OK, struct.pack("<HHI", 36, 0, 0) + key + struct.pack("<IQ", state, 0), {}
        raise Failed(S_INVALID_PARAMETER)


# ---------------------------------------------------------------------------------------------
# Control socket for the local backend: `<action> <path> [<old path>]` lines.

async def control(server, reader, writer):
    while True:
        line = await reader.readline()
        if not line:
            break
        parts = line.decode().rstrip("\n").split("\t")
        action = {"added": NOTIFY_ADDED, "removed": NOTIFY_REMOVED, "modified": NOTIFY_MODIFIED, "renamed": NOTIFY_RENAMED_NEW}[parts[0]]
        if action == NOTIFY_RENAMED_NEW:
            server.changed(parts[2], action, old=parts[1])
        else:
            server.changed(parts[1], action)
        writer.write(b"ok\n")
        await writer.drain()
    writer.close()


async def main():
    global LOG
    p = argparse.ArgumentParser()
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--user", required=True)
    p.add_argument("--password-file", required=True)
    p.add_argument("--share", default="spike")
    p.add_argument("--local")
    p.add_argument("--control")
    p.add_argument("--daemon")
    p.add_argument("--drive")
    p.add_argument("--max-dialect", type=lambda s: int(s, 16), default=0x0311)
    p.add_argument("--leases", choices=["none", "file", "all"], default="all")
    p.add_argument("--no-notify", dest="notify", action="store_false")
    p.add_argument("--no-breaks", dest="breaks", action="store_false")
    p.add_argument("--signing-required", action="store_true")
    p.add_argument("--no-caching", action="store_true", help="tree connect says SMB2_SHAREFLAG_NO_CACHING")
    p.add_argument("--nfc", action="store_true")
    p.add_argument("--frozen-dir-times", action="store_true", help="local backend: report every folder's time as 1970, as the core does for folders")
    p.add_argument("--hold-open-version", action="store_true", help="while this connection has a file open, its path and listing show the version open")
    p.add_argument("--advance-dir-times", action="store_true", help="a folder's time is at least the last change the server learned of in it")
    p.add_argument("--break-ancestor", action="store_true", help="a change in a folder holding no lease breaks the nearest folder above it that holds one")
    p.add_argument("--path-ids", action="store_true", help="a file's ID is made from its inode and path, so a rename gives it a new one")
    p.add_argument("--cipher-zero", action="store_true", help="answer an encryption context with cipher 0 instead of leaving it out")
    p.add_argument("--log-reads", action="store_true")
    p.add_argument("--log-info", action="store_true")
    p.add_argument("--log")
    args = p.parse_args()
    LOG = open(args.log, "a") if args.log else sys.stderr
    if args.local:
        backend = LocalBackend(args.local, args.frozen_dir_times)
    else:
        backend = DaemonBackend(args.daemon, args.drive)
        await backend.start()
    if args.path_ids:
        backend = PathIds(backend)
    server = Server(args, backend)
    if args.advance_dir_times:
        server.backend = AdvanceDirTimes(backend, server.dir_times)

    async def accept(reader, writer):
        c = Conn(server, reader, writer)
        server.conns.append(c)
        await c.run()

    tcp = await asyncio.start_server(accept, "127.0.0.1", args.port)
    log(f"serving {args.local or args.drive!r} as {args.share!r} on 127.0.0.1:{args.port}, max dialect {args.max_dialect:#x}, leases={args.leases}, notify={args.notify}, breaks={args.breaks}")
    tasks = [tcp.serve_forever()]
    if args.control:
        ctl = await asyncio.start_unix_server(lambda r, w: control(server, r, w), args.control)
        tasks.append(ctl.serve_forever())
    if args.daemon:
        tasks.append(backend.watch(server))
    await asyncio.gather(*tasks)


if __name__ == "__main__":
    asyncio.run(main())
