# SMB feasibility spike: runs, 11–12 October 2026

Raw output behind [docs/spikes/smb.md](../../../docs/spikes/smb.md) (step 5, item 6). macOS 27.0.1
(26A434), smbfs 7.0, MacBook Pro M5 Pro. The Mac's own SMB client (`mount_smbfs`, and NetFS once)
mounted a spike server on 127.0.0.1:44445 as user `spike` with a random password from a 0600 file;
nothing was installed, File Sharing stayed off, and `nsmb.conf` was left as it was (absent).

## The scripts

| File | What |
|---|---|
| `smbspike.py` | The spike's SMB 2.1/3.1.1 server (Python standard library and CommonCrypto through `ctypes`): NTLMv2 per user with SPNEGO and signing, read-only CREATE/READ/QUERY_INFO/QUERY_DIRECTORY, file and directory leases (v1/v2) and their breaks, CHANGE_NOTIFY, bound to 127.0.0.1 only. It serves a local folder (`--local`, with a control socket for another machine's changes) or the daemon's `/v1/fs` session (`--daemon`, `--drive`), turning the change feed into breaks and notifications. Its switches are the configurations below. The runs here used it as of 37c29be; it has kept a reference to every task it starts since the unanswered READs were traced to their loss ([spike §4.6](../../../docs/spikes/smb.md#46-reads)) |
| `localpeer.py` | Another machine for `--local`: makes a change in the served folder (a put writes a new file and renames it over, as a new version replaces the old) and tells the server, as the feed would |
| `smbprobe.py` | A name looked up and missed, then created by another machine: milliseconds until it is found by path (or listed, `--poll list`), for each of the FSKit blocker's holds (nothing, a sibling file open, the folder as cwd, the top folder), a folder this Mac never listed (`unlisted`), a young or day-old folder, with or without a kqueue watcher |
| `holdprobe.py`, `negprobe.py` | Item 3's probes ([mac-fskit-daemon](../mac-fskit-daemon/)), unchanged |
| `renameprobe.py` | Another machine renames a file this Mac had listed, had read, or still has open: until the new name is listed and opens |
| `netfsmount.py` | Mounts an SMB URL the way Finder's Connect to Server does (`NetFSMountURLSync`, no UI), to see where it lands |
| `daemonreads.py` | smbfs's READs straight at the daemon's session, without SMB: a sliding window of 256 KiB–1 MiB READs over pooled connections, `Busy` sent again, every task kept and every reply checked against the seed's copy; then the daemon's free call slots |
| `lostreads.py` | The unbounded spike's READ (`DaemonBackend._request`, no timeout) in tasks with no reference, as the server started them, under the GC as it runs or `gc.collect()` every 50 ms, against the bound, a timeout and kept tasks; parses what reached connections whose reader was gone |
| `lostreads_conn.py` | A `smbspike.py`'s own `Conn.run` fed SMB2 READ frames, under `gc.collect()`: the server as run (37c29be) against this one, which keeps its tasks |

`apps/macos/scripts/coherence.py` is the item 3 script; this spike changed it to watch for a
renamed file's new name opening, not only being listed (over SMB the first crashed it).

## Configurations (file names)

`311` is SMB 3.1.1 and `21` SMB 2.1. A `d` prefix means the daemon backed the server (a release
`voidfs-server` on a memory store at 127.0.0.1:9000 with the spike's throwaway key, the drive
seeded by `mount_seed`, a release `void daemon` in a `mktemp -d` state directory, `mount_peer` as
the other machine); otherwise a local folder did.

| Name | Server |
|---|---|
| `full` | File leases (RH with durable handles), directory leases (v2, from 3.0), both broken on every change, CHANGE_NOTIFY completed on every change |
| `nobreaks` | Leases granted but never broken; notify on |
| `fileleases` | File leases only; notify on |
| `silent` | No leases, no notify: smbfs's own timers only |
| `frozen` | Every folder reports 1970 (`--frozen-dir-times`), as the core reports folders today |
| `nomdc` | Mounted with `-o nomdatacache` |
| `pathids` | A file's ID made from its inode and path (`--path-ids`) |
| `best` | `--path-ids --hold-open-version`: also, while this Mac has a file open, its path and listing show the version open |
| `final` | `best` plus `--advance-dir-times`: a folder's time is at least the last change the server learned of in it |
| `unlisted-advance`, `-ancestor`, `-both` | The never-listed folder with `--advance-dir-times`, `--break-ancestor`, or both |
| `list` | The probe polls the folder's listing instead of `stat` |

## Files

| File | What |
|---|---|
| `probe-311-full-young.txt`, `probe-311-full-old.txt` | `smbprobe.py`, full: every hold, young and old folders, the top folder, with and without a watcher |
| `probe-311-nobreaks.txt`, `probe-311-fileleases.txt`, `probe-311-silent.txt`, `probe-21-full.txt` | The same without breaks, with file leases only, with nothing pushed, and on SMB 2.1 |
| `probe-311-frozen.txt`, `probe-311-frozen-silent.txt` | Folder times frozen, with and without pushes |
| `probe-311-full-list.txt`, `probe-311-silent-list.txt` | Listing instead of path lookup |
| `probe-311-unlisted.txt`, `probe-311-unlisted-frozen.txt`, `probe-l-unlisted-ancestor.txt` | A folder this Mac never listed |
| `probe-311-silent-nomdc*.txt`, `probe-311-silent-frozen-nomdc.txt` | `nomdatacache` |
| `holdprobe-*.txt`, `negprobe-*.txt`, `renameprobe-*.txt` | Item 3's probes and the rename probe, per configuration |
| `coherence-311-*.txt`, `coherence-21-full.txt` | `coherence.py` on the local folder: `full-crashed` is the first run (the rename crash), `full-2` after the script's fix |
| `coherence-d311-full.txt`, `coherence-d311-best.txt`, `coherence-d311-final-{1,2,3}.txt` | `coherence.py` through the daemon: no server switches, `best`, and three runs of `final` |
| `probe-d311-*.txt`, `probe-d-unlisted-*.txt`, `*-d311-best.txt`, `*-d311-final.txt` | The probes through the daemon |
| `reads.txt` | `media/big.bin` read through the daemon-backed mount: READ sizes and how many were in flight, and the runs where READs got no reply; then the three scripts above, which found that the spike lost those READs |
| `server-log-excerpts.txt` | The server log behind the findings: the AAPL context, a never-listed folder's lookups, the open file's size, the rename, the logons |

Times are from the other machine's commit (`mount_peer` and `localpeer.py` print it) to the first
poll (every 10–20 ms) that saw the change. A local-folder change reaches the server's control socket
before the peer returns, so 2–3 ms there is the poll interval; through the daemon, 25–45 ms
includes the change feed.

Reproduce: start `smbspike.py --port 44445 --user spike --password-file <file> --local <dir>
--control <sock>` (or `--daemon <state dir>/daemon.sock --drive spike`) with the configuration's
switches, `mount_smbfs -o nobrowse //spike:<password>@127.0.0.1:44445/spike <dir>`, run a probe
with the peer (`localpeer.py <dir> <sock>`, or `mount_peer spike` with `VOIDFS_ENDPOINT`,
`VOIDFS_ACCESS_KEY_ID` and `VOIDFS_SECRET_ACCESS_KEY` set), then `umount`. smbfs keeps a folder's
time from going backwards, so `smbprobe.py --age-cmd` ages a folder on the server before the Mac
first sees it.
