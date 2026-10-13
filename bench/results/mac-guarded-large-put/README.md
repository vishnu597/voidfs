# Guarded publication of large new mount files

A real 171,808,569-byte ProRes movie generated on the isolated FSKit mount remained in local staging when upload item 4923 failed with `a new mount file needs an absence guard unsupported by multipart completion`. New mount files require `If-None-Match: *`; multipart completion cannot carry that absence guard. The source bytes remained readable and decoded correctly. `mount-failure-excerpt.json` keeps the original source SHA and failed queue state; the complete row remains in the app matrix evidence.

The fix sends an immutable journal file through a signed ordinary PUT when a new mount file exceeds the multipart threshold. It retains the absence guard, mode, mtime and entry marker. SHA-256 hashing and request streaming use 64KiB blocks, with 128KiB reserved from the queue memory budget. Each retry reopens the source and resets per-item progress. Pieces respect the shared upload rate and update progress while streaming. A lost successful response is recognized through the existing entry marker after the guarded replay receives 412; a competing remote creation preserves the local and remote conflict sides.

The SHA prepass checks pause/cancel between blocks. The streaming silence watchdog excludes deliberate bandwidth pacing, resets when a piece is yielded, and bounds a stalled peer or response headers after the final piece. Reading the small response body also has a timeout. It uses the existing protocol and adds no dependencies.

Validation on macOS 27.0.1 /26A434, October 13, 2026:

- Client library: 102 passed, 1 existing ignored. SDK library: 16 passed. SDK/client all-target clippy with `-D warnings` passed.
- A 5 MiB+37-byte mount creation with a 300 ms server timeout and about 1 second paced upload succeeds, reports `0 < item.sent < size` during upload, and stores exact bytes, metadata and one version.
- Competing remote creation during the actual PUT retains both versions locally and leaves the peer's remote version unchanged. The guard-weakened mutation fails this test.
- A 503 returned after the first committed PUT causes the source to reopen. Guarded replay receives 412, recognizes its own marker, and succeeds with one server version. Per-item final progress is one file; aggregate bandwidth counts both attempts.
- A 32 MiB fixture bounds a peer that stops reading before the final chunk, a peer holding response headers after consuming the whole request, and a peer holding its response body.
- Pause and cancel signaled after the first 64 KiB hash read stop before a second read.
- Private source mutations reproduce failures for the old multipart route, weakened absence guard, pre-upload HTTP read timeout, final-only progress and uninterruptible hash prepass. The refined code passes. The first mutation runs reused the test target; the real source was then touched and rebuilt, and its library/daemon checks passed. Later mutation runs used separate build targets. All owned mutation sources and build scratch were removed after evidence collection.

Independent review found the signing, immutable source, guards, retry recognition, conflict retention, progress accounting and bounded buffering sound after the timeout/progress/Stop refinements. The watchdog also rechecks latest activity at expiry so an obsolete timer cannot expire a fresh deadline or intentional pacing.

`retained-media-after-fix.jsonl` passes publication of the original 171 MB source through the rebuilt daemon: original SHA-256, mode, mtime, xattrs and server history match; six native random scrubs decode 15 frames each and retain the 10-second duration (zero failures/limits). `fresh-media-after-fix.jsonl` also passes a fresh 171 MB save with alias pause verified against the mounted drive ID, original/server SHA and metadata, first/warm scrubs, 2-second HEVC export and manifest SHA. It retains one explicit cold-descriptor limitation; a fresh fork mount is tested separately in the broader matrix. These measurements use an isolated loopback daemon/server and do not establish cross-host locking or power-loss durability.
