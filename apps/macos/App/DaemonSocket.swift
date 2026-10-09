// SPDX-License-Identifier: Apache-2.0
//
// HTTP/1.1 over the Rust daemon's user-only Unix socket, which is all the launch agent forwards
// to. Requests reuse a small pool of keep-alive connections, one request at a time on each;
// bodies are bounded both ways, and the invalidation watch streams on its own connection.

import Foundation

struct DaemonResponse {
    let status: Int
    let body: Data
}

enum DaemonSocketError: Error {
    /// No daemon answers on the socket.
    case notRunning(Int32)
    case io(Int32)
    case malformed(String)
    case tooLarge
}

final class DaemonSocket: @unchecked Sendable {
    let path: String
    private let lock = NSLock()
    private var idle: [Int32] = []
    private static let maxIdle = 8
    fileprivate static let headerLimit = 16 << 10
    /// As the Rust client's: an ordinary call that takes longer has failed.
    private static let timeout = 30

    init(path: String) { self.path = path }

    deinit { for fd in idle { Darwin.close(fd) } }

    /// The connection that waits longest for a call goes first: the daemon may have closed it.
    private func take() -> (Int32, Bool)? {
        lock.lock(); defer { lock.unlock() }
        return idle.popLast().map { ($0, true) }
    }

    private func give(_ fd: Int32) {
        lock.lock()
        if idle.count < Self.maxIdle { idle.append(fd); lock.unlock() } else { lock.unlock(); Darwin.close(fd) }
    }

    func connect(timeout: Int? = DaemonSocket.timeout) throws -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw DaemonSocketError.io(errno) }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8CString)
        guard bytes.count <= MemoryLayout.size(ofValue: addr.sun_path) else { Darwin.close(fd); throw DaemonSocketError.io(ENAMETOOLONG) }
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in bytes.withUnsafeBytes { raw.copyMemory(from: $0) } }
        addr.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        let connected = withUnsafePointer(to: &addr) { $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) } }
        if connected != 0 {
            let e = errno
            Darwin.close(fd)
            throw (e == ENOENT || e == ECONNREFUSED) ? DaemonSocketError.notRunning(e) : DaemonSocketError.io(e)
        }
        var one: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        if let timeout {
            var tv = timeval(tv_sec: timeout, tv_usec: 0)
            setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
            setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        }
        return fd
    }

    /// One call; its answer's body at most `limit` bytes. Runs the blocking socket work off the
    /// caller's thread.
    func request(_ method: String, _ target: String, generation: UInt32?, contentType: String? = nil, body: Data = Data(), limit: Int) async throws -> DaemonResponse {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                continuation.resume(with: Result { try self.requestSync(method, target, generation: generation, contentType: contentType, body: body, limit: limit) })
            }
        }
    }

    /// The same call on the caller's thread, which it blocks.
    func requestSync(_ method: String, _ target: String, generation: UInt32?, contentType: String? = nil, body: Data = Data(), limit: Int) throws -> DaemonResponse {
        let head = Self.head(method, target, generation: generation, contentType: contentType, length: body.count)
        // A pooled connection the daemon closed while it was idle fails before any answer:
        // that one request is sent again on a new connection.
        for attempt in 0..<2 {
            let pooled = attempt == 0 ? take() : nil
            let (fd, reused) = try pooled ?? (connect(), false)
            var reader = Reader(fd: fd)
            do {
                try Self.send(fd, head, body)
                let (response, keep) = try reader.response(limit: limit)
                if keep { give(fd) } else { Darwin.close(fd) }
                return response
            } catch DaemonSocketError.io(let e) where reused && reader.received == 0 && (e == EPIPE || e == ECONNRESET || e == 0) {
                Darwin.close(fd)
                continue
            } catch {
                Darwin.close(fd)
                throw error
            }
        }
        throw DaemonSocketError.io(ECONNRESET)
    }

    static func head(_ method: String, _ target: String, generation: UInt32?, contentType: String?, length: Int) -> Data {
        var h = "\(method) \(target) HTTP/1.1\r\nHost: localhost\r\nContent-Length: \(length)\r\n"
        if let generation { h += "x-voidfs-generation: \(generation)\r\n" }
        if let contentType { h += "Content-Type: \(contentType)\r\n" }
        return Data((h + "\r\n").utf8)
    }

    static func send(_ fd: Int32, _ parts: Data...) throws {
        for part in parts where !part.isEmpty {
            try part.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
                var sent = 0
                while sent < raw.count {
                    let n = Darwin.write(fd, raw.baseAddress! + sent, raw.count - sent)
                    if n < 0 { if errno == EINTR { continue }; throw DaemonSocketError.io(errno) }
                    sent += n
                }
            }
        }
    }

    /// Streams a long-lived NDJSON answer line by line until it ends, `line` returns false or
    /// `cancel` is called. Lines over `limit` end it with `tooLarge`.
    func stream(_ target: String, generation: UInt32, limit: Int, opened: (Int32) -> Void, line: (Data) -> Bool) throws {
        let fd = try connect(timeout: nil)
        defer { Darwin.close(fd) }
        opened(fd)
        try Self.send(fd, Self.head("GET", target, generation: generation, contentType: nil, length: 0))
        var reader = Reader(fd: fd)
        let (status, headers) = try reader.head()
        guard status == 200 else { throw DaemonCallError(status: status, body: try reader.body(headers, limit: limit)) }
        var pending = Data()
        while let chunk = try reader.chunk(headers, limit: limit) {
            pending.append(chunk)
            while let end = pending.firstIndex(of: 0x0a) {
                let one = pending[pending.startIndex..<end]
                pending.removeSubrange(pending.startIndex...end)
                if !line(Data(one)) { return }
            }
            if pending.count > limit { throw DaemonSocketError.tooLarge }
        }
        if !pending.isEmpty { throw DaemonSocketError.malformed("an invalidation ended before its newline") }
    }

    /// Stops a stream from another thread.
    static func cancel(_ fd: Int32) { shutdown(fd, SHUT_RDWR) }
}

