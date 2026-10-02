# The Mac apps head to head: SpaceFS's drive against voidfs's mount

*1 October 2026, from about 15:25 to 20:45 EDT, one Mac (Apple M5 Pro, macOS 27.0.1, 26A434), on
home internet. Step 1 of the [parity plan](../../../docs/PARITY.md#7-step-by-step-plan): "Benchmark
the two Mac apps head to head, with the spike's mount measurements (listing, sequential and random
reads, `semantics.c`, Finder behaviour) on the same test data". Run through computer use, as the
user asked: Space's app, Finder and TextEdit were driven on screen, and the scripted timings ran
from Bash against the same mounts, since terminals take no typing from computer use.*

| | SpaceFS | voidfs |
|---|---|---|
| App | Space 0.2.333 (build 333), its trial drive "Space" ("Space Individual – Trial — 3 days left") | `voidfs.app`, the read-only FSKit spike (`apps/macos`, built 27 September from the code committed as `76c8b9a`), and `voidfs-server` at `415b9fe` |
| Mount | `spacefs mount --retained-v1 --adapter smb`: a loopback SMB server, at `/tmp/Space` | FSKit, `mount -F -t voidfs voidfs://127.0.0.1:9000/h2h ~/voidfs/h2h`, through the app's `mount` command (the code its Mount button runs) |
| Where the data is | **Unconfirmed.** The mount process holds HTTPS connections to `172.64.66.1`, the address `*.r2.cloudflarestorage.com` resolves to (R2's default jurisdiction, not the EU one), and to `69.46.46.34` and `.52`, which whois gives to Railway (`app.spacefs.com` is `c7g0xev3.up.railway.app`). Inferred: file data in Cloudflare R2, read directly by the mount, and the drive's service on Railway. The bucket's location hint and Railway's region are not visible | The user's Cloudflare R2 bucket, location hint Eastern North America (ENAM), pool `voidfs-bench/pool-mac-h2h/` with the `inline-data` feature. `voidfs-server` ran on this Mac with its 512 MiB shard cache, between the mount and the bucket |
| Local state | A SQLite journal per drive (inodes, xattrs, written and fetched blocks), a 20 GB disk cache and a 192 MB memory cache | The server's shard cache, in memory; the spike's module caches listings for 5 s and keeps no data |

The two setups differ where the drive's metadata lives: Space keeps it on this Mac, in its
journal, and voidfs in `voidfs-server`'s memory, which here also runs on this Mac but loads it from
the bucket when it starts. The links to R2 were the same for both, and runs alternated between the
apps (below).

## The test data

`seed.py`'s tree, written once on this Mac by [`make_tree.py`](make_tree.py) so that both drives
got the same bytes, under a folder `h2h/` in each drive: `hello.txt`, `many/` (1,000 files of
16–19 bytes), `many10k/` (10,000), `media/big.bin` (1 GiB of random bytes, SHA-256
`48f9699c3fa486c48a53380631bcfac713d60dec42a89a7d743fa197ba7dc4c8`), `docs/nested/deep/readme.md`,
`tagged.txt` with the xattr `user.voidfs.test` and a Finder tag, and `ünïcödé – 名前.txt`, named
in NFC. 11,005 files. One change from `seed.py`: the tag is a valid binary plist (`["h2h"]`), so
that Get Info has something to show.

How each drive got it (the user approved the upload to the trial drive):
- **Space:** a Finder drag was meant to copy the folder into `/tmp/Space`; it was dropped on
  Space's Settings window instead, which takes files as an upload, and Space uploaded the folder
  itself (its mount process read the source files). The upload view showed "In Progress · 11005
  files · 0% · 1 KB/s", the log counted objects "still to publish" (up to about 7,200, then down at
  about 12 a second), and at 19:50:23Z it logged "everything written here is in the cloud",
  1,073,985,657 bytes, **18 minutes 18 seconds** after the drop. voidfs's seed ran from 15:38 to
  15:43 at the same time, so this is not a clean upload time. **Space's upload kept no xattrs**:
  `tagged.txt` arrived bare. The tag was then added in Finder's Get Info and `user.voidfs.test` with
  `xattr -w`, through the mount.
- **voidfs:** through its S3 API, with rclone (16 at once): the 11,004 small files in **267 s**,
  `big.bin` in **21.4 s** (64 MiB parts, 8 at once; about 50 MB/s up), and the xattrs with one
  `POST ?x-voidfs-attrs`. The spike is read-only, so nothing went through its mount.

