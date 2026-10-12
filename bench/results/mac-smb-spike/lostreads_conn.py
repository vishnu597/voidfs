"""lostreads_conn.py SMBSPIKE_PY SOCK [--mib N] [--window W]: a smbspike.py's own Conn.run, fed READs.

Conn.run gets SMB2 READ frames through its StreamReader and starts a task per frame as it does for
smbfs. Conn.handle is replaced by the unbounded spike's READ: DaemonBackend._request with no bound and
no timeout, a Busy READ fed in again after 10 ms. gc.collect() runs every 50 ms. Compare the server
before its tasks were kept (`git show 37c29be:bench/results/mac-smb-spike/smbspike.py`) with this
directory's.
"""
import argparse, asyncio, gc, importlib.util, struct, time, types, urllib.parse

p = argparse.ArgumentParser()
p.add_argument("spike"); p.add_argument("sock")
p.add_argument("--mib", type=int, default=192)
p.add_argument("--window", type=int, default=39)
a = p.parse_args()
spec = importlib.util.spec_from_file_location("smbspike", a.spike)
s = importlib.util.module_from_spec(spec)
spec.loader.exec_module(s)


class Writer:
    def get_extra_info(self, name):
        return "lostreads_conn"

    def close(self):
        pass


def frame(off, n):
    msg = b"\xfeSMB" + struct.pack("<HHIHHIIQ", 64, 0, 0, s.READ, 1, 0, 0, off) + b"\x00" * 32 + struct.pack("<QI", off, n)
    return len(msg).to_bytes(4, "big") + msg


async def main():
    loop = asyncio.get_running_loop()
    destroyed = []
    loop.set_exception_handler(lambda l, c: destroyed.append(c["message"]) if "destroyed but it is pending" in c.get("message", "") else l.default_exception_handler(c))
    b = s.DaemonBackend(a.sock, "spike")
    await b.start()
    fh, _ = await b.open("media/big.bin")
    reader = asyncio.StreamReader()
    started, answered, busy = set(), set(), [0]
    progress = asyncio.Event()

    async def handle(self, data):
        off, n = struct.unpack_from("<QI", data, 64)
        q = urllib.parse.urlencode({"fh": fh, "offset": off, "length": n})
        try:
            await b._request("GET", b._route("read") + "?" + q, raw=True)
        except s.DaemonError as e:
            if e.errno != 35:
                raise
            busy[0] += 1
            loop.call_later(0.01, reader.feed_data, frame(off, n))
            return
        answered.add(off)
        progress.set()
    s.Conn.handle = handle

    server = types.SimpleNamespace(next_conn=1, args=types.SimpleNamespace(), conns=[])
    conn = s.Conn(server, reader, Writer())
    server.conns.append(conn)
    running = asyncio.ensure_future(conn.run())

    async def collector():
        while True:
            await asyncio.sleep(0.05)
            gc.collect()
    collecting = asyncio.ensure_future(collector())

    sizes, off, i, t0, stalled = [512 << 10, 512 << 10, 256 << 10, 1 << 20, 512 << 10], 0, 0, time.monotonic(), False
    while off < a.mib << 20 and not stalled:
        while len(started) - len(answered) >= a.window:
            progress.clear()
            try:
                await asyncio.wait_for(progress.wait(), 15)
            except asyncio.TimeoutError:
                stalled = True
                break
        if not stalled:
            n = min(sizes[i % len(sizes)], (a.mib << 20) - off)
            started.add(off)
            reader.feed_data(frame(off, n))
            off, i = off + n, i + 1
    until = time.monotonic() + 15
    while not stalled and len(answered) < len(started) and time.monotonic() < until:
        await asyncio.sleep(0.2)
    never = sorted(o >> 20 for o in started if o not in answered)
    print(f"{a.spike}: {len(started)} READs, {len(answered)} answered, {busy[0]} Busy sent again, {len(never)} never answered "
          f"(MiB {sorted(set(never))[:12]}), {'stalled' if stalled else 'done'} in {time.monotonic() - t0:.1f} s; "
          f"tasks destroyed while pending {len(destroyed)}", flush=True)
    collecting.cancel()
    reader.feed_eof()
    await running


asyncio.run(main())
