# Read-only FSKit on the daemon: runs, 10–11 October 2026

Raw output behind [step 5's item 3 section](../../../docs/step-5-macos.md#read-only-fskit-on-the-daemon-10-october):
macOS 27.0.1 (26A434), Xcode 27 beta (27A5194q), a development-signed Release build of the app
and its extension, a release `void` daemon in a temporary state directory (`/tmp/vfsd.*`, reached
by the agent through its `VoidfsStateDirectory` developer default), and a release `voidfs-server`
on a memory store at 127.0.0.1:9000 with the spike's throwaway key. The drive `spike` was filled
by `cargo run --release -p voidfs-sdk --example mount_seed` (the spike's tree; `seed.py` needs
boto3). The other machine is the SDK's `mount_peer` example against the same server.

| File | What |
|---|---|
| `manifest-spike.txt`, `manifest-daemon.txt` | `apps/macos/scripts/manifest.sh` on the spike's volume (main at ca989ba, built here) and on the daemon-backed one: every path, type, size, mode, mtime and SHA-256 of the seeded tree. They are identical (11,011 lines); `big.sha256` is the local copy's hash |
| `xattr-spike.txt`, `xattr-daemon.txt` | `xattr -l tagged.txt` on each |
| `bench-spike.txt` | `apps/macos/scripts/bench.sh voidfs://127.0.0.1:9000/spike`, three runs, on the spike's volume |
| `bench-daemon-cold.txt` | The same on `voidfs://spike`, three runs, with `VOIDFS_BENCH_COLD` emptying the daemon's state (cache and namespace) and restarting it before every cold step |
| `bench-daemon-warm.txt` | The same, three runs, with the daemon's cache holding the drive: only the kernel and the extension start cold |
| `coherence.txt` | `apps/macos/scripts/coherence.py`, three runs, 0 failures; `limit` lines are FSKit's, reported rather than failed |
| `offline.txt` | `apps/macos/scripts/offline.py` with the daemon behind a TCP relay that is killed (refused) or stopped (hung), and the extension's log lines for going offline and becoming current again |
| `restarts.txt` | `apps/macos/scripts/restarts.py`: the daemon killed and restarted, the agent restarted and killed, three rounds, with a file held open; then the daemon cases on a `void mount`ed drive |
| `activate-selector.txt` | The extension built without its `activateVolumeWithOptions:` answer, crashing on the first mount |
| `open-sibling-experiments.md`, `open-sibling-holds.txt`, `negprobe.py`, `holdprobe.py` | The release blocker: what keeps a missed name cached, and everything tried against it |
| `breaks-rust.log`, `breaks-swift.log` | The failure proofs: each break, whether its test ran and failed, and the failing check |

Reproduce: start a server and seed it as above; `void daemon start` with `VOIDFS_ENDPOINT`,
`VOIDFS_ACCESS_KEY_ID` and `VOIDFS_SECRET_ACCESS_KEY` (and `VOIDFS_STATE_DIR` for a test daemon,
which the agent then needs as `defaults write dev.voidfs.app VoidfsStateDirectory <dir>` and
`launchctl kickstart -k gui/$(id -u)/dev.voidfs.agent`); build and register the app as
`apps/macos/README.md` says; then `mount -F -t voidfs voidfs://spike /tmp/voidfs-mnt` or
`void mount spike /tmp/voidfs-mnt`, and run the scripts.
