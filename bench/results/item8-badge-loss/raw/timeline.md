# The live run's last phases, by the clock

Times are local (EDT), 16 October 2026. "Log" lines are in [logs/live-run-mounts.txt](logs/live-run-mounts.txt).
The other lines come from the live run's own tool log: the command that ran, and the Finder windows it
listed and screenshotted (window ids as computer use reported them).

| Time | What happened | Source |
| --- | --- | --- |
| 16:37:05 → 16:37:18 | The app rebuilt (G to R ran on this build): volume unmounted, then mounted again; new Finder extension processes | log |
| 16:38:36 | Phase H: test window closed, `void daemon stop` | tool log |
| 16:38:37.425 | Volume unmounted (Finder's volume -503 gone) | log |
| 16:38:46 | Phase I: `void daemon start` | tool log, `i-daemon-started.txt` |
| 16:38:47.177 | Volume mounted again, 9.8 s after the unmount: Finder's volume -504, file system id 33059174 | log |
| 16:38:48.899 | `open <mnt>/live`: Finder opens `live` (node `.file/id=33059174.17`), window 9884 | log, tool log |
| 16:38:51 | Window 9884 screenshot: every badge ([i](../../item8-live/raw/live/i-finder-after-daemon-restart.jpg)) | tool log |
| 16:39 to 16:46 | Phases J to N in window 9884, with badges ([j](../../item8-live/raw/live/j-finder-after-agent-restart.jpg), [k](../../item8-live/raw/live/k-offline-finder.jpg)) | tool log |
| 16:46:42 | Phase O: one command builds `void` with the control fix (12.9 s), then closes window 9884, stops and starts the daemon, and opens `<mnt>/live` | tool log, `o-daemon-fix.txt` |
| 16:46:56.477 | Volume unmounted (-504 gone) | log |
| 16:46:56.731 | Volume mounted again, **0.25 s** after the unmount: Finder's volume -505, file system id 33114754 | log |
| 16:46:57.760 | Finder opens `live` (node `.file/id=33114754.17`, 12 children), window 9910 | log, tool log |
| 16:47:02 | Phase P: relay stopped (hung) | `p-hung-fixed-how.txt` |
| 16:47:16 to 16:47:25 | Full-screen screenshots fail ("Screenshot capture returned nil") | tool log |
| 16:47:35 | Phase Q: relay answers 503 | `q-degraded-how.txt` |
| 16:47:55, 16:47:57 | Window 9910 screenshots fail ("audio/video capture failure") | tool log |
| 16:48:05 | Phase R: relay forwards again | `r-recovered.txt` |
| 16:49:24 | Status says online | `r-recovered.txt` |
| 16:49:27 | Window 9910's first screenshot: no badge on any item ([r](../../item8-live/raw/live/r-recovered-finder.jpg)) | tool log |
| 16:51:43 | The screenshot copied into the run's output (the README's "16:51") | tool log |
| 16:51:47 | Window 9910 closed | tool log |
| 16:52:28 | Volume unmounted for the restore | log |

Window 9910 was never captured with badges. Every capture of it came after phase O's remount, and the first
one came after Q and R. Earlier remounts in the run were rebuilds of the app, which also restarted the Finder
extension, or phase H/I's, with the volume gone for 9.8 s:

| Unmounted | Mounted | Gap | Why | Finder extension restarted |
| --- | --- | --- | --- | --- |
| 16:26:03.511 | 16:26:32.157 | 28.6 s | app rebuilt | yes |
| 16:32:23.206 | 16:32:43.334 | 20.1 s | app rebuilt | yes |
| 16:34:08.956 | 16:34:24.757 | 15.8 s | app rebuilt | yes |
| 16:35:22.561 | 16:35:32.801 | 10.2 s | app rebuilt (diagnostic build) | yes |
| 16:36:01.927 | 16:36:02.053 | 0.13 s | `void unmount && void mount` (no window opened on the mount before the next rebuild) | no |
| 16:37:05.775 | 16:37:18.391 | 12.6 s | app rebuilt | yes |
| 16:38:37.425 | 16:38:47.177 | 9.8 s | phases H and I, daemon stopped and started | no |
| 16:46:56.477 | 16:46:56.731 | 0.25 s | phase O, daemon restarted in one command | no |

The Finder extension's four processes, by their first and last log lines (`log show --predicate 'process ==
"VoidfsFinder"'`): a new set started with each rebuild (16:24:51, 16:26:03, 16:32:38, 16:34:19, 16:35:25,
16:37:17), and the last set, pids 24784, 24786, 24787 and 24788, ran from 16:37:17.8 to 16:52:40.6: through
phases G to R, the daemon restarts of H/I and O included.
