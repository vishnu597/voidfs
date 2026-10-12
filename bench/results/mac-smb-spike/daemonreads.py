"""daemonreads.py SOCK LOCAL [options]: smbfs's READs straight at the daemon's /v1/fs session.

A sliding window of sequential READs of 256 KiB, 512 KiB and 1 MiB on drive `spike`'s
media/big.bin, over pooled HTTP/1.1 connections, as the unbounded spike sent them: no bound on
calls, and a Busy READ sent again. Every task is kept and every reply is checked against LOCAL
(mount_seed's copy). A watchdog reports a call with no reply after --stall seconds; at the end,
40 getattrs at once show the daemon's free call slots (32 answered, 8 Busy when none is lost).
"""
import argparse, asyncio, collections, json, random, time

p = argparse.ArgumentParser()
p.add_argument("sock"); p.add_argument("local")
p.add_argument("--window", type=int, default=39, help="READs in flight")
p.add_argument("--mib", type=int, default=192, help="how much of the file to read")
p.add_argument("--handles", type=int, default=1, help="open handles the READs take turns on")
p.add_argument("--busy-delay", type=float, default=0.0, help="seconds before a Busy READ is sent again")
p.add_argument("--meta", type=float, default=0.0, help="getattrs mixed in, per READ")
p.add_argument("--cpu-ms", type=float, default=0.0, help="the event loop is busy this long per READ answered")
p.add_argument("--chunk", type=int, default=0, help="read bodies this many bytes at a time")
p.add_argument("--stall", type=float, default=20.0)
p.add_argument("--seed", type=int, default=1)
a = p.parse_args()
random.seed(a.seed)
local = open(a.local, "rb")

pool, info, stats, pending = [], {}, collections.Counter(), {}


async def request(method, path, body=None, raw=False, what=None):
    if pool:
        reader, writer = pool.pop()
    else:
        reader, writer = await asyncio.open_unix_connection(a.sock)
        stats["connections"] += 1
    data = b"" if body is None else json.dumps(body).encode()
    head = f"{method} {path} HTTP/1.1\r\nhost: localhost\r\ncontent-length: {len(data)}\r\ncontent-type: application/json\r\n"
    if info:
        head += f"x-voidfs-generation: {info['generation']}\r\n"
    key = object()
    pending[key] = (what or path, time.monotonic(), reader)
    try:
        writer.write(head.encode() + b"\r\n" + data)
        await writer.drain()
        status = int((await reader.readline()).split()[1])
        length = 0
        while True:
            line = await reader.readline()
            if line in (b"\r\n", b""):
                break
            k, _, v = line.decode().partition(":")
            if k.strip().lower() == "content-length":
                length = int(v)
        if a.chunk:
            payload = bytearray()
            while len(payload) < length:
                payload += await reader.readexactly(min(a.chunk, length - len(payload)))
                await asyncio.sleep(0)
            payload = bytes(payload)
        else:
            payload = await reader.readexactly(length)
    finally:
        pending.pop(key)
    pool.append((reader, writer))
    if status >= 300:
        return status, json.loads(payload)["error"]
    return status, (payload if raw else json.loads(payload or b"{}"))


def route(op):
    return f"/v1/fs/{info['id']}/{op}"


async def watchdog():
    reported = set()
    while True:
        await asyncio.sleep(1)
        for what, t0, reader in list(pending.values()):
            if time.monotonic() - t0 > a.stall and (what, t0) not in reported:
                reported.add((what, t0))
                stats["no reply"] += 1
                print(f"no reply in {a.stall:.0f} s: {what}; {len(reader._buffer)} bytes waiting on its connection", flush=True)


async def main():
    global info
    s, info = await request("POST", "/v1/fs/sessions", {"version": 1, "drive": "spike", "readOnly": True})
    assert s == 200, info
    ino = info["root"]
    for name in ("media", "big.bin"):
        s, attr = await request("POST", route("lookup"), {"parent": ino, "name": name})
        assert s == 200, attr
        ino = attr["ino"]
    fhs = []
    for _ in range(a.handles):
        s, r = await request("POST", route("open"), {"ino": ino, "write": False})
        assert s == 200, r
        fhs.append(r["fh"])
    dog = asyncio.ensure_future(watchdog())

    pieces, off = [], 0
    while off < a.mib << 20:
        n = min(random.choice([512 << 10, 512 << 10, 256 << 10, 1 << 20, 512 << 10]), (a.mib << 20) - off)
        pieces.append((off, n)); off += n
    window = asyncio.Semaphore(a.window)
    inflight, peak, wrong, latency = 0, 0, [], []

    async def meta():
        s, _ = await request("POST", route("getattr"), {"ino": ino}, what="getattr")
        stats["getattr busy" if s == 503 else ("getattr ok" if s == 200 else f"getattr {s}")] += 1

    async def read(i, off, n):
        nonlocal inflight
        fh = fhs[i % len(fhs)]
        try:
            while True:
                t0 = time.monotonic()
                s, r = await request("GET", route("read") + f"?fh={fh}&offset={off}&length={n}", raw=True,
                                     what=f"read fh={fh} offset={off} length={n}")
                if s == 200:
                    if a.cpu_ms:
                        until = time.monotonic() + a.cpu_ms / 1000
                        while time.monotonic() < until:
                            pass
                    latency.append(time.monotonic() - t0)
                    local.seek(off)
                    if r != local.read(n):
                        wrong.append(off)
                    stats["read ok"] += 1
                    return
                if s == 503 and r.get("errno") == 35:
                    stats["read busy"] += 1
                    await asyncio.sleep(a.busy_delay)
                    continue
                stats[f"read {s} {r.get('code')}"] += 1
                return
        finally:
            inflight -= 1
            window.release()

    t0, tasks = time.monotonic(), []
    for i, (off, n) in enumerate(pieces):
        await window.acquire()
        inflight += 1; peak = max(peak, inflight)
        tasks.append(asyncio.ensure_future(read(i, off, n)))
        if a.meta and random.random() < a.meta:
            tasks.append(asyncio.ensure_future(meta()))
    await asyncio.gather(*tasks)
    dt = time.monotonic() - t0
    dog.cancel()
    latency.sort()
    print(f"{len(pieces)} READs over {a.mib} MiB in {dt:.1f} s, {a.handles} fh, at most {peak} in flight; "
          f"{dict(stats)}; wrong bytes at MiB {[o >> 20 for o in wrong][:10]}; "
          f"latency median {latency[len(latency) // 2] * 1000:.0f} ms, max {latency[-1] * 1000:.0f} ms", flush=True)
    res = await asyncio.gather(*[request("POST", route("getattr"), {"ino": ino}) for _ in range(40)])
    print("then 40 getattrs at once:", dict(collections.Counter(s for s, _ in res)), flush=True)
    for fh in fhs:
        await request("POST", route("close"), {"fh": fh})
    await request("POST", route("release"), {})


asyncio.run(main())
