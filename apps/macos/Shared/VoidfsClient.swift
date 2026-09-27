// SPDX-License-Identifier: Apache-2.0
//
// The calls a read-only mount makes: folder listings with attributes (§4.9), ranged reads (§3),
// extended attributes (§4.8), drive description (§5.4) and the change feed (§5.6).

import Foundation
import os

enum VoidfsError: Error, CustomStringConvertible {
    /// The server answered with an error status; `code` is the S3 error code when there is one.
    case http(status: Int, code: String?)
    /// No answer: the server is down, unreachable, or too slow.
    case network(URLError)
    case badResponse(String)

    var description: String {
        switch self {
        case let .http(status, code): "HTTP \(status) \(code ?? "")"
        case let .network(e): "network: \(e.code.rawValue) \(e.localizedDescription)"
        case let .badResponse(s): "bad response: \(s)"
        }
    }

    /// The errno a file system call should fail with.
    var errno: Int32 {
        switch self {
        case let .http(status, _):
            switch status {
            case 404: ENOENT
            case 401, 403: EACCES
            case 409: EEXIST
            case 412: EAGAIN
            case 413: EFBIG
            default: EIO
            }
        case let .network(e):
            switch e.code {
            case .timedOut: ETIMEDOUT
            case .cannotConnectToHost, .cannotFindHost, .networkConnectionLost, .notConnectedToInternet, .dnsLookupFailed: EHOSTDOWN
            case .cancelled: ECANCELED
            default: EIO
            }
        case .badResponse: EIO
        }
    }
}

/// Where a mount points: `voidfs://<host>[:<port>]/<drive>`, plus `?tls=1` for HTTPS.
struct MountTarget: Sendable, Hashable {
    let endpoint: URL
    let drive: String

    init?(url: URL) {
        guard url.scheme?.lowercased() == "voidfs", let host = url.host, !host.isEmpty else { return nil }
        let drive = url.path(percentEncoded: false).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard !drive.isEmpty, !drive.contains("/") else { return nil }
        let tls = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.contains { $0.name == "tls" && $0.value == "1" } ?? false
        var c = URLComponents()
        c.scheme = tls ? "https" : "http"
        c.host = host
        c.port = url.port
        guard let endpoint = c.url else { return nil }
        self.endpoint = endpoint
        self.drive = drive
    }

    var url: URL { URL(string: "voidfs://\(endpoint.host!)\(endpoint.port.map { ":\($0)" } ?? "")/\(drive)")! }
}

struct ListEntry: Decodable, Sendable {
    let name: String
    let kind: String
    let objectId: String
    let versionId: String?
    let size: UInt64?
    let etag: String?
    let mtime: String
    let mode: String
    let hasXattrs: Bool?
    let target: String?
}

struct Listing: Sendable {
    var seq: UInt64
    var entries: [ListEntry]
}

struct DriveInfo: Decodable, Sendable {
    let driveId: String
    let alias: String?
    let displayName: String?
    let createdAt: String
    let seq: UInt64
    let usageBytes: UInt64?
}

struct Attrs: Decodable, Sendable {
    let objectId: String
    let xattrs: [String: String]
    let flags: [String]?
}

struct Change: Decodable, Sendable {
    let seq: UInt64
    let op: String
    let key: String
    let objectId: String?
    let kind: String?
    let fromKey: String?
}

struct Changes: Decodable, Sendable {
    let seq: UInt64
    let changes: [Change]
    let more: Bool?
}

/// Counts requests so the spike can report requests per file system operation.
final class RequestStats: @unchecked Sendable {
    private let lock = OSAllocatedUnfairLock(initialState: [String: (count: Int, nanos: UInt64, bytes: Int)]())

    func record(_ kind: String, nanos: UInt64, bytes: Int) {
        lock.withLock { s in
            var e = s[kind, default: (0, 0, 0)]
            e.count += 1
            e.nanos += nanos
            e.bytes += bytes
            s[kind] = e
        }
    }

    func snapshot() -> [String: (count: Int, nanos: UInt64, bytes: Int)] { lock.withLock { $0 } }

    func reset() { lock.withLock { $0 = [:] } }
}

final class VoidfsClient: Sendable {
    let target: MountTarget
    let signer: SigV4
    let session: URLSession
    /// Long polls wait longer than any ordinary request may.
    let feedSession: URLSession
    let stats = RequestStats()
    private let log = Logger(subsystem: "dev.voidfs", category: "http")

    init(target: MountTarget, credentials: Credentials, timeout: TimeInterval = 15, connections: Int = 8) {
        self.target = target
        self.signer = SigV4(credentials: credentials)
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = timeout
        config.timeoutIntervalForResource = max(timeout, 300)
        config.httpMaximumConnectionsPerHost = connections
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        config.urlCache = nil
        config.httpCookieStorage = nil
        config.waitsForConnectivity = false
        self.session = URLSession(configuration: config)
        let feed = config.copy() as! URLSessionConfiguration
        feed.timeoutIntervalForRequest = 90
        feed.httpMaximumConnectionsPerHost = 1
        self.feedSession = URLSession(configuration: feed)
    }

    private func url(key: String, query: [(String, String)] = []) -> URL {
        var s = target.endpoint.absoluteString + "/" + SigV4.encode(target.drive)
        if !key.isEmpty { s += "/" + SigV4.encode(key, keepSlash: true) }
        if !query.isEmpty { s += "?" + SigV4.query(query) }
        return URL(string: s)!
    }

