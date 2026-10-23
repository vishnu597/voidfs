#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Forwards HTTP/1.1 to a voidfs server and logs every request: step 5, item 9's request counts.

    /usr/bin/python3 -I apps/macos/scripts/request-relay.py <listen port> <upstream host:port> <log.jsonl> [--delay-file <path>]

The daemon reaches the server through this relay, so the log holds each request this Mac made,
which the server's own metrics lump together as `extension`. One JSON line per request, written
when its response has been forwarded:

    {"t": arrival, epoch seconds, "conn": connection number, "method", "path",
     "query": {name: value, ...}, "status", "ms": arrival to the response's end, "bytes": response body}

The change feed's event stream never ends, so it is logged when its response starts, with
`"stream": true`, `ms` to its headers and `bytes` 0.

Query values are cut to 200 characters, and presigned `X-Amz-*` names are left out, so no
signature is logged; headers, where the key is, are never logged.

`--delay-file` names a file holding a number of milliseconds (no file: 0), read at every request;
each request is held that long before it is forwarded, as a round trip to a remote server would
hold it. Concurrent requests on separate connections are held at the same time.
"""

import asyncio
import json
import os
import sys
import time
from urllib.parse import parse_qsl, urlsplit

LIMIT = 1 << 20
CHUNK = 1 << 16


def delay(path):
    if not path:
        return 0.0
    try:
        with open(path) as f:
            return max(0.0, float(f.read().strip() or 0)) / 1000
    except FileNotFoundError:
        return 0.0


def headers(head):
    """The start line and the lower-cased headers of a request's or a response's head."""
    lines = head.decode("latin-1").split("\r\n")
    fields = {}
    for line in lines[1:]:
        if ":" in line:
            name, value = line.split(":", 1)
            fields[name.strip().lower()] = value.strip()
    return lines[0], fields


async def body(reader, writer, fields, until_eof=False):
    """Copies a message body: by Content-Length, chunked, or (a response only) to the end. Returns its length."""
    n = 0
    if "chunked" in fields.get("transfer-encoding", "").lower():
        while True:
            line = await reader.readuntil(b"\r\n")
            writer.write(line)
            size = int(line.split(b";")[0].strip(), 16)
            if size == 0:
                # Trailers, then the empty line.
                while True:
                    line = await reader.readuntil(b"\r\n")
                    writer.write(line)
                    if line == b"\r\n":
                        return n
            left = size + 2
            while left:
                data = await reader.readexactly(min(left, CHUNK))
                writer.write(data)
                left -= len(data)
            n += size
            await writer.drain()
    if "content-length" in fields:
        left = int(fields["content-length"])
        while left:
            data = await reader.readexactly(min(left, CHUNK))
            writer.write(data)
            left -= len(data)
            n += len(data)
            await writer.drain()
        return n
    if until_eof:
        while data := await reader.read(CHUNK):
            writer.write(data)
            n += len(data)
            await writer.drain()
    return n


class Relay:
    def __init__(self, upstream, log, delay_file):
        self.host, port = upstream.rsplit(":", 1)
        self.port = int(port)
        self.log = open(log, "a", buffering=1)
        self.delay_file = delay_file
        self.conns = 0
        # asyncio keeps only a weak reference to a connection's task.
        self.tasks = set()

    def line(self, t, conn, method, target, status, n, **more):
        url = urlsplit(target)
        query = {k: v[:200] for k, v in parse_qsl(url.query, keep_blank_values=True) if not k.lower().startswith("x-amz-")}
        self.log.write(json.dumps({"t": round(t, 6), "conn": conn, "method": method, "path": url.path, "query": query,
                                   "status": status, "ms": round((time.time() - t) * 1000, 3), "bytes": n, **more}) + "\n")

    async def serve(self, creader, cwriter):
        self.tasks.add(asyncio.current_task())
        try:
            await self.forward(creader, cwriter)
        finally:
            self.tasks.discard(asyncio.current_task())

    async def forward(self, creader, cwriter):
        self.conns += 1
        conn = self.conns
        try:
            ureader, uwriter = await asyncio.open_connection(self.host, self.port, limit=LIMIT)
        except OSError:
            cwriter.close()
            return
        try:
            while True:
                try:
                    head = await creader.readuntil(b"\r\n\r\n")
                except (asyncio.IncompleteReadError, ConnectionError):
                    return
                t = time.time()
                start, fields = headers(head)
                method, target = start.split(" ")[:2]
                wait = delay(self.delay_file)
                if wait:
                    await asyncio.sleep(wait)
                uwriter.write(head)
                await body(creader, uwriter, fields)
                await uwriter.drain()
                while True:
                    rhead = await ureader.readuntil(b"\r\n\r\n")
                    status_line, rfields = headers(rhead)
                    status = int(status_line.split(" ")[1])
                    cwriter.write(rhead)
                    if status >= 200:
                        break
                    await cwriter.drain()
                close = rfields.get("connection", "").lower() == "close"
                # The change feed's event stream doesn't end: it is logged when it opens.
                stream = rfields.get("content-type", "").startswith("text/event-stream")
                if stream:
                    self.line(t, conn, method, target, status, 0, stream=True)
                if method == "HEAD" or status in (204, 304):
                    n = 0
                else:
                    framed = "content-length" in rfields or "chunked" in rfields.get("transfer-encoding", "").lower()
                    n = await body(ureader, cwriter, rfields, until_eof=not framed)
                    close = close or not framed
                await cwriter.drain()
                if not stream:
                    self.line(t, conn, method, target, status, n)
                if close:
                    return
        except (asyncio.IncompleteReadError, ConnectionError, ValueError, IndexError):
            return
        finally:
            for w in (cwriter, uwriter):
                try:
                    w.close()
                except Exception:
                    pass


async def main():
    args = sys.argv[1:]
    delay_file = None
    if "--delay-file" in args:
        i = args.index("--delay-file")
        delay_file = args[i + 1]
        del args[i:i + 2]
    port, upstream, log = args
    relay = Relay(upstream, log, delay_file)
    server = await asyncio.start_server(relay.serve, "127.0.0.1", int(port), limit=LIMIT)
    print(f"relay 127.0.0.1:{port} -> {upstream}, pid {os.getpid()}", flush=True)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