/// A non-success answer to a stream request, carrying the daemon's error body.
struct DaemonCallError: Error {
    let status: Int
    let body: Data
}

/// Reads one HTTP/1.1 answer from a blocking socket.
private struct Reader {
    let fd: Int32
    var buffer = Data()
    var received = 0
    /// Set once the body's framing has been consumed in full, chunked or by length.
    var chunked: Bool?
    var remaining = 0
    var done = false

    init(fd: Int32) { self.fd = fd }

    mutating func fill() throws -> Bool {
        var chunk = [UInt8](repeating: 0, count: 64 << 10)
        while true {
            let n = chunk.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress!, $0.count) }
            if n < 0 {
                if errno == EINTR { continue }
                throw DaemonSocketError.io(errno == EAGAIN ? ETIMEDOUT : errno)
            }
            if n == 0 { return false }
            buffer.append(contentsOf: chunk[0..<n])
            received += n
            return true
        }
    }

    mutating func line(limit: Int) throws -> Data {
        while true {
            if let r = buffer.firstRange(of: Data([0x0d, 0x0a])) {
                let l = buffer[buffer.startIndex..<r.lowerBound]
                buffer.removeSubrange(buffer.startIndex..<r.upperBound)
                return Data(l)
            }
            if buffer.count > limit { throw DaemonSocketError.tooLarge }
            guard try fill() else { throw DaemonSocketError.io(0) }
        }
    }

    mutating func exactly(_ count: Int) throws -> Data {
        while buffer.count < count { guard try fill() else { throw DaemonSocketError.malformed("the answer ended early") } }
        let d = buffer.prefix(count)
        buffer.removeFirst(count)
        return Data(d)
    }

    mutating func head() throws -> (Int, [String: String]) {
        let status = try line(limit: 4096)
        let parts = String(decoding: status, as: UTF8.self).split(separator: " ", maxSplits: 2)
        guard parts.count >= 2, parts[0].hasPrefix("HTTP/1."), let code = Int(parts[1]) else { throw DaemonSocketError.malformed("status line") }
        var headers: [String: String] = [:]
        var size = status.count
        while true {
            let l = try line(limit: 4096)
            size += l.count
            if size > DaemonSocket.headerLimit { throw DaemonSocketError.tooLarge }
            if l.isEmpty { break }
            let text = String(decoding: l, as: UTF8.self)
            guard let colon = text.firstIndex(of: ":") else { throw DaemonSocketError.malformed("header") }
            headers[text[..<colon].lowercased()] = text[text.index(after: colon)...].trimmingCharacters(in: .whitespaces)
        }
        if code == 100 { return try head() }
        return (code, headers)
    }

    /// The next piece of the body, or nil at its end.
    mutating func chunk(_ headers: [String: String], limit: Int) throws -> Data? {
        if done { return nil }
        if chunked == nil {
            if headers["transfer-encoding"]?.lowercased().contains("chunked") == true { chunked = true }
            else {
                chunked = false
                guard let length = headers["content-length"].flatMap(Int.init), length >= 0 else { throw DaemonSocketError.malformed("no length") }
                guard length <= limit else { throw DaemonSocketError.tooLarge }
                remaining = length
            }
        }
        if chunked == true {
            let sizeLine = try line(limit: 64)
            let text = String(decoding: sizeLine, as: UTF8.self).split(separator: ";").first.map(String.init) ?? ""
            guard let size = Int(text.trimmingCharacters(in: .whitespaces), radix: 16), size >= 0 else { throw DaemonSocketError.malformed("chunk size") }
            if size == 0 {
                while try !line(limit: 4096).isEmpty {}
                done = true
                return nil
            }
            guard size <= limit else { throw DaemonSocketError.tooLarge }
            let data = try exactly(size)
            guard try line(limit: 2).isEmpty else { throw DaemonSocketError.malformed("chunk end") }
            return data
        }
        if remaining == 0 { done = true; return nil }
        if buffer.isEmpty { guard try fill() else { throw DaemonSocketError.malformed("the answer ended early") } }
        let n = min(remaining, buffer.count)
        let data = buffer.prefix(n)
        buffer.removeFirst(n)
        remaining -= n
        return Data(data)
    }

    mutating func body(_ headers: [String: String], limit: Int) throws -> Data {
        var body = Data()
        while let piece = try chunk(headers, limit: limit) {
            body.append(piece)
            if body.count > limit { throw DaemonSocketError.tooLarge }
        }
        return body
    }

    /// The whole answer, and whether its connection can carry another request.
    mutating func response(limit: Int) throws -> (DaemonResponse, Bool) {
        let (status, headers) = try head()
        // An error body may be larger than the bytes asked for, but never than the daemon's
        // 1 MiB response bound.
        let body = try self.body(headers, limit: (200..<300).contains(status) ? limit : max(limit, 1 << 20))
        let keep = headers["connection"]?.lowercased() != "close" && buffer.isEmpty
        return (DaemonResponse(status: status, body: body), keep)
    }
}
