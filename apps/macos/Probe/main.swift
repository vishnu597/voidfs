// SPDX-License-Identifier: Apache-2.0
//
// voidfs-probe: runs the mount's HTTP calls directly, without FSKit, to check signing and to give
// a baseline for the mount's numbers.
//
//     VOIDFS_ACCESS_KEY_ID=... VOIDFS_SECRET_ACCESS_KEY=... voidfs-probe voidfs://127.0.0.1:9000/spike list many/
//     voidfs-probe <url> read <key> [block-bytes] [parallel]      sequential read, prints MB/s and SHA-256
//     voidfs-probe <url> random <key> [count] [bytes]             random small reads, prints latency
//     voidfs-probe <url> changes <since> [wait-seconds]

import CryptoKit
import Foundation

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(2)
}

func percentile(_ sorted: [Double], _ p: Double) -> Double {
    sorted[min(sorted.count - 1, Int(Double(sorted.count) * p))]
}

let args = CommandLine.arguments
guard args.count >= 3, let url = URL(string: args[1]), let target = MountTarget(url: url) else {
    fail("usage: voidfs-probe voidfs://host:port/drive list|read|random|changes …")
}
let env = ProcessInfo.processInfo.environment
guard let keyId = env["VOIDFS_ACCESS_KEY_ID"], let secret = env["VOIDFS_SECRET_ACCESS_KEY"] else {
    fail("set VOIDFS_ACCESS_KEY_ID and VOIDFS_SECRET_ACCESS_KEY")
}
let client = VoidfsClient(target: target, credentials: Credentials(accessKeyId: keyId, secretAccessKey: secret), connections: 16)
let clock = ContinuousClock()

func seconds(_ d: Duration) -> Double { Double(d.components.seconds) + Double(d.components.attoseconds) / 1e18 }

switch args[2] {
case "list":
    let prefix = args.count > 3 ? args[3] : ""
    for run in 1...3 {
        var listing = Listing(seq: 0, entries: [])
        let t = try await clock.measure { listing = try await client.list(prefix: prefix) }
        print("list \(prefix.isEmpty ? "/" : prefix): \(listing.entries.count) entries, seq \(listing.seq), \(String(format: "%.1f", seconds(t) * 1000)) ms (run \(run))")
    }

case "read":
    guard args.count > 3 else { fail("read <key> [block] [parallel]") }
    let key = args[3]
    let block = args.count > 4 ? Int(args[4])! : 1 << 20
    let parallel = args.count > 5 ? Int(args[5])! : 1
    let size = try await client.list(prefix: key.contains("/") ? String(key[...key.lastIndex(of: "/")!]) : "")
        .entries.first { $0.name == key.split(separator: "/").last.map(String.init) }?.size ?? 0
    guard size > 0 else { fail("\(key) not found") }
    var hasher = SHA256()
    let t = try await clock.measure {
        var offset: UInt64 = 0
        while offset < size {
            // `parallel` blocks in flight, hashed in order.
            let starts = (0..<parallel).map { offset + UInt64($0 * block) }.filter { $0 < size }
            let blocks = try await withThrowingTaskGroup(of: (UInt64, Data).self) { group in
                for s in starts { group.addTask { (s, try await client.read(key: key, offset: s, length: Int(min(UInt64(block), size - s)))) } }
                var out = [(UInt64, Data)]()
                for try await r in group { out.append(r) }
                return out.sorted { $0.0 < $1.0 }
            }
            for (_, d) in blocks { hasher.update(data: d) }
            offset = starts.last! + UInt64(blocks.last!.1.count)
        }
    }
    let mb = Double(size) / 1_048_576
    print(String(format: "read %@: %.0f MiB in %.2f s = %.0f MiB/s (block %d KiB, %d in flight)", key, mb, seconds(t), mb / seconds(t), block / 1024, parallel))
    print("sha256 \(SigV4.hex(hasher.finalize()))")

case "random":
    guard args.count > 3 else { fail("random <key> [count] [bytes]") }
    let key = args[3]
    let count = args.count > 4 ? Int(args[4])! : 200
    let bytes = args.count > 5 ? Int(args[5])! : 4096
    let size: UInt64 = 1 << 30
    var lat = [Double]()
    for _ in 0..<count {
        let off = UInt64.random(in: 0..<(size - UInt64(bytes))) & ~4095
        let t = try await clock.measure { _ = try await client.read(key: key, offset: off, length: bytes) }
        lat.append(seconds(t) * 1000)
    }
    lat.sort()
    print(String(format: "random %d-byte reads x%d: p50 %.2f ms, p90 %.2f ms, p99 %.2f ms, max %.2f ms", bytes, count, percentile(lat, 0.5), percentile(lat, 0.9), percentile(lat, 0.99), lat.last!))

case "changes":
    let since = args.count > 3 ? UInt64(args[3])! : 0
    let wait = args.count > 4 ? Int(args[4])! : 0
    let c = try await client.changes(since: since, wait: wait)
    print("seq \(c.seq), \(c.changes.count) changes, more \(c.more ?? false)")
    for ch in c.changes.prefix(20) { print("  \(ch.seq) \(ch.op) \(ch.key)") }

case "xpc":
    // Spike instrumentation: round trips to scripts/xpc-echo-agent from an unsandboxed process.
    print(await XPCEchoProbe.run(name: args.count > 3 ? args[3] : "\(MountStore.appGroup).agent"))

default:
    fail("unknown command \(args[2])")
}
