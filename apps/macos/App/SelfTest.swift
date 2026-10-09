// SPDX-License-Identifier: Apache-2.0
//
// `voidfs bridge-selftest <drive> [main|daemon-restart|agent-restart|remote <key>|refused|wire]
// [--socket <path>]`: the bridge end to end (the client and its memo, XPC coding, the agent's
// checks and forwarding, the daemon) over an in-process XPC listener, without launchd or FSKit.
// The listener accepts this app's own signature; `refused` shows the extension's requirement
// turning this app away. The signed, sandboxed extension runs the same checks through the real
// agent (`.voidfs-bridge` in a mount).

import Foundation

enum SelfTest {
    /// This app, signed by voidfs's team: what the in-process listener accepts.
    static let ownRequirement = "anchor apple generic and certificate leaf[subject.OU] = \"HAUTK68F56\" and identifier \"dev.voidfs.app\""

    static func run(_ args: [String]) async -> Int32 {
        var rest = args
        var socket = Agent.stateDirectory?.appending(path: "daemon.sock").path
        if let i = rest.firstIndex(of: "--socket"), i + 1 < rest.count {
            socket = rest[i + 1]
            rest.removeSubrange(i...(i + 1))
        }
        guard let drive = rest.first, let socket else {
            print("usage: bridge-selftest <drive> [main|daemon-restart|agent-restart|remote <key>|refused|wire] [--socket <path>]")
            return 2
        }
        let mode = rest.count > 1 ? rest[1] : "main"
        let say: (String) -> Void = { print($0); fflush(stdout) }
        say("bridge self-test: drive \(drive), daemon socket \(socket), \(mode)")
        let listener = NSXPCListener.anonymous()
        let delegate = BridgeListener(socket: DaemonSocket(path: socket), requirement: mode == "refused" ? Bridge.peerRequirement : ownRequirement)
        listener.delegate = delegate
        listener.resume()
        defer { listener.invalidate() }
        let scenario = BridgeScenario(client: BridgeClient(endpoint: listener.endpoint), drive: drive, another: { BridgeClient(endpoint: listener.endpoint) }, say: say)
        switch mode {
        case "main": await scenario.main()
        case "daemon-restart": await scenario.restart("daemon")
        case "agent-restart":
            var dropped: [(String, UInt32)] = []
            await scenario.restart("agent") {
                try? await Task.sleep(for: .milliseconds(200))
                dropped = delegate.daemonSessions()
                delegate.dropAll()
            }
            // The agent releases a lost connection's sessions in the daemon, which then no longer
            // knows them.
            let daemon = DaemonSocket(path: socket)
            var stale = 0
            for _ in 0..<50 where stale < dropped.count {
                stale = dropped.filter { (id, generation) in
                    (try? daemon.requestSync("POST", "/v1/fs/\(id)/getattr", generation: generation, contentType: "application/json", body: Data("{\"ino\":1}".utf8), limit: 1 << 20))
                        .map { $0.status == 409 && String(decoding: $0.body, as: UTF8.self).contains("StaleSession") } ?? false
                }.count
                if stale < dropped.count { try? await Task.sleep(for: .milliseconds(100)) }
            }
            scenario.check("the dropped connection's \(dropped.count) daemon sessions are released", !dropped.isEmpty && stale == dropped.count, "\(stale) of \(dropped.count)")
        case "remote" where rest.count > 2: await scenario.remote(key: rest[2])
        case "wire":
            // The agent's own codecs, on inputs the daemon doesn't send today.
            for (text, seconds, nanos) in [("1970-01-01T00:00:00Z", Int64(0), Int32(0)), ("1969-12-31T23:59:58.5Z", -2, 500_000_000),
                                            ("2026-10-09T12:34:56.123456Z", 1_791_549_296, 123_456_000), ("0000-01-01T00:00:00.000000001Z", -62_167_219_200, 1),
                                            ("9999-12-31T23:59:59.999999999Z", 253_402_300_799, 999_999_999)] {
                let parsed = Wire.time(text)
                scenario.check("parse \(text)", parsed?.0 == seconds && parsed?.1 == nanos, "\(String(describing: parsed))")
                let printed = Wire.timestamp(seconds, nanos).flatMap(Wire.time)
                scenario.check("print and parse \(seconds).\(nanos)", printed?.0 == seconds && printed?.1 == nanos, "\(String(describing: Wire.timestamp(seconds, nanos)))")
            }
            for bad in ["2026-10-09T12:34:56+01:00", "2026-13-01T00:00:00Z", "2026-10-09T12:34:56.Z", "2026-10-09 12:34:56Z", "2026-10-09T12:34:56.1234567890Z"] {
                scenario.check("refuse \(bad)", Wire.time(bad) == nil)
            }
            scenario.check("no time before year 0 or after 9999", Wire.timestamp(-62_167_219_201, 0) == nil && Wire.timestamp(253_402_300_800, 0) == nil)
            scenario.check("escape a name", Wire.encode("user.a é&=%/+?#~") == "user.a%20%C3%A9%26%3D%25%2F%2B%3F%23~", Wire.encode("user.a é&=%/+?#~"))
            say("wire checks: \(scenario.checks), failures: \(scenario.failures)")
        case "refused":
            let ping = await scenario.errno { _ = try await scenario.client.ping(Data(count: 16)) }
            scenario.check("a peer without the extension's signature can't ping", ping != 0, "it answered")
            let open = await scenario.errno { _ = try await scenario.client.open(drive: drive, readOnly: true, watch: false) }
            scenario.check("nor open a session", open != 0, "it opened one")
            say("refused with \(BridgeScenario.name(ping)), then \(BridgeScenario.name(open)); sessions the listener holds: \(delegate.sessionCount)")
            scenario.check("and holds none", delegate.sessionCount == 0)
        default:
            say("unknown mode \(mode)")
            return 2
        }
        return scenario.failures == 0 ? 0 : 1
    }
}
