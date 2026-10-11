import os, sys, time, subprocess
mnt, peer = sys.argv[1], sys.argv[2]
tag = sys.argv[3] if len(sys.argv) > 3 else str(int(time.time()))
def exists(p):
    try: os.stat(p); return True
    except FileNotFoundError: return False
def wait(p, limit):
    t = time.time()
    while time.time() - t < limit:
        if exists(p): return time.time() - t
        time.sleep(0.02)
    return None
def case(name, hold):
    d = os.path.join(mnt, "probe")
    os.listdir(d)
    new = os.path.join(d, f"{name}-{tag}.txt")
    held = open(os.path.join(d, "a.txt")) if hold else None
    print(f"{name}: negative lookup first: exists={exists(new)}")
    subprocess.run([peer, "put", f"probe/{name}-{tag}.txt", "x"], check=True, stdout=subprocess.DEVNULL)
    t = wait(new, float(sys.argv[4]) if len(sys.argv) > 4 else 15)
    print(f"{name}: hold={hold} visible after {t if t is None else round(t*1000)} ms; in listdir: {os.path.basename(new) in os.listdir(d)}")
    if held:
        held.close()
        t = wait(new, 10)
        print(f"{name}: after the sibling closed, visible after {t if t is None else round(t*1000)} ms")
case("free", False)
case("held", True)
