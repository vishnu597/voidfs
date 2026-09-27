// SPDX-License-Identifier: Apache-2.0
//
// Spike instrumentation: an XPC echo, to measure whether and how fast the sandboxed FSKit
// extension can reach a launchd agent (scripts/xpc-echo-agent.swift).

import Foundation

@objc protocol XPCEcho {
    func echo(_ data: Data, reply: @escaping (Data) -> Void)
}

enum XPCEchoProbe {
    /// Sends `count` round trips of `bytes` to the mach service `name`, and describes the
    /// latency, or the error.
    static func run(name: String, count: Int = 500, bytes: Int = 4096) async -> String {
        let connection = NSXPCConnection(machServiceName: name)
        connection.remoteObjectInterface = NSXPCInterface(with: XPCEcho.self)
        connection.resume()
        defer { connection.invalidate() }
        let payload = Data(count: bytes)
        var lat: [Double] = []
        for _ in 0..<count {
            let start = ContinuousClock.now
            let result: Result<Data, Error> = await withCheckedContinuation { cont in
                let proxy = connection.remoteObjectProxyWithErrorHandler { cont.resume(returning: .failure($0)) } as! XPCEcho
                proxy.echo(payload) { cont.resume(returning: .success($0)) }
            }
            if case let .failure(e) = result { return "\(name): \(e.localizedDescription)" }
            let d = ContinuousClock.now - start
            lat.append(Double(d.components.seconds) * 1e6 + Double(d.components.attoseconds) / 1e12)
        }
        lat.sort()
        return String(format: "%@: %d x %d B round trips, p50 %.0f µs, p90 %.0f µs, p99 %.0f µs", name, count, bytes, lat[count / 2], lat[count * 9 / 10], lat[count * 99 / 100])
    }
}
