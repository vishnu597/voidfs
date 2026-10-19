#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Disposable loopback fault proxy for the busy-eject checks.

Example:
  /usr/bin/python3 proxy.py --listen-port 9372 --upstream-port 9373 \
      --mode-file /path/to/mode.json

Atomically replace mode.json with an object such as:
  {"fail_mutations": false, "hold_feed": false}

fail_mutations rejects POST/PUT/PATCH/DELETE with an S3-style 503. hold_feed
forwards change-feed response headers immediately, but gates every body chunk,
including chunks on connections opened before the flag changed. Replace the
file with both flags false to recover. Missing/invalid updates retain the last
valid mode. The proxy never logs request headers, bodies, or query strings.
"""

import argparse
import datetime
import http.client
import json
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


HOP_BY_HOP = {
    "connection", "keep-alive", "proxy-authenticate", "proxy-authorization",
    "te", "trailer", "transfer-encoding", "upgrade",
}
MUTATIONS = {"POST", "PUT", "PATCH", "DELETE"}
CHUNK = 64 * 1024


class BadRequest(Exception):
    pass


class Mode:
    def __init__(self, path):
        self.path = path
        self.lock = threading.Lock()
        self.last = {"fail_mutations": False, "hold_feed": False}
        self.last_error = None

    def read(self):
        with self.lock:
            try:
                value = json.loads(self.path.read_text(encoding="utf-8"))
                if not isinstance(value, dict):
                    raise ValueError("mode must be an object")
                flags = {}
                for name in self.last:
                    flag = value.get(name, False)
                    if not isinstance(flag, bool):
                        raise ValueError("mode flags must be booleans")
                    flags[name] = flag
                self.last = flags
                self.last_error = None
            except (OSError, ValueError, UnicodeError) as error:
                # Do not print the exception: it could contain file contents.
                kind = type(error).__name__
                if kind != self.last_error:
                    print("mode update unavailable (%s); retaining last mode" % kind,
                          flush=True)
                    self.last_error = kind
            return dict(self.last)


def excluded(headers):
    names = set(HOP_BY_HOP)
    for value in headers.get_all("Connection", []):
        names.update(token.strip().lower() for token in value.split(","))
    return names


class Proxy(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, port, upstream_port, mode):
        self.upstream_port = upstream_port
        self.mode = mode
        self.stopping = threading.Event()
        super().__init__(("127.0.0.1", port), Handler)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _format, *args):
        # The default logger includes the whole request target, including queries.
        pass

    def record(self, status):
        stamp = datetime.datetime.now(datetime.timezone.utc).isoformat()
        path = self.path.split("?", 1)[0]
        print("%s %s %s %s" % (stamp, self.command, path, status), flush=True)

    def reply(self, status, code, message):
        body = ("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"
                "<Error><Code>%s</Code><Message>%s</Message>"
                "<RequestId>voidfs-local-fault-proxy</RequestId></Error>"
                % (code, message)).encode("utf-8")
        self.send_response_only(status)
        self.send_header("Content-Type", "application/xml")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True
        self.record(status)
        if self.command != "HEAD":
            self.wfile.write(body)
        self.wfile.flush()

    def framing(self):
        transfer = self.headers.get("Transfer-Encoding", "").strip().lower()
        if transfer:
            if transfer != "chunked" or self.headers.get("Content-Length") is not None:
                raise BadRequest()
            return None
        try:
            lengths = self.headers.get_all("Content-Length", [])
            if len(set(lengths)) > 1:
                raise BadRequest()
            length = int(lengths[0]) if lengths else 0
            if length < 0:
                raise BadRequest()
            return length
        except ValueError:
            raise BadRequest() from None

    def exact(self, length):
        while length:
            data = self.rfile.read(min(CHUNK, length))
            if not data:
                raise BadRequest()
            length -= len(data)
            yield data

    def body(self, length):
        if length is not None:
            yield from self.exact(length)
            return
        while True:
            line = self.rfile.readline(CHUNK + 1)
            if len(line) > CHUNK or not line.endswith(b"\r\n"):
                raise BadRequest()
            try:
                size = int(line.split(b";", 1)[0], 16)
            except ValueError:
                raise BadRequest() from None
            if size < 0:
                raise BadRequest()
            if size == 0:
                # Trailer fields are hop-by-hop and are not forwarded.
                while True:
                    trailer = self.rfile.readline(CHUNK + 1)
                    if trailer == b"\r\n":
                        return
                    if len(trailer) > CHUNK or not trailer.endswith(b"\r\n"):
                        raise BadRequest()
            yield from self.exact(size)
            if self.rfile.read(2) != b"\r\n":
                raise BadRequest()

    def wait_for_feed(self):
        while self.server.mode.read()["hold_feed"]:
            if self.server.stopping.wait(0.025):
                return False
        return not self.server.stopping.is_set()

    def forward(self):
        upstream = None
        headers_sent = False
        try:
            if not self.path.startswith("/"):
                raise BadRequest()
            length = self.framing()
            mode = self.server.mode.read()
            if self.command in MUTATIONS and mode["fail_mutations"]:
                # Drain framing so a small upload gets the valid 503 rather than a
                # reset while it is still sending its request body.
                for _ in self.body(length):
                    pass
                self.reply(503, "ServiceUnavailable", "Injected transient failure")
                return

            upstream = http.client.HTTPConnection(
                "127.0.0.1", self.server.upstream_port, timeout=120)
            upstream.putrequest(self.command, self.path,
                                skip_host=True, skip_accept_encoding=True)
            skip = excluded(self.headers)
            for name, value in self.headers.items():
                if name.lower() not in skip:
                    # In particular, keep the original Host and the exact target:
                    # the client's signature covers both.
                    upstream.putheader(name, value)
            if length is None:
                upstream.putheader("Transfer-Encoding", "chunked")
            upstream.putheader("Connection", "close")
            upstream.endheaders()
            for data in self.body(length):
                if length is None:
                    upstream.send(("%x\r\n" % len(data)).encode("ascii"))
                upstream.send(data)
                if length is None:
                    upstream.send(b"\r\n")
            if length is None:
                upstream.send(b"0\r\n\r\n")

            response = upstream.getresponse()
            query = urllib.parse.urlsplit(self.path).query
            feed = (any(name == "x-voidfs-changes" for name, _ in
                        urllib.parse.parse_qsl(query, keep_blank_values=True))
                    or response.getheader("Content-Type", "").split(";", 1)[0]
                    .strip().lower() == "text/event-stream")
            feed = feed and 200 <= response.status < 300
            self.send_response_only(response.status, response.reason)
            skip = excluded(response.headers)
            for name, value in response.getheaders():
                if name.lower() not in skip:
                    self.send_header(name, value)
            # HTTPResponse decodes upstream chunk framing. Closing this connection
            # delimits responses without Content-Length, including streamed feeds.
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.flush()
            self.close_connection = True
            headers_sent = True
            self.record(response.status)
            if self.command == "HEAD":
                return
            while not self.server.stopping.is_set():
                if feed and not self.wait_for_feed():
                    return
                data = response.read1(CHUNK)
                if not data:
                    return
                # Recheck after reading: an existing feed can be held while its
                # next chunk was waiting upstream.
                if feed and not self.wait_for_feed():
                    return
                self.wfile.write(data)
                self.wfile.flush()
        except BadRequest:
            if not headers_sent:
                self.reply(400, "InvalidRequest", "Invalid request framing")
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            pass
        except (OSError, http.client.HTTPException):
            if not headers_sent:
                self.reply(502, "BadGateway", "Local upstream unavailable")
        finally:
            self.close_connection = True
            if upstream is not None:
                upstream.close()

    do_GET = forward
    do_HEAD = forward
    do_POST = forward
    do_PUT = forward
    do_PATCH = forward
    do_DELETE = forward
    do_OPTIONS = forward


def port(value):
    number = int(value)
    if not 1 <= number <= 65535:
        raise argparse.ArgumentTypeError("port must be between 1 and 65535")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--listen-port", type=port, default=9372)
    parser.add_argument("--upstream-port", type=port, default=9373)
    parser.add_argument("--mode-file", type=Path, required=True)
    args = parser.parse_args()
    if args.listen_port == args.upstream_port:
        parser.error("listen and upstream ports must differ")
    mode = Mode(args.mode_file)
    mode.read()
    with Proxy(args.listen_port, args.upstream_port, mode) as server:
        print("loopback proxy 127.0.0.1:%s -> 127.0.0.1:%s"
              % (args.listen_port, args.upstream_port), flush=True)
        try:
            server.serve_forever(poll_interval=0.1)
        except KeyboardInterrupt:
            pass
        finally:
            server.stopping.set()


if __name__ == "__main__":
    main()
