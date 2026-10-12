"""lostreads.py SOCK LOCAL MODE [options]: the unbounded spike's READ path, without SMB.

Each READ is a task started with asyncio.ensure_future and no reference kept, as the spike's
Conn.run started Conn.handle, and it awaits the spike's own DaemonBackend._request with no bound and
no timeout, as the spike did before SPIKE_DAEMON_READS. A Busy READ is sent again after 10 ms, as
smbfs sends one the spike answered STATUS_RETRY. Bytes that reach a connection after its
StreamReader is gone are kept, and at the end each such connection's whole stream is parsed as
replies back to back, to see whether the daemon answered in full.

MODE: natural  the cyclic GC runs when it runs; --garbage and --cpu-ms make each READ cost what the
               SMB side's work did
      collect  gc.collect() every 50 ms
      bound    as collect, through the spike's bound of 24 (DaemonBackend.reads) with no timeout
      timeout  as collect, each call in asyncio.wait_for(..., 60)
      spike    as collect, through this directory's DaemonBackend.read (bound, Busy retried, 10 s timeout)
      kept     as collect, every task kept in a set
"""
import argparse, asyncio, gc, os, sys, time, urllib.parse

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import smbspike as s

p = argparse.ArgumentParser()
p.add_argument("sock"); p.add_argument("local")
p.add_argument("mode", choices=["natural", "collect", "bound", "timeout", "spike", "kept"])
p.add_argument("--mib", type=int, default=192)
p.add_argument("--window", type=int, default=39, help="READs in flight, as smbfs kept")
p.add_argument("--garbage", type=int, default=50, help="objects made per READ answered")
p.add_argument("--cpu-ms", type=float, default=0.0, help="the event loop is busy this long per READ answered")
p.add_argument("--stall", type=float, default=15.0, help="seconds without progress before giving up")
a = p.parse_args()
local = open(a.local, "rb")

streams, orphaned = {}, set()  # id(protocol) -> every byte it received; those received with no reader
_received = asyncio.StreamReaderProtocol.data_received


def data_received(self, data):
    streams.setdefault(id(self), bytearray()).extend(data)
    if self._stream_reader is None:
        orphaned.add(id(self))
    return _received(self, data)


asyncio.StreamReaderProtocol.data_received = data_received


def parse(buf):
    """How many whole replies BUF holds, and whether it ends at a reply's end."""
    pos = replies = 0
    while pos < len(buf):
        end = buf.find(b"\r\n\r\n", pos)
        if end < 0 or not buf.startswith(b"HTTP/1.1 ", pos):
            return replies, False
        n = int([l.split(b":")[1] for l in buf[pos:end].split(b"\r\n") if l.lower().startswith(b"content-length")][0])
        if end + 4 + n > len(buf):
            return replies, False
        pos, replies = end + 4 + n, replies + 1
    return replies, True


async def main():
    loop = asyncio.get_running_loop()
    destroyed = []
    loop.set_exception_handler(lambda l, c: destroyed.append(c["message"]) if "destroyed but it is pending" in c.get("message", "") else l.default_exception_handler(c))
    b = s.DaemonBackend(a.sock, "spike")
    await b.start()
    fh, _ = await b.open("media/big.bin")
    started, answered, wrong, busy, kept = set(), set(), [], [0], set()
    progress = asyncio.Event()

    async def read(off, n):
        q = urllib.parse.urlencode({"fh": fh, "offset": off, "length": n})
        try:
            if a.mode == "spike":
                data = await b.read(fh, off, n)
            elif a.mode == "bound":
                async with b.reads:
                    data = await b._request("GET", b._route("read") + "?" + q, raw=True)
            else:
                call = b._request("GET", b._route("read") + "?" + q, raw=True)
                data = await (asyncio.wait_for(call, 60) if a.mode == "timeout" else call)
        except s.DaemonError as e:
            if e.errno != 35:
                raise
            busy[0] += 1
            loop.call_later(0.01, start, off, n)
            return
        _ = [{"offset": off, "i": i} for i in range(a.garbage)]
        if a.cpu_ms:
            until = time.monotonic() + a.cpu_ms / 1000
            while time.monotonic() < until:
                pass
        local.seek(off)
        if data != local.read(n):
            wrong.append(off)
        answered.add(off)
        progress.set()

    def start(off, n):
        started.add(off)
        task = asyncio.ensure_future(read(off, n))
        if a.mode == "kept":
            kept.add(task); task.add_done_callback(kept.discard)

    async def collector():
        while True:
            await asyncio.sleep(0.05)
            gc.collect()
    if a.mode != "natural":
        collecting = asyncio.ensure_future(collector())

    sizes, off, i, t0, stalled = [512 << 10, 512 << 10, 256 << 10, 1 << 20, 512 << 10], 0, 0, time.monotonic(), False
    while off < a.mib << 20 and not stalled:
        while len(started) - len(answered) >= a.window:
            progress.clear()
            try:
                await asyncio.wait_for(progress.wait(), a.stall)
            except asyncio.TimeoutError:
                stalled = True
                break
        if not stalled:
            n = min(sizes[i % len(sizes)], (a.mib << 20) - off)
            start(off, n)
            off, i = off + n, i + 1
    until = time.monotonic() + a.stall
    while not stalled and len(answered) < len(started) and time.monotonic() < until:
        await asyncio.sleep(0.2)
    gc.collect()
    await asyncio.sleep(1)
    never = sorted(o for o in started if o not in answered)
    parsed = [parse(bytes(streams[k])) for k in orphaned]
    print(f"{a.mode}: {len(started)} READs, {len(answered)} answered, {busy[0]} Busy sent again, {len(never)} never answered "
          f"(MiB {sorted(set(o >> 20 for o in never))[:12]}), {'stalled' if stalled else 'done'} in {time.monotonic() - t0:.1f} s; "
          f"wrong bytes {len(wrong)}; tasks destroyed while pending {len(destroyed)}; "
          f"{len(orphaned)} connections received bytes after their reader was gone, "
          f"{sum(ok for _, ok in parsed)} of them complete replies with nothing missing ({sum(r for r, _ in parsed)} replies)", flush=True)

    c = s.DaemonBackend(a.sock, "spike"); c.info = b.info
    async def probe():
        try:
            await asyncio.wait_for(c._request("GET", c._route("read") + f"?fh={fh}&offset=0&length=1048576", raw=True), 30)
            return True
        except s.DaemonError:
            return False
    res = await asyncio.gather(*[probe() for _ in range(32)])
    print(f"{a.mode}: then 32 reads at once on new connections: {sum(res)} answered, {len(res) - sum(res)} Busy", flush=True)


asyncio.run(main())
