# Spike: SMB on macOS for a voidfs drive

*2026-10-11 and 12. MacBook Pro, Apple M5 Pro; macOS 27.0.1 (26A434), smbfs 7.0, no `nsmb.conf`.
Server: this spike's own SMB 2.1/3.1.1 server (Python) on 127.0.0.1, over a local folder or the
daemon's mount core; release `voidfs-server` (memory store) and `void daemon` built from main at
8f1589d.*

Step 5's item 3 ended with a release blocker: FSKit keeps a name this Mac looked up and missed
while anything below its folder is in use, and in the drive's top folder until remount
([step 5](../step-5-macos.md#the-blocker-a-name-missed-while-its-folder-is-in-use)). Item 6's SMB
gate asks whether the macOS SMB client shows the coherence FSKit couldn't, and whether a server
can be embedded beside the Rust daemon. Raw output is in
[bench/results/mac-smb-spike](../../bench/results/mac-smb-spike/README.md).

## 1. Summary

- **smbfs has no pinned miss.** What is held open never matters to it. A name missed and then
  created elsewhere shows in **2–3 ms** with a server that breaks SMB 3.x directory leases on each
  change: in every hold case of the blocker, in a day-old folder, and in the top folder. Through
  the daemon it shows in **25–45 ms** from the other machine's commit, the change feed included.
  Item 3's `holdprobe.py` and `negprobe.py` pass in every case.
- **`coherence.py` passes through the daemon, 0 failures in three runs**, with two `limit` lines
  where FSKit has six: an open file's path shows the version open until it closes. The names
  FSKit pins, missed with a sibling held and in the top folder, are found in 21–25 ms, and the old
  names FSKit keeps for open files are gone within the script's 2-second check (a renamed open
  file's in 1 ms, `renameprobe.py`).
- **The server has to earn it.** Each of these was measured to matter (§4):
  1. break directory leases (SMB 3.x) on every change; change notifications cover watched folders;
  2. report folder times that move when something in the folder changes. Without a push, smbfs
     drops a cached miss only when it sees the folder's time advance, after a tenth of the
     folder's age (2–30 s). The core gives a folder its first object's time (the top folder 1970),
     so a miss without a push is **never** dropped;
  3. give a file a new ID when another machine renames it: smbfs finds a known file ID's node and
     opens it by its old name, which fails for as long as the node lives;
  4. answer a session that still holds an older version open with that version, by path too.
     Otherwise `fstat` shows the new size while reads return the old bytes.
- **What remains with the best server:** a name missed in a folder this Mac **never listed** and
  isn't watching shows only when smbfs's attribute timer expires: **2 s** in a recently changed
  folder, **30 s** in one older than five minutes. smbfs asks for a directory lease only when it
  lists a folder. FSKit never shows that name while anything below the folder is in use.
- **No existing server fits.** Samba is GPLv3 with a local-file-system model; go-smb2 is AGPL (or
  commercial) with leasing switched off; the MIT `smb-server` crate has one release (April 2026)
  without leases or notifications; SMBLibrary is .NET and LGPL without leases; Apple's server is File
  Sharing. A voidfs server in Rust inside the daemon is the option: the spike's read-only server is
  1,550 lines of Python.
- **Mac plumbing works:** NTLMv2 per user, bound to loopback (the LAN address is refused);
  `NetFSMountURLSync` mounts into **`/Volumes` as the user, with no privileged helper**; smbfs sends
  Apple's AAPL create context on its first open.
- **Open problem, read path:** smbfs kept up to 39 READs in flight, over the daemon's 32 calls.
  In two runs some READs got no reply; in one of them the application read wrong bytes **without
  an error**. Bounding the server's daemon reads fixed it in three later runs; the cause was not
  found (§4.6).
- **Recommendation for the open decision (§8): SMB by default, with voidfs's own server; FSKit
  kept behind the adapter boundary, read-only, until Apple fixes the miss.** The decision is the
  user's.

## 2. What was built

[`smbspike.py`](../../bench/results/mac-smb-spike/smbspike.py) is a small SMB2 server on the Python
standard library, with AES-CMAC from CommonCrypto through `ctypes`. Nothing was installed, File
Sharing stayed off, and the server binds 127.0.0.1 only.

- **Connection:** SMB1 → SMB2 upgrade negotiate; SMB 2.1 or 3.1.1 (preauth integrity SHA-512,
  AES-CMAC signing); NTLMv2 inside SPNEGO with the mechListMIC, the session key exchange and a
  signed final SESSION_SETUP; one user and password per server, from a 0600 file.
- **Read-only file system:** CREATE (with MxAc, QFid and lease contexts), READ, CLOSE,
  QUERY_INFO (the file and file-system classes smbfs asks for), QUERY_DIRECTORY with patterns,
  IOCTL validate-negotiate, CHANGE_NOTIFY with CANCEL, and lease breaks (v1, v2 with epochs and
  parent keys). Writes answer read-only.
- **Backends:** a local folder, changed by `localpeer.py`, which tells the server through a control
  socket; or the daemon's `/v1/fs` session over its Unix socket. The daemon's watch stream becomes
  breaks and notifications, as a voidfs server would turn the change feed into them.
- **Switches** for each behaviour measured: dialect, leases (none, files, all), breaks, notify,
  frozen folder times (as the core reports folders), path-derived file IDs, the open version by
  path, advancing folder times, breaking an ancestor's lease.

The probes are item 3's scripts unchanged, item 3's `coherence.py` with one fix (it now waits for a
renamed file's new name to open; it crashed with ENOENT on SMB), and three new ones:
`smbprobe.py` (the blocker's holds × folder age × a watcher, or a never-listed folder),
`renameprobe.py` and `netfsmount.py`.

## 3. How smbfs caches

From the measurements, and from Apple's published smbfs source, which stops at smb-759.40.1
(2015) and so predates directory leases:

- **Attributes**, a folder's included, are kept for a tenth of the item's age, clamped to
  **2–30 s**, as NFS does. A day-old folder is checked every 30 s, a new one every 2 s.
- **Missed names** are cached in the kernel's name cache. smbfs drops a folder's misses
  (`cache_purge_negatives`) when a refresh shows the folder's modification time moved, when the
  server breaks the folder's directory lease, or when a change notification completes on it. With
  a folder time that never moves, only the last two work.
- smbfs **never lets a folder's time go backwards**, so a test must age a folder on the server
  before the Mac first sees it.
- **Lookups** are a CREATE of the folder and a QUERY_DIRECTORY with the name as the pattern. These
  ask for no lease. smbfs asks for a folder's **RH directory lease when it lists the folder**
  (SMB 3.x), and an RH file lease with a durable handle on every file open.
- **Listings** are cached separately. Mounting with `-o nomdatacache` turns that cache off, not
  the attribute timer.
- **Change notify** is armed only on folders someone watches (kqueue, FSEvents, a Finder window).
  smbfs throttles it: more than four completions within a second back it off for five seconds.
- **Open files** read through the handle they opened, so a server that keeps the opened version
  gives whole old bytes. Their size, though, comes from a separate open-query-close by path.
- **File IDs** identify nodes. A known ID under a new name gets the old node, opened by its old
  name.

## 4. Measurements

Times are from the other machine's commit until the first poll (every 10–20 ms) that saw the
change. On the local folder 2–3 ms is the poll interval.

### 4.1 The blocker's cases

A name looked up and missed, then created by another machine; found by path, in ms. "Held"
covers nothing, a sibling file open, and the folder as a shell's cwd (all alike); `holdprobe.py`
adds a subfolder file, another folder's file and the folder opened `O_EVTONLY`/`O_RDONLY`, with the
same results.

| Server (local folder) | Young folder, held | Day-old folder, held | Top folder | Watched (kqueue) |
|---|---|---|---|---|
| 3.1.1, leases broken, notify | **2–3** | **2–3** | **2** | 2–3 |
| 3.1.1, file leases only | 2,114–2,950 | 30,379–30,870 | 10,933 (92 s old) | 2–3 |
| 3.1.1, leases never broken | 2,670–2,959 | 30,372–30,852 | 10,960 (92 s old) | 2–3 |
| 3.1.1, nothing pushed | 2,083–2,935 | 30,647–30,881 | 10,954 | 2,679–2,946 (no notify) |
| 2.1 (no directory leases), notify | 2,664–2,949 | 30,409–30,860 | 10,965 | 2–3 |
| 3.1.1, leases broken, folders at 1970 | 2–3 | 2–3 | 2 | 2–3 |
| 3.1.1, nothing pushed, folders at 1970 | **never** (70 s + 70 s after release) | — | **never** | 2 (notify alone) |
| Through the daemon, final server | 37–45 | — | 33 | 40–45 |

What is held never changes a row. Without a directory-lease break the timer decides; with folders
at 1970 and nothing pushed, the miss stays, which is FSKit's failure without the hold.

### 4.2 Folder times and folders never listed

| A folder this Mac only looked up | Unwatched | Watched |
|---|---|---|
| Local folder, young, leases broken | 2,542 | 2 |
| Local folder, day old, leases broken | 30,801 | 3 |
| Local folder, folders at 1970 | **never** | — |
| Through the daemon (folder times don't move) | **never** | 41 |
| Through the daemon, folder times advanced by the server | 2,034–2,792 | 29 |
| Through the daemon, the nearest leased folder above broken instead | never | — |
| Local, day old, the nearest leased folder above broken | 30,170 | — |

The core keeps a folder's time at its first object's (`probe/sub` kept its time after a new child
arrived; the top folder is 1970). Advancing it in the server brings back smbfs's timer; breaking
an ancestor's lease does not make smbfs look the folder up again.

### 4.3 Listings

A new name in the folder's listing: 2–3 ms with leases broken, 32–40 ms through the daemon. With
nothing pushed, `smbprobe.py`'s polled listing refreshed after 2.2–2.9 s even in a day-old folder
(10.5–18.3 s at the top) while path lookups there waited 30 s, so `ls` could show a file `stat`
didn't find. In `coherence.py` with nothing pushed (and on 2.1), renames, deletes and new names
didn't reach the listing within 5 s at all. `-o nomdatacache` made listings current within 2 ms
even with nothing pushed.

### 4.4 Open files, renames and deletes (`coherence.py`)

| Server | Failed | What failed | `limit` lines |
|---|---|---|---|
| Local, 3.1.1, leases broken | 3 | an open file's `fstat` shows the new size (2); a renamed file this Mac had read never opens by its new name | — |
| Local, nothing pushed | 6 | listings (renames, deletes, the top folder) | open version by path (2), old name of an open renamed file |
| Local, 2.1 | 8 | listings, `fstat` | — |
| Local, `nomdatacache` (pushed or not) | 3 | `fstat` (2), the rename | — |
| Local, path IDs | 2 | `fstat` | — |
| Local, path IDs + open version | **0** | — | open version by path (2) |
| Daemon, no switches | 3 | `fstat` (2), the rename (old name stays, new never opens) | — |
| **Daemon, final server, 3 runs** | **0** | — | open version by path (2) |
| *Item 3: FSKit on the daemon, 3 runs* | *0* | — | *open version by path (2), old names of an open renamed or deleted file (2), missed names with a sibling held and at the top (2)* |

`renameprobe.py` isolates the rename: with the inode as file ID, a file this Mac had listed or
still has open is fine, but one it had read and closed never opened by its new name in 30 s. smbfs
looked the new name up, got the entry with the known ID, and sent `CREATE` for the old name
(`server-log-excerpts.txt`). Path-derived IDs fix it (1 ms). An open file whose path refreshes
after a break reads the opened version whole, 12 bytes, while `fstat` says 32: smbfs got the size
from a fresh open by path. Answering that open with the version the session holds turns the
failure into FSKit's accepted limit. The daemon keeps a file's inode across another machine's
rename and overwrite (10051 throughout), so it needs both switches.

### 4.5 Through the daemon

The final server (`--path-ids --hold-open-version --advance-dir-times`) on the daemon's session:
`coherence.py` 3 runs, 0 failures, a median of 25 ms and at most 32 ms for its 39 timed checks.
`holdprobe.py` 33–38 ms in every hold, `negprobe.py` 32–33 ms with and without the sibling,
`renameprobe.py` new names listed in 26–29 ms and opening at once, `smbprobe.py` 33–45 ms except the
never-listed folder (2,136 ms; 29 ms watched). The daemon needed no change: its `/v1/fs` calls and
watch stream were enough.

### 4.6 Reads

smbfs read `media/big.bin` in 256 KiB, 512 KiB and 1 MiB READs (8 MiB negotiated, the daemon's
`maxIo`), with up to **39 in flight** in one run and 24 in another. The daemon allows 32 calls at
once and answers `Busy` (EAGAIN) beyond, so a server must queue its calls to the daemon.

In the first two runs some READs got no reply from the daemon side of the spike (14 of 1,016 and
6 of 432, clustered in a few 8 MiB blocks). In the first, the application's sequential read
completed **without an error, but the bytes from 49 MiB, the first unanswered READ, differed from
the file**; in the second, the read failed with `ETIMEDOUT`. With the spike's daemon reads
bounded to 24, `Busy` retried and a 10 s timeout, three runs (one with a fresh daemon and a cold
cache, one unbounded) matched the file in full with no timeout. Straight against the daemon,
40 concurrent cold reads, reused connections and a dropped first reader never lost a reply. Which
side hung is **not known**, and why smbfs returned bytes for an unanswered READ is not known either.
Throughput was not measured: the Python server is the bottleneck (cold reads ran at 2 MB/s
including the stalls).

### 4.7 Authentication, reach and mounting

- A wrong user and a guest logon are refused (`Authentication error`), a wrong share is
  `ENOENT`. A wrong password is refused, **except while this macOS user already has a session to
  the same host as the same name**: smbfs then reuses that session's credentials, on another port
  and with `-s` too. Apple's published source shares a session only with the same uid; a second
  macOS account was not tried.
- Bound to 127.0.0.1, the server refused a connection to the Mac's LAN address.
- `NetFSMountURLSync` (Finder's Connect to Server) with `AllowLoopback` mounted the share at
  **`/Volumes/spike` as the user**, with no prompt and no privileged helper, in 2.3 s including
  Python's start. FSKit needs the `fskit.mount` entitlement or a helper for `/Volumes`.
- smbfs sends Apple's **AAPL create context** on its first open of a mount. A server that answers
  it declares Unix modes, case sensitivity and full fsync, and gets Finder info in listings; the
  spike ignored it, so smbfs treated it as a plain server (modes 0700, no resource forks), which is
  what SpaceFS's SMB mount shows ([PARITY §5](../PARITY.md#5-what-spacefss-mac-app-is-made-of)).

## 5. Servers to embed or run beside the daemon

| Server | License | What it is | Leases, directory leases | Notify for outside changes | For voidfs |
|---|---|---|---|---|---|
| Samba `smbd` | GPLv3; a VFS module is GPL too | C, the reference Unix server; directory leases since 4.22; `vfs_fruit` for Macs | yes, yes | from inotify (Linux), or its VFS layer | A separate GPLv3 process in the app (source offered) over a local-file-system model: the namespace would need a GPL VFS module or a real folder; heavy to build and ship on macOS |
| go-smb2 (macos-fuse-t) | AGPL-3.0, or commercial | Go, the server inside fuse-t; Mac-oriented (xattrs as streams, modes) | switched off (a TODO) | its own operations only | AGPL in a shipped app, or a licence bought; lacks exactly the coherence hooks |
| `smb-server` crate | MIT | Rust, 13k lines; one release on crates.io, 0.4.0 (30 April 2026); MSRV 1.95 | none | `CHANGE_NOTIFY` → NOT_SUPPORTED | Young; above voidfs's MSRV (1.90); no xattrs; the coherence work would be ours anyway |
| SMBLibrary | LGPL-3.0, or commercial | C#/.NET; SMB up to 3.0 | none | through its file-store interface | Needs a .NET runtime in the app; no 3.1.1 |
| impacket `smbserver` | Apache-style (modified 1.1) | Python security-testing tool; 3.1.1 | none | NOT_SUPPORTED | Not a product server |
| ksmbd | GPLv2 | Linux kernel server | yes | — | Not macOS |
| macOS File Sharing (`smbd`) | Apple, closed | The system's server | yes | for APFS | Serves real folders system-wide, needs File Sharing switched on (reachable from the network); no way in for the daemon's namespace |
| Tuxera Fusion SMB, Visuality YNQ, Ryussi MoSMB | Commercial | C servers for Linux NAS and embedded systems | vendor-stated | not checked | No published macOS terms or prices; not evaluated further |
| **voidfs's own, in the daemon** | Apache-2.0 | Rust, in the daemon process, talking to the mount core directly | ours | the change feed, directly | The spike's read-only server is 1,550 lines of Python; §6 lists what a product needs |

The plan's checklist for the server voidfs would write:

| Need | Status after the spike |
|---|---|
| Bounded reads | Negotiated 8 MiB = the core's `maxIo`; READs bounded by the open version's size; calls to the core must be queued below its concurrency (§4.6) |
| Write and flush | Not built (read-only spike): WRITE, FLUSH (smbfs sends it for `fsync`; AAPL advertises full sync for `F_FULLFSYNC`), SET_INFO end-of-file and times, onto the core's staged writes |
| xattrs and streams | Not built: smbfs maps xattrs to named streams and resource forks to `AFP_Resource`; the core has xattrs; `nostreams` exists as a mount option |
| Unicode and case | smbfs sent precomposed names; the core stores NFC and rejects ambiguous names; AAPL can declare case sensitivity; not tested beyond the seeded name |
| Rename-over, delete-on-close | Not built: SET_INFO rename with replace, disposition and `FILE_DELETE_ON_CLOSE`, onto the core's renames and deletes (smbfs deletes on last close for an open unlinked file) |
| Lease, oplock and notify hooks | Built and measured: directory leases broken from the feed, file leases, notify; plus folder times, rename IDs and the open version (§1) |
| Loopback only, per-user authentication | Bound to 127.0.0.1; NTLMv2 per user; credentials reused within one macOS user (§4.7) |

## 6. What a voidfs SMB server must do

Beyond the protocol, the measurements give the server these duties:

1. **Break every directory lease** on a change below it in the feed (R removed), and the changed
   file's lease; complete pending change notifications (watch-tree ones too). Expect smbfs's
   notify throttle under bursts; leases are not throttled.
2. **Advance folder times** to the last change the server learned of in the folder, so smbfs's
   timer works where no lease reaches (never-listed folders, a lost break). The core's folder
   times stay as they are; this is the adapter's view.
3. **New file IDs on another machine's rename**, stable otherwise (Finder and apps keep IDs across
   their own renames, which the server sees as its own operations).
4. **The open version for its session**, by path and in listings, while that session has it open;
   no lease on such an open, so nothing caches it past the close.
5. **Queue calls to the core** below its concurrency, answer every request (an error rather than
   silence), and verify read bytes in tests: smbfs may hand an application bytes for a READ that
   got no answer (§4.6).
6. **Answer AAPL**, so Unix modes, case sensitivity, full sync and Finder info work.
7. **Per-mount secrets:** a random user name and password per mount, held by the daemon (never in a
   URL, argument or log), loopback only. smbfs keeps credentials in the kernel for reconnects, and
   reuses a same-host, same-name session's credentials within one macOS user.
8. **Signing** (AES-CMAC on 3.1.1) is cheap insurance on loopback; encryption is not needed.

## 7. Risks

1. **smbfs is closed now.** Its directory-lease behaviour isn't in any published source; Apple
   could change it in a release, as FSKit can. Keep the probes as a rig on each macOS release.
2. **Writing an SMB 3.1.1 server is real work**: the protocol surface for Finder, Office and the
   video tools (writes, locks, streams, AAPL, compound requests, credits) and its test matrix.
3. **The never-listed folder** stays a limit: 2–30 s, beyond the 5-second requirement for old
   folders. Apps that `stat` known paths without listing (build tools, editors' lock files) hit it.
4. **The read-path hang** (§4.6) is unexplained, and the wrong-bytes outcome is the worst kind.
5. **A network volume is different in Finder**: Spotlight indexing, Trash and Time Machine treat
   SMB volumes as remote. Not tested.
6. **New dependencies:** NTLM needs MD4 and RC4 and 3.1.1 signing needs AES-CMAC; the workspace has
   `hmac`, `md-5`, `sha2` and `aws-lc-rs`, but not MD4, RC4 or a CMAC crate. Adding any is the
   user's call.

## 8. Recommendation: FSKit, SMB or both

| Option | What it leaves unsolved |
|---|---|
| **FSKit by default** (the 5 October decision) | The release blocker itself: a missed name stays while anything below its folder is in use, and at the top until remount, with no API to clear it ([step 5](../step-5-macos.md#the-blocker-a-name-missed-while-its-folder-is-in-use)); only Apple can fix it. Also the old names of open renamed and deleted files, the `fskit.mount` entitlement or a helper for `/Volumes`, and macOS 27 only |
| **SMB by default, voidfs's own server** (recommended) | A never-listed, unwatched folder's misses take 2–30 s; building and maintaining an SMB server (writes, xattrs, AAPL, locks); the read-path hang to explain; network-volume behaviour in Finder; Apple changing smbfs |
| **Both** | Both lists until one is dropped; twice the durability and coherence validation, an explicit default and adapter-specific remembered mounts (item 6's done-when), and no silent switch for a live writable mount |

The recommendation is **SMB by default with voidfs's own server in the daemon**, keeping FSKit's
read-only adapter behind the adapter boundary (no writable FSKit work) and filing an Apple Feedback
for the pinned miss; if Apple fixes it, FSKit can be reconsidered. SMB fails the 5-second
requirement only for folders never listed, by seconds, where FSKit fails it indefinitely and in the
cases users meet first (a folder in Finder with a file open). The SMB client also reaches `/Volumes`
without a helper. The cost is the server. This decision is the user's; item 4 waits for it.

## 9. Limits of this spike

- One Mac, one macOS build, loopback only; no second Mac, no WAN.
- Read-only: writes, locks, xattrs, streams and AAPL were not built; Finder, Office and the video
  apps were not run on the SMB mount.
- The server was Python: no throughput, latency under load or memory numbers for SMB itself.
- Not tested: a second macOS account (credential sharing across uids), sleep and reconnect,
  `nsmb.conf` settings, Finder's handling of the mount (sidebar, Trash, Spotlight), macOS 26.
- The read-path hang was not reproduced or explained (§4.6).
