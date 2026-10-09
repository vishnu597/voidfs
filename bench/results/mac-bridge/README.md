# The Swift bridge: logs, 9 October 2026

Raw output behind [step 5's bridge sections](../../../docs/step-5-macos.md#the-swift-bridge-9-october):
macOS 27.0.1, Xcode 27 beta (27A5194q), a release `voidfs-server` on a memory store at
127.0.0.1:9000 with the spike's throwaway key, and the test drive `spike`.

| File | What |
|---|---|
| `extension.log` | Every `bridge`, `xpc` and `agent` line the extension and the agent logged: the hooks' checks and timings, restarts, the remote change, the failed first spawn under the old launch constraint. The 11:39 runs are the final ones; at 11:49 the daemon is at the CLI's default location |
| `selftest-*.log` | `voidfs bridge-selftest` in process. `selftest-first-run.log` is the first run, which met the core's `EAGAIN` before the client retried it |
| `container-*.log`, `default-location-launchd-daemon.log` | The daemon run by a transient launchd job with its state in the App Group container (unsigned, signed and entitled, inside the app's bundle), a client there, and the daemon at the CLI's default location |
| `breaks-pr52.log`, `breaks-pr53.log`, `breaks-pr54.log` | The failure proofs: each break, whether its test caught it, and the failing check |
