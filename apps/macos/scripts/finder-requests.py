#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""The requests a mounted drive makes when Finder and the shell list big folders: step 5, item 9.

    /usr/bin/python3 apps/macos/scripts/finder-requests.py --mnt <mountpoint> --log <relay log> \
        --cold <command> --remount <command> [--delay-file <path> --delay-ms <n>] [--tag <folder>] \
        [--folders plain-1k,xattrs-1k,...] [--scenarios finder-list,ls,...] [--quiet <s>] [--out <results.jsonl>]

The daemon must reach the server through `request-relay.py`, whose log this reads; the drive is
the one `finder_seed` fills, with the folders under `--tag` if given. `--cold` must empty the
daemon's cache and namespace and mount the drive again at `--mnt`, so nothing is cached anywhere;
`--remount` must unmount and mount it again with the daemon's state kept, so only the kernel and
the module start cold. `--delay-ms` is written to `--delay-file` for the whole run, so each
request waits that long in the relay.

Each step prints one line, and `--out` appends one JSON record for it. A step starts just before
its action and ends when no request has finished for `--quiet` seconds (the feed's stream
excepted): Finder reads xattrs seconds after it lists a folder. A step's requests are split into
the folder's own (its listing pages and requests for its files), counted by kind, and the rest,
counted by folder: after a mount, Finder lists again the folders it has shown before and reads
their visible files' xattrs, with no window open on them. The span runs from the action's start
to the end of the folder's last request. Scenarios:

    finder-list, finder-icon, finder-column
        `cold`: a new 1000 x 700 point Finder window in that view, pointed at the folder. Then
        `reopen`: the window closed and a new one opened on the same mount. Then `remount`: the
        window closed, the drive remounted with the daemon's state kept, and a new window opened.
        Before `cold` and `remount`, a `mount` step records what the mount alone brought.
        Finder remembers a folder it has shown (its scroll position, say), so a cold run needs
        folders this Finder has never opened: a new `--tag`.
    ls        Cold: `ls -l@`, every file's xattr names and sizes.
    bulkstat  Cold: getattrlistbulk over the folder, Finder's listing call, without xattrs.
"""

import argparse
import json
import os
import subprocess
import sys
import time

here = os.path.dirname(os.path.abspath(__file__))
p = argparse.ArgumentParser()
p.add_argument("--mnt", required=True)
p.add_argument("--log", required=True)
p.add_argument("--cold", required=True)
p.add_argument("--remount", required=True)
p.add_argument("--delay-file")
p.add_argument("--delay-ms", type=float, default=0)
p.add_argument("--tag", default="")
p.add_argument("--folders", default="plain-1k,xattrs-1k,plain-10k,xattrs-10k")
p.add_argument("--scenarios", default="finder-list,finder-icon,ls,bulkstat")
p.add_argument("--quiet", type=float, default=10.0)
p.add_argument("--limit", type=float, default=900.0)
p.add_argument("--out")
args = p.parse_args()


def kind(r):
    q = r["query"]
    for name in ("x-voidfs-list", "x-voidfs-attrs", "x-voidfs-drive"):
        if name in q:
            return name[len("x-voidfs-"):]
    if any(k.startswith("x-voidfs-") for k in q):
        return "other-extension"
    if r["method"] == "GET" and r["path"].strip("/").count("/") >= 1:
        return "get"
    return r["method"].lower()


def where(r):
    """The folder a request is about: a listing's prefix, or the folder of the key it names."""
    if "x-voidfs-list" in r["query"]:
        return r["query"].get("prefix", "")
    parts = r["path"].lstrip("/").split("/", 1)
    if len(parts) < 2:
        return ""
    return parts[1].rsplit("/", 1)[0] + "/" if "/" in parts[1] else ""


def lines_from(offset):
    with open(args.log, "rb") as f:
        f.seek(offset)
        data = f.read()
    # The feed's stream opening isn't a request of any step's.
    return [r for r in (json.loads(l) for l in data.decode().splitlines() if l.strip()) if not r.get("stream")]


def size():
    return os.path.getsize(args.log)


