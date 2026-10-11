import os, sys, time, subprocess
mnt, peer, tag = sys.argv[1], sys.argv[2], sys.argv[3]
d = os.path.join(mnt, "probe")
def exists(p):
    try: os.stat(p); return True
    except FileNotFoundError: return False
def wait(p, limit):
    t = time.time()
    while time.time() - t < limit:
        if exists(p): return round((time.time() - t) * 1000)
        time.sleep(0.02)
    return None
def case(name, hold, release):
    os.listdir(d)
    new = os.path.join(d, f"{name}-{tag}.txt")
    token = hold()
    exists(new)
    subprocess.run([peer, "put", f"probe/{name}-{tag}.txt", "x"], check=True, stdout=subprocess.DEVNULL)
    t = wait(new, 6)
    release(token)
    after = wait(new, 6) if t is None else None
    print(f"{name}: visible after {t} ms while held" + ("" if t is not None else f"; {after} ms after release"))
def proc(cmd):
    return lambda: subprocess.Popen(cmd)
def kill(p): p.terminate(); p.wait()
case("cwd", lambda: subprocess.Popen(["/bin/sh", "-c", "cd \"$0\" && exec sleep 30", d]), kill)
case("dir-evtonly", lambda: os.open(d, os.O_RDONLY | 0x8000), os.close)
case("dir-rdonly", lambda: os.open(d, os.O_RDONLY), os.close)
case("subfolder-file", lambda: open(os.path.join(d, "sub", "x.txt")), lambda f: f.close())
case("other-folder-file", lambda: open(os.path.join(mnt, "other", "y.txt")), lambda f: f.close())
case("sibling-file", lambda: open(os.path.join(d, "a.txt")), lambda f: f.close())
case("nothing", lambda: None, lambda _: None)
