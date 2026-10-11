# Open siblings and cached missing names: experiments, 10 October 2026

macOS 27.0.1 (26A434), Xcode 27 beta (27A5194q), development-signed build of this branch,
release `void` daemon and `voidfs-server` (memory store) on 127.0.0.1:9000, drive `spike`.
Another machine is `mount_peer` (the SDK) putting `probe/<name>`; each case first `stat`s the
name on the mount (a miss the kernel caches), then the peer creates it, then the mount is
polled every 20 ms.

## The spike's volume (main at ca989ba), with its own feed and revokes

`negprobe.py`: no sibling held, visible in 4 ms; `probe/a.txt` held open, not visible in 15 s,
visible 3 ms after it closed (the spike's finding, reproduced).

With `probe/a.txt` held open, the spike's `.voidfs-cache:` hook called `setCacheState` on
`probe/` with every action (push, pushInvalidate, invalidate, update, revoke; modes none and
read; coherency noCache and readCache), on the root, and on the held file then the folder. Every
call returned success; the name stayed missing after each, and appeared once the file closed.

## The daemon-backed volume (this branch)

| Tried | Result while the sibling is held |
|---|---|
| Revoke the folder's children, then the folder (the spike's order) | missing |
| A new FSItem for the folder after the revoke (so a new lookup would get a new vnode) | missing: the kernel never looks the folder up again; the root's name cache still maps it to the old vnode |
| A newer modification and change time on the folder after the change (`stat` of the folder shows it) | missing: the kernel asks for the folder's attributes again after the revoke and sees the new time, but keeps the miss |
| `getattrlistbulk` of the folder (Finder's call), which lists the new name | missing by `stat` |
| Granting `noCache` instead of `readCache` on open | missing |
| Waiting | missing after 90 s |

`holdprobe.py`, after revoking the folder's whole known subtree deepest first (a cached vnode
pins its parent, so a visited subfolder blocks the purge too), twice: see
`open-sibling-holds.txt`. Nothing held, or a file open in another folder: visible in 33–37 ms.
A working directory in the folder, the folder opened (also `O_EVTONLY`, as change watchers
open it), or a file open in the folder or any subfolder: missing while held; 9–18 ms after the
holder lets go.

Reading of it: FSKit's revoke of a folder takes effect only when the kernel reclaims the folder's
vnode, which it can't while anything below is in use, and nothing else a module can do makes the
kernel drop a cached miss.
