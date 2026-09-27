// SPDX-License-Identifier: Apache-2.0
//
// Where the host app leaves access keys for the file system extension: a JSON file in the App
// Group container both are entitled to. A spike stand-in for a shared Keychain item.

import Foundation

enum MountStore {
    static let appGroup = "HAUTK68F56.dev.voidfs"

    struct Entry: Codable {
        var accessKeyId: String
        var secretAccessKey: String
    }

    static var fileURL: URL? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup)?.appending(path: "mounts.json")
    }

    static func load() -> [String: Entry] {
        guard let url = fileURL, let data = try? Data(contentsOf: url) else { return [:] }
        return (try? JSONDecoder().decode([String: Entry].self, from: data)) ?? [:]
    }

    static func save(_ target: MountTarget, _ entry: Entry) throws {
        guard let url = fileURL else { throw CocoaError(.fileNoSuchFile) }
        var all = load()
        all[target.url.absoluteString] = entry
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try JSONEncoder().encode(all).write(to: url, options: [.atomic])
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    /// The access key for a mount URL: from the store, or (development only) the URL's user and
    /// password.
    static func credentials(for url: URL, target: MountTarget) -> Credentials? {
        if let e = load()[target.url.absoluteString] {
            return Credentials(accessKeyId: e.accessKeyId, secretAccessKey: e.secretAccessKey)
        }
        if let user = url.user(percentEncoded: false), let password = url.password(percentEncoded: false) {
            return Credentials(accessKeyId: user, secretAccessKey: password)
        }
        return nil
    }
}