def measure(name, folder, action, view=None):
    """Runs `action`, waits for the requests to stop, and records them."""
    offset = size()
    start = time.time()
    took = action()
    last, quiet_since = size(), time.time()
    while time.time() - start < args.limit:
        time.sleep(0.1)
        now = size()
        if now != last:
            new = lines_from(last)
            last = now
            if new:
                quiet_since = time.time()
        elif time.time() - quiet_since >= args.quiet:
            break
    rows = lines_from(offset)
    prefix = os.path.join(args.tag, folder) + "/"
    own = [r for r in rows if where(r) == prefix]
    counts, elsewhere = {}, {}
    for r in own:
        counts[kind(r)] = counts.get(kind(r), 0) + 1
    for r in rows:
        if where(r) != prefix:
            w = where(r) or "/"
            elsewhere[w] = elsewhere.get(w, 0) + 1
    attrs = [r["path"] for r in own if kind(r) == "attrs"]
    ends = [r["t"] + r["ms"] / 1000 for r in own]
    # The most of the folder's requests in flight at once, from their arrivals and ends.
    edges = sorted([(r["t"], 1) for r in own] + [(e, -1) for e in ends])
    most = cur = 0
    for _, d in edges:
        cur += d
        most = max(most, cur)
    rec = {
        "scenario": name, "folder": prefix.rstrip("/"), "view": view, "delay_ms": args.delay_ms,
        "requests": len(own), "by_kind": dict(sorted(counts.items())),
        "attrs_files": len(set(attrs)), "max_in_flight": most,
        "first_s": round(min(r["t"] for r in own) - start, 3) if own else None,
        "span_s": round(max(ends) - start, 3) if own else None,
        "command_s": round(took, 3) if took is not None else None,
        "elsewhere": dict(sorted(elsewhere.items())),
        "errors": sum(1 for r in rows if r["status"] >= 400),
        "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(start)),
    }
    kinds = " ".join(f"{k}={v}" for k, v in rec["by_kind"].items()) or "none"
    span = f"{rec['span_s']:.3f} s" if rec["span_s"] is not None else "-"
    cmd = f", command {rec['command_s']:.3f} s" if rec["command_s"] is not None else ""
    other = sum(elsewhere.values())
    print(f"{rec['folder']:<15} {name:<22} {len(own):>6} requests ({kinds}; {rec['attrs_files']} files' attrs; "
          f"{most} at once), span {span}{cmd}; {other} elsewhere", flush=True)
    if args.out:
        with open(args.out, "a") as f:
            f.write(json.dumps(rec) + "\n")
    return rec


def sh(command):
    subprocess.run(command, shell=True, check=True)


def osa(script, *argv):
    out = subprocess.run(["osascript", "-e", script, *argv], check=True, capture_output=True, text=True)
    return out.stdout.strip()


OPEN = """on run argv
    tell application "Finder"
        set w to make new Finder window
        set bounds of w to {80, 80, 1080, 780}
        set v to item 2 of argv
        if v is "list" then
            set current view of w to list view
        else if v is "column" then
            set current view of w to column view
        else
            set current view of w to icon view
        end if
        set target of w to (POSIX file (item 1 of argv) as alias)
        return id of w
    end tell
end run"""


# The windows this run opened. A window showing a volume that unmounts stays open, showing the
# Computer, so windows are closed by their ids rather than by what they show.
opened = set()


def close_windows():
    for w in sorted(opened):
        try:
            osa(f'tell application "Finder" to close window id {w}')
        except subprocess.CalledProcessError:
            pass
    opened.clear()


def finder(folder, view):
    path = os.path.join(args.mnt, args.tag, folder)

    def open_window():
        opened.add(osa(OPEN, path, view))

    close_windows()
    measure(f"finder-{view}-cold-mount", folder, lambda: sh(args.cold), view)
    measure(f"finder-{view}-cold", folder, open_window, view)
    close_windows()
    measure(f"finder-{view}-reopen", folder, open_window, view)
    close_windows()
    measure(f"finder-{view}-remount-mount", folder, lambda: sh(args.remount), view)
    measure(f"finder-{view}-remount", folder, open_window, view)
    close_windows()


def command(folder, name, argv):
    close_windows()
    measure(f"{name}-cold-mount", folder, lambda: sh(args.cold))

    def run():
        start = time.time()
        subprocess.run(argv, check=True, stdout=subprocess.DEVNULL)
        return time.time() - start

    measure(f"{name}-cold", folder, run)


def main():
    if args.delay_file:
        with open(args.delay_file, "w") as f:
            f.write(f"{args.delay_ms:g}\n")
    tools = os.path.join(os.environ.get("TMPDIR", "/tmp"), "voidfs-bench-tools")
    os.makedirs(tools, exist_ok=True)
    bulkstat = os.path.join(tools, "bulkstat")
    subprocess.run(["cc", "-O2", "-o", bulkstat, os.path.join(here, "bulkstat.c")], check=True)
    try:
        for folder in args.folders.split(","):
            for s in args.scenarios.split(","):
                if s.startswith("finder-"):
                    finder(folder, s[len("finder-"):])
                elif s == "ls":
                    command(folder, "ls-l@", ["ls", "-l@", os.path.join(args.mnt, args.tag, folder)])
                elif s == "bulkstat":
                    command(folder, "bulkstat", [bulkstat, os.path.join(args.mnt, args.tag, folder)])
                else:
                    sys.exit(f"unknown scenario {s}")
    finally:
        close_windows()
        if args.delay_file:
            with open(args.delay_file, "w") as f:
                f.write("0\n")


main()