The two uploads are not like for like (an app's upload against an S3 client), and are here for
context only.

## Method

- **Order.** On screen: voidfs, Space, Space, voidfs. Scripted: Space, voidfs, voidfs, Space. Every
  run started cold.
- **Cold, for Space:** Settings → Cache → Clear Cache (confirmed in its dialog), then Drives →
  Eject and Mount. The cache readout and its folder (`retained-smb/cloud-cache`) were checked empty
  before each data measurement. Listing steps only ejected and remounted: Space's metadata is in
  its journal on disk, which Clear Cache does not touch ("Your files stay in Space").
- **Cold, for voidfs:** `voidfs-server` restarted (an empty shard cache, and the drive loaded again
  from R2: 159–216 GETs) and the drive remounted. Listing steps only remounted, as the spike's
  `bench.sh` did, so that both apps' listings start from metadata already on the Mac.
- **On screen:** Finder in a fixed window, its status bar on. Each step's timing comes from screen
  captures taken from Bash every 0.12–0.16 s ([`frames.sh`](frames.sh)), compared pixel by pixel
  in the window's content ([`framediff.swift`](framediff.swift)): from the first frame that the
  click changed to the frame where the result appeared. The copy's timing is the destination file's
  size, polled every 0.05 s ([`poll.sh`](poll.sh)), from its creation to 1 GiB. The mount's is
  Space's log (`mount_backend_started` to `mount_backend_ready`) and, for voidfs, the `mount`
  command's own time.
- **Scripted:** [`h2h-step.sh`](h2h-step.sh), the spike's `bench.sh` steps made to work on either
  mount, with `randread`, `bulkstat` and `semantics` from `apps/macos/scripts` (`cc -O2`). Output:
  [`scripted.txt`](scripted.txt). voidfs's bucket requests per step come from the server's
  `/metrics`.

## Results

### On screen (observed)

| | Space, run 1 | Space, run 2 | voidfs, run 1 | voidfs, run 2 |
|---|--:|--:|--:|--:|
| Mount, from the app | 7.1 s | 5.3 s | 0.31 s | 0.073 s |
| Finder, `many/` (1,000 files), icon view | 0.47 s | 0.26 s | ≤ 0.15 s (one frame) | ≤ 0.15 s |
| Finder, `many10k/` (10,000 files), icon view | 5.39 s | 5.35 s | 2.84 s | 1.87 s |
| Switch `many10k/` to list view, then to its end | one frame each | one frame each | one frame each | one frame each |
| Copy `big.bin` (1 GiB) out with Finder | 29.7 s (36 MB/s) | 27.7 s (39 MB/s) | 67.2 s (16 MB/s) | 56.9 s (19 MB/s) |

Every copy's SHA-256 matched. Space's mount time is from its log; the click came up to 0.7 s before
`mount_backend_started`. Its Eject took under 0.7 s. In both apps, Finder opened `many10k/` with a
"Loading…" view first; the times above run from that view to the icons. Space's 7.1 s mount was its
first after the upload.

What each showed, on screen (observed):

| | Space | voidfs |
|---|---|---|
| While working | Uploads: a progress row per upload ("h2h · 0 of 11005 files · 100%", then "Recent: h2h · 11005 files · 1.07 GB"), Pause and Cancel; Finder's copy window ("50.3 MB of 1.07 GB — Less than a minute") | Nothing of its own; Finder's copy window. The spike's menu-bar window holds a URL, key fields and Mount, and computer use could not reach it (an agent app outside `/Applications`), so its mount was run from the command line |
| Volume in Finder | Sidebar "Space", with Eject; "9.22 EB available" | A volume named `h2h` on the desktop, mounted at `~/voidfs/h2h`; no free space shown |
| Quick Look, `hello.txt` and `big.bin` | Text shown; for `big.bin` a generic BIN icon, 1.07 GB, "Uncompress" | The same |
| Open `hello.txt` | TextEdit, editable. Opening it wrote `com.apple.lastuseddate#PS` onto the file in the drive | TextEdit, "hello.txt — Locked" (read-only volume) |
| Get Info, `tagged.txt` | No tag at first (the upload dropped it). "11 bytes (11 bytes on disk)", "Server: smb://space.localhost/Space/h2h/tagged.txt", Modified 3:25 PM (the source's), Created 3:32 PM (the upload). Adding the tag in Get Info worked, and **moved Created to 8:10 PM** | Tag `h2h` shown, greyed out; "11 bytes (4 KB on disk)"; Created and Modified 3:42 PM, the upload's time |
| Get Info, `ünïcödé – 名前.txt` | Name, size and preview right; `readdir` gives the name in **NFD** (30 bytes) | Right; `readdir` gives **NFC** (26 bytes), as stored |
| Dates in list view | The source files' modification time | The upload's time: the spike shows the object's time, not the `mtime` rclone sent as metadata (inferred from the descriptor, which holds `meta.mtime`) |

### Scripted (observed)

Milliseconds, first cold, then warm. Runs 1 and 2 of each app, and the spike's loopback figures
([fskit.md §3](../../../docs/spikes/fskit.md#3-measurements)) for scale.

| | Space | voidfs | voidfs, bucket GETs | Spike, loopback |
|---|---|---|--:|---|
| `cat` of `docs/nested/deep/readme.md`, cold | 193, 47 | 167, 139 | 1 | 7.4 |
| `ls -f many` (1,000), cold / warm | 8.8, 19.7 / 9.4, 10.2 | 31.7, 26.6 / 12.2, 11.7 | 0 | 21–40 / 6 |
| `getattrlistbulk many` (Finder's call), cold / warm | 17.7, 18.0 / 9.8, 11.2 | 18.9, 18.5 / 10.6, 10.3 | 0 | 21–33 / 6–8 |
| `ls -l many`, cold / warm | 26.9, 19.4 / 18.1, 17.6 | 183, 180 / 72.7, 69.2 | 0 | 189 / 69 |
| `ls -f many10k` (10,000), cold / warm | 70.4, 79.0 / 14.0, 14.3 | 105, 141 / 27.4, 27.9 | 0 | |
| `getattrlistbulk many10k`, cold / warm | 68.4, 70.7 / 12.7, 13.3 | 114, 137 / 34.4, 26.2 | 0 | 103–184 / 19 |
| `ls -l many10k`, cold / warm | 142, 137 / 55.5, 53.9 | 1,563, 1,605 / 618, 615 | 0 | 1,700 / 650 |
| `dd bs=1m` of `big.bin`, cold (MB/s, as `dd` gives it) | 37.7, 41.7 | 36.2, 32.0 | 452 | 2,290–2,330 |
| `dd bs=8m`, cold | 40.4, 39.0 | 35.1, 35.1 | 452 | 2,320–2,520 |
| `dd bs=1m`, warm (page cache, GB/s) | 23.4, 23.9 | 22.2, 23.0 | 0 | 21.6 |
| 300 random 4 KiB reads, `F_NOCACHE`: p50 / p90 / p99 | 1.22 / 270 / 366; 1.18 / 299 / 582 | 140 / 246 / 479; 164 / 327 / 702 | 205, 207 | 1.5–1.9 / 3.1–4.0 / 4.5–5.6 |
| The same through the page cache | 1.45 / 312 / 434; run 2 below | 146 / 238 / 543; 166 / 323 / 599 | 200, 210 | 1.3–1.5 / 2.9–3.1 / 4.4–4.8 |

### `semantics.c` (observed)

[`semantics.txt`](semantics.txt), with APFS for reference. voidfs's spike is read-only: it
refuses the scratch folder (`EROFS`) and only its advertised capabilities count.

| | APFS | Space (SMB) | voidfs (spike) |
|---|---|---|---|
| Advertised | case-insensitive; hard links, `RENAME_SWAP`, `RENAME_EXCL`, clone, `flock`, `fcntl` locks, allocate | case-sensitive; no persistent ids, no hard links; xattrs and named streams; `flock`; no `fcntl` locks; `RENAME_SWAP`, `RENAME_EXCL`, clone unknown (`?`) | case-sensitive; xattrs, no named streams; `flock` and `fcntl` locks; no swap, exclusive rename or clone |
| xattrs, AppleDouble | ok, no `._` | `user.test` and FinderInfo ok; ResourceFork `EINVAL`; no `._` | read-only |
| Atomic save (`rename` over a file) | ok | ok | read-only |
| `RENAME_SWAP` / `RENAME_EXCL` / `exchangedata` | swaps / `EEXIST` / `ENOTSUP` | `ENOTSUP` / `EEXIST` / `ENOTSUP`, files untouched | read-only |
| `clonefile`, hard links | ok | `ENOTSUP` | read-only |
| `F_FULLFSYNC`, `F_BARRIERFSYNC` | ok | **`ENOTSUP`** | read-only |
| `flock` / `fcntl F_SETLK` | ok / `EAGAIN` (the probe's own `flock` holds it) | ok / `ENOTSUP` | read-only |
| Shared writable `mmap` + `msync`, unlink while open, symlink, `chmod`, `O_EXLOCK` | ok | ok | read-only |

**The probe had a bug, fixed here.** `semantics.c`'s `get()` returned one static buffer, so a line
printing two files (`a=%s b=%s`) showed whichever was read last, twice: a correct swap on APFS
printed "a=A b=A", and Space's refused swap "a=B b=B". `get()` now rotates buffers, as `p()`
already did, and the table above is from the fixed probe. The spike's finding that `RENAME_SWAP`
on Apple's FSKit FAT module "returns success and overwrites" came from the same printout; the spike
says it was also confirmed after a remount, and no FAT volume was tested here, so it stands
unconfirmed ([fskit.md §4.5](../../../docs/spikes/fskit.md#45-what-must-a-writable-mount-additionally-handle)).

## What the runs show

Observed, unless marked:
- **Mounting:** voidfs's mount took 0.07–0.31 s and Space's 5–7 s: Space starts an SMB server and
  mounts it.
- **Listing from the shell is close, but for `ls -l`:** `ls -f` and `getattrlistbulk` of 1,000 and
  10,000 files take 0.9–3.6 times as long on voidfs as on Space, at most 66 ms more, and none of them
  reached the bucket.
  `ls -l` of 10,000 files takes 1.6 s on voidfs against 0.14 s, with no request either: the spike's
  four FSKit upcalls per file ([fskit.md §3.1](../../../docs/spikes/fskit.md#31-listing)) cost more
  than Space's SMB, which returns attributes with the listing (inferred).
- **Finder showed 10,000 files sooner on voidfs:** 1.9–2.8 s against 5.4 s. Why Space takes longer
  was not measured; Finder over SMB asks for more per file (inferred).
- **Sequential reads are level when the kernel reads ahead:** cold `dd` ran at 32–36 MB/s on voidfs
  and 38–42 on Space, both from R2 through the same link.
- **Finder's copy is twice as fast on Space:** 28–30 s against 57–67 s. On voidfs the copy made
  452 GETs of about 2.4 MB each, and their summed time is the copy's time: one fetch at a time.
  `dd` made the same 452 GETs in 30 s, with the kernel's reads overlapping. Finder's copy does not
  get that overlap on voidfs, and nothing between the module and the bucket reads ahead for it.
  Space's copy ran as fast as its `dd`, so it reads ahead itself (inferred).
- **Random 4 KiB reads favour Space's larger blocks:** p50 1.2 ms against 140–166 ms, though a
  miss costs Space more (p90 270–312 ms against 238–327). With 8 MiB blocks (inferred from its CLI's
  `--block-size` default), 300 random reads would touch about 116 different blocks,
  so most reads found their block already fetched; voidfs's shards are about 2.4 MB (452 per GiB),
  and 205–210 of 300 reads went to the bucket.
- **Space's local journal shows in its semantics:** writable, xattrs and Finder tags (but not from
  its uploader), `flock`, atomic save; no `F_FULLFSYNC`, no `fcntl` locks, no hard links, no clone,
  no swap. An xattr write moves a file's creation date. Names come back NFD over SMB.

For voidfs's Mac client (step 5), from these runs:
1. Read ahead for sequential reads the kernel does not overlap (Finder's copy), in the module or the
   client core.
2. A block cache on the Mac, and a fetch size for random reads: Space's 20 GB disk cache and 8 MiB
   blocks are what make its random reads fast.
3. The spike's per-file upcalls for `ls -l` (already in its plan: a metadata memo in the extension).
4. Show the file times the client sent (`mtime` metadata), not the upload's.
5. Keep xattrs on upload, which Space's uploader does not.

## Not measured

- Writes through the mount, which the spike can't do; and nothing offline.
- Space's FSKit mount method (Settings → Advanced still offers it); the app used SMB.
- Space's drive location, confirmed (above).
- Finder Sync badges and Space's menu-bar window.
- Space's second run of random reads through the page cache: the screen locked before its last
  reset, and the row says so.

## Files

| File | What |
|---|---|
| [`onscreen-log.txt`](onscreen-log.txt) | Every on-screen step, with its timestamps and frame analysis |
| [`scripted.txt`](scripted.txt) | Every scripted step's output, in run order, with voidfs's bucket GETs |
| [`semantics.txt`](semantics.txt) | `semantics.c` (fixed) on APFS, Space and voidfs |
| `copy-{v1,s1,s2,v2}.txt` | Each Finder copy's destination size over time (voidfs run 1, Space runs 1 and 2, voidfs run 2) |
| [`make_tree.py`](make_tree.py), [`h2h-step.sh`](h2h-step.sh), [`frames.sh`](frames.sh), [`framediff.swift`](framediff.swift), [`poll.sh`](poll.sh) | The tools |

The screen captures were deleted after their timings were read. Nothing in these files names the
R2 bucket or its keys. Afterwards the 11,318 objects under `voidfs-bench/` were purged (none left).
