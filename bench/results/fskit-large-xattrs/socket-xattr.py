# SPDX-License-Identifier: Apache-2.0
"""Sets and reads large xattr values through the daemon's filesystem socket (/v1/fs), without
FSKit, the module or the agent: the Rust half of the probe's path.

/usr/bin/python3 socket-xattr.py <daemon.sock> <drive> <xattr-probe> <out.jsonl>

One process owns the session (the daemon binds a session to its creator's pid).
"""
import hashlib
import http.client
import json
import socket
import subprocess
import sys
import time


class Unix(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__('localhost', timeout=300)
        self.path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(300)
        self.sock.connect(self.path)


def value(probe, size, seed):
    # The probe's stream, regenerated in C and read back from an APFS file.
    import ctypes
    import tempfile
    import os
    d = tempfile.mkdtemp(prefix='vfx-val.', dir='/private/tmp')
    path = os.path.join(d, 'v')
    subprocess.run([probe, 'make', path, 'user.v', str(size), str(seed)], check=True, capture_output=True)
    libc = ctypes.CDLL('/usr/lib/libc.dylib')
    n = libc.getxattr(path.encode(), b'user.v', None, 0, 0, 0)
    buf = ctypes.create_string_buffer(max(n, 1))
    got = libc.getxattr(path.encode(), b'user.v', buf, n, 0, 0)
    assert got == size, (got, size)
    os.unlink(path)
    os.rmdir(d)
    return buf.raw[:n]


def main():
    sock, drive, probe, out = sys.argv[1:5]
    log = open(out, 'a')
    c = Unix(sock)

    def call(method, target, body=b'', ctype=None, gen=None):
        headers = {}
        if ctype:
            headers['Content-Type'] = ctype
        if gen is not None:
            headers['x-voidfs-generation'] = str(gen)
        start = time.monotonic()
        c.request(method, target, body=body, headers=headers)
        r = c.getresponse()
        data = r.read()
        return r.status, data, time.monotonic() - start

    status, data, _ = call('POST', '/v1/fs/sessions', json.dumps({'version': 1, 'drive': drive, 'readOnly': False}).encode(), 'application/json')
    s = json.loads(data)
    sid, gen, root = s['id'], s['generation'], s['root']
    record = {'case': 'session', 'status': status, 'maxXattrBytes': s['capabilities']['maxXattrBytes']}
    print(json.dumps(record), file=log, flush=True)

    status, data, _ = call('POST', f'/v1/fs/{sid}/create', json.dumps({'parent': root, 'name': f'socket-{int(time.time())}.bin', 'mode': 0o644}).encode(), 'application/json', gen)
    ino = json.loads(data)['ino']
    for name, size, seed in [('user.big', 16 << 20, 1606), ('com.apple.ResourceFork', 16 << 20, 1607), ('user.over', (16 << 20) + 1, 1608)]:
        v = value(probe, size, seed)
        want = hashlib.sha256(v).hexdigest()
        st, body, t = call('PUT', f'/v1/fs/{sid}/setxattr?ino={ino}&name={name}&how=set', v, 'application/octet-stream', gen)
        rec = {'case': 'setxattr', 'name': name, 'bytes': size, 'sha256': want, 'status': st, 'seconds': round(t, 3), 'answer': body[:200].decode(errors='replace')}
        if st == 200:
            st2, back, t2 = call('GET', f'/v1/fs/{sid}/getxattr?ino={ino}&name={name}', gen=gen)
            rec.update({'getStatus': st2, 'getBytes': len(back), 'getSha256': hashlib.sha256(back).hexdigest(), 'getSeconds': round(t2, 3), 'whole': hashlib.sha256(back).hexdigest() == want})
        print(json.dumps(rec), file=log, flush=True)
    st, body, _ = call('POST', f'/v1/fs/{sid}/listxattr', json.dumps({'ino': ino}).encode(), 'application/json', gen)
    print(json.dumps({'case': 'listxattr', 'status': st, 'answer': json.loads(body)}), file=log, flush=True)
    st, body, _ = call('POST', f'/v1/fs/{sid}/getattr', json.dumps({'ino': ino}).encode(), 'application/json', gen)
    print(json.dumps({'case': 'getattr', 'status': st, 'answer': json.loads(body)}), file=log, flush=True)
    st, body, _ = call('POST', f'/v1/fs/{sid}/release', b'{}', 'application/json', gen)
    print(json.dumps({'case': 'release', 'status': st, 'answer': body[:200].decode(errors='replace')}), file=log, flush=True)


main()