    private func send(_ kind: String, _ method: String, _ url: URL, headers: [String: String] = [:], session: URLSession? = nil) async throws -> (Data, HTTPURLResponse) {
        var req = URLRequest(url: url)
        req.httpMethod = method
        for (k, v) in headers { req.setValue(v, forHTTPHeaderField: k) }
        signer.sign(&req)
        let start = DispatchTime.now().uptimeNanoseconds
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await (session ?? self.session).data(for: req)
        } catch let e as URLError {
            log.error("\(kind, privacy: .public) \(url.path(percentEncoded: true), privacy: .public): \(e.code.rawValue) \(e.localizedDescription, privacy: .public)")
            throw VoidfsError.network(e)
        }
        let nanos = DispatchTime.now().uptimeNanoseconds - start
        stats.record(kind, nanos: nanos, bytes: data.count)
        guard let http = response as? HTTPURLResponse else { throw VoidfsError.badResponse("not HTTP") }
        log.debug("\(kind, privacy: .public) \(http.statusCode) \(data.count)B \(Double(nanos) / 1e6, format: .fixed(precision: 2))ms \(url.path(percentEncoded: true), privacy: .public)")
        guard (200..<300).contains(http.statusCode) else {
            throw VoidfsError.http(status: http.statusCode, code: Self.errorCode(data))
        }
        return (data, http)
    }

    private static func errorCode(_ body: Data) -> String? {
        guard let s = String(data: body, encoding: .utf8), let a = s.range(of: "<Code>"), let b = s.range(of: "</Code>") else { return nil }
        return String(s[a.upperBound..<b.lowerBound])
    }

    /// A folder's children (`prefix` is "" for the root or ends in "/"), following pagination.
    func list(prefix: String) async throws -> Listing {
        var listing = Listing(seq: 0, entries: [])
        var token: String?
        struct Page: Decodable { let seq: UInt64; let entries: [ListEntry]; let nextContinuationToken: String? }
        repeat {
            var q = [("x-voidfs-list", ""), ("prefix", prefix)]
            if let token { q.append(("continuation-token", token)) }
            let (data, _) = try await send("list", "GET", url(key: "", query: q))
            let page = try JSONDecoder().decode(Page.self, from: data)
            listing.seq = max(listing.seq, page.seq)
            listing.entries += page.entries
            token = page.nextContinuationToken
        } while token != nil
        return listing
    }

    /// Bytes `offset ..< offset + length` of a file; shorter at the end of the file.
    func read(key: String, offset: UInt64, length: Int) async throws -> Data {
        let (data, _) = try await send("read", "GET", url(key: key), headers: ["Range": "bytes=\(offset)-\(offset + UInt64(length) - 1)"])
        return data
    }

    func attrs(key: String) async throws -> Attrs {
        let (data, _) = try await send("attrs", "GET", url(key: key, query: [("x-voidfs-attrs", "")]))
        return try JSONDecoder().decode(Attrs.self, from: data)
    }

    func describe() async throws -> DriveInfo {
        let (data, _) = try await send("drive", "GET", url(key: "", query: [("x-voidfs-drive", "")]))
        return try JSONDecoder().decode(DriveInfo.self, from: data)
    }

    /// Long-polls the change feed for up to `wait` seconds.
    func changes(since: UInt64, wait: Int) async throws -> Changes {
        let (data, _) = try await send(
            "changes", "GET", url(key: "", query: [("x-voidfs-changes", ""), ("since", String(since))]),
            headers: ["x-voidfs-wait": String(wait)], session: feedSession)
        return try JSONDecoder().decode(Changes.self, from: data)
    }
}

/// Parses the protocol's RFC 3339 timestamps (`2026-09-27T14:02:39.893513Z`) to the nanosecond.
func parseTimestamp(_ s: String) -> timespec {
    var tm = tm()
    var nanos = 0
    let u = Array(s.utf8)
    func num(_ from: Int, _ count: Int) -> Int32 {
        var v: Int32 = 0
        for i in from..<min(from + count, u.count) where u[i] >= 48 && u[i] <= 57 { v = v * 10 + Int32(u[i] - 48) }
        return v
    }
    guard u.count >= 19 else { return timespec() }
    tm.tm_year = num(0, 4) - 1900
    tm.tm_mon = num(5, 2) - 1
    tm.tm_mday = num(8, 2)
    tm.tm_hour = num(11, 2)
    tm.tm_min = num(14, 2)
    tm.tm_sec = num(17, 2)
    var i = 19
    if i < u.count, u[i] == UInt8(ascii: ".") {
        i += 1
        var digits = 0
        while i < u.count, u[i] >= 48, u[i] <= 57 {
            if digits < 9 { nanos = nanos * 10 + Int(u[i] - 48); digits += 1 }
            i += 1
        }
        while digits < 9 { nanos *= 10; digits += 1 }
    }
    var secs = timegm(&tm)
    // A numeric offset instead of `Z` (the server always sends `Z`, but accept both).
    if i + 5 < u.count + 1, i < u.count, u[i] == UInt8(ascii: "+") || u[i] == UInt8(ascii: "-") {
        let off = Int(num(i + 1, 2)) * 3600 + Int(num(i + 4, 2)) * 60
        secs += u[i] == UInt8(ascii: "+") ? -off : off
    }
    return timespec(tv_sec: secs, tv_nsec: nanos)
}

/// Parses an octal mode string such as `0644`.
func parseMode(_ s: String) -> UInt32 { UInt32(s, radix: 8) ?? 0o644 }
