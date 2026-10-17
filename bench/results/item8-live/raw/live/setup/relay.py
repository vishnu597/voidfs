#!/usr/bin/python3
"""A TCP relay between the daemon and a local test server, for links that go bad.

    /usr/bin/python3 relay.py <listen port> <server port>

Forwards each connection. SIGUSR1: answer every new request `503 Slow Down` instead (a degraded
link: the server answers, with errors); SIGUSR2: forward again. SIGSTOP hangs it (no answer),
SIGCONT resumes it, and killing it refuses connections (offline).
"""

import signal
import socket
import sys
import threading

listen, target = int(sys.argv[1]), int(sys.argv[2])
failing = False
open_sockets = set()


def toggle(on):
    def handler(_sig, _frame):
        global failing
        failing = on
        # Connections kept alive would go on forwarding: close them, so the next request opens one.
        for c in list(open_sockets):
            try:
                c.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
    return handler


signal.signal(signal.SIGUSR1, toggle(True))
signal.signal(signal.SIGUSR2, toggle(False))


def pipe(a, b):
    try:
        while True:
            data = a.recv(65536)
            if not data:
                break
            b.sendall(data)
    except OSError:
        pass
    finally:
        open_sockets.discard(a)
        open_sockets.discard(b)
        for s in (a, b):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass


def serve(client):
    if failing:
        try:
            client.recv(65536)
            body = b"<Error><Code>SlowDown</Code><Message>relay says slow down</Message></Error>"
            client.sendall(b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/xml\r\nConnection: close\r\nContent-Length: "
                           + str(len(body)).encode() + b"\r\n\r\n" + body)
        except OSError:
            pass
        client.close()
        return
    try:
        server = socket.create_connection(("127.0.0.1", target))
    except OSError:
        client.close()
        return
    open_sockets.update((client, server))
    threading.Thread(target=pipe, args=(client, server), daemon=True).start()
    threading.Thread(target=pipe, args=(server, client), daemon=True).start()


s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", listen))
s.listen(128)
while True:
    c, _ = s.accept()
    serve(c)
