// SPDX-License-Identifier: Apache-2.0
//
// AWS Signature Version 4 for the requests a mount makes (protocol §2): header authentication,
// service `s3`, path-style addressing, single percent-encoding, no path normalization.

import CryptoKit
import Foundation

struct Credentials: Sendable {
    let accessKeyId: String
    let secretAccessKey: String
}

struct SigV4: Sendable {
    let credentials: Credentials
    var region = "us-east-1"

    static let emptyPayloadHash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

    /// Adds `x-amz-date`, `x-amz-content-sha256` and `Authorization` to `request`. The URL must
    /// already be in canonical form: path segments and query built with `SigV4.encode`, and the
    /// query sorted.
    func sign(_ request: inout URLRequest, payloadHash: String = emptyPayloadHash, date: Date = Date()) {
        let url = request.url!
        let (amzDate, day) = Self.timestamps(date)
        var host = url.host!
        if let port = url.port, !(url.scheme == "http" && port == 80), !(url.scheme == "https" && port == 443) {
            host += ":\(port)"
        }
        request.setValue(amzDate, forHTTPHeaderField: "x-amz-date")
        request.setValue(payloadHash, forHTTPHeaderField: "x-amz-content-sha256")

        // Every x-voidfs-* header must be signed (protocol §2); sign them with the required three.
        var headers: [(String, String)] = [("host", host), ("x-amz-content-sha256", payloadHash), ("x-amz-date", amzDate)]
        for (name, value) in request.allHTTPHeaderFields ?? [:] where name.lowercased().hasPrefix("x-voidfs-") {
            headers.append((name.lowercased(), value.trimmingCharacters(in: .whitespaces)))
        }
        headers.sort { $0.0 < $1.0 }
        let signedHeaders = headers.map(\.0).joined(separator: ";")
        let canonical = [
            request.httpMethod ?? "GET",
            url.path(percentEncoded: true).isEmpty ? "/" : url.path(percentEncoded: true),
            url.query(percentEncoded: true) ?? "",
            headers.map { "\($0.0):\($0.1)\n" }.joined(),
            signedHeaders,
            payloadHash,
        ].joined(separator: "\n")
        let scope = "\(day)/\(region)/s3/aws4_request"
        let toSign = "AWS4-HMAC-SHA256\n\(amzDate)\n\(scope)\n\(Self.hex(SHA256.hash(data: Data(canonical.utf8))))"
        let signature = Self.hex(HMAC<SHA256>.authenticationCode(for: Data(toSign.utf8), using: signingKey(day: day)))
        request.setValue(
            "AWS4-HMAC-SHA256 Credential=\(credentials.accessKeyId)/\(scope), SignedHeaders=\(signedHeaders), Signature=\(signature)",
            forHTTPHeaderField: "Authorization")
    }

    private func signingKey(day: String) -> SymmetricKey {
        func mac(_ key: SymmetricKey, _ data: String) -> SymmetricKey {
            SymmetricKey(data: HMAC<SHA256>.authenticationCode(for: Data(data.utf8), using: key))
        }
        let secret = SymmetricKey(data: Data("AWS4\(credentials.secretAccessKey)".utf8))
        return mac(mac(mac(mac(secret, day), region), "s3"), "aws4_request")
    }

    private static func timestamps(_ date: Date) -> (String, String) {
        var t = time_t(date.timeIntervalSince1970)
        var tm = tm()
        gmtime_r(&t, &tm)
        let day = String(format: "%04d%02d%02d", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
        return (day + String(format: "T%02d%02d%02dZ", tm.tm_hour, tm.tm_min, tm.tm_sec), day)
    }

    static func hex<D: Sequence>(_ digest: D) -> String where D.Element == UInt8 {
        digest.map { String(format: "%02x", $0) }.joined()
    }

    private static let unreserved: Set<UInt8> = Set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~".utf8)

    /// RFC 3986 encoding as SigV4 wants it: every byte but the unreserved ones, and `/` too
    /// unless `keepSlash`.
    static func encode(_ s: String, keepSlash: Bool = false) -> String {
        var out = ""
        for b in s.utf8 {
            if unreserved.contains(b) || (keepSlash && b == UInt8(ascii: "/")) {
                out.append(Character(UnicodeScalar(b)))
            } else {
                out += String(format: "%%%02X", b)
            }
        }
        return out
    }

    /// A canonical, sorted query string: `a=1&b=`.
    static func query(_ items: [(String, String)]) -> String {
        let encoded: [(key: String, value: String)] = items.map { (encode($0.0), encode($0.1)) }
        let sorted = encoded.sorted { a, b in a.key == b.key ? a.value < b.value : a.key < b.key }
        return sorted.map { "\($0.key)=\($0.value)" }.joined(separator: "&")
    }
}
