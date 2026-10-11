// SPDX-License-Identifier: Apache-2.0
//
// The FSKit entry point: recognizes `voidfs://<drive>` URL resources and loads each as one
// read-only volume, served by the daemon through the app's launch agent. The resource names only
// the drive: the daemon holds the server and its key, so nothing secret reaches the extension, the
// URL or `mount`'s output.

import CryptoKit
import FSKit
import Foundation
import os

final class VoidfsFileSystem: FSUnaryFileSystem, FSUnaryFileSystemOperations {
    private let log = Logger(subsystem: "dev.voidfs", category: "fs")

    func didFinishLoading() {
        log.notice("module loaded: pid \(getpid()) uid \(getuid()) agent \(Bridge.service, privacy: .public)")
    }

    /// The drive a resource names: `voidfs://<drive>`, with nothing else. The spike's
    /// `voidfs://host:port/drive` named a server, which the daemon now chooses.
    static func drive(_ url: URL) -> String? {
        guard url.scheme?.lowercased() == "voidfs", let host = url.host(percentEncoded: false), !host.isEmpty, url.port == nil,
              url.user == nil, url.password == nil, url.query == nil, url.fragment == nil, url.path(percentEncoded: false).isEmpty || url.path() == "/" else { return nil }
        return host.utf8.count <= Bridge.maxDrive && !host.contains("/") && !host.contains("\0") ? host : nil
    }

    private func drive(_ resource: FSResource) -> String? {
        (resource as? FSGenericURLResource).flatMap { Self.drive($0.url) }
    }

    /// A stable container id per drive name, so the system sees the same volume on every mount.
    private static func uuid(_ drive: String) -> UUID {
        var b = Array(SHA256.hash(data: Data("voidfs:\(drive)".utf8)).prefix(16))
        b[6] = (b[6] & 0x0f) | 0x50
        b[8] = (b[8] & 0x3f) | 0x80
        return UUID(uuid: (b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]))
    }

    func probeResource(resource: FSResource) async throws -> FSProbeResult {
        guard let drive = drive(resource) else { return .notRecognized }
        return .usable(name: drive, containerID: FSContainerIdentifier(uuid: Self.uuid(drive)))
    }

    func loadResource(resource: FSResource, options: FSTaskOptions) async throws -> FSVolume {
        guard let drive = drive(resource) else {
            log.error("load: not voidfs://<drive>: \(String(describing: resource), privacy: .public) (voidfs://host:port/drive named a server; the daemon now chooses it)")
            throw fs_errorForPOSIXError(EINVAL)
        }
        log.notice("load \(drive, privacy: .public) options \(options.taskOptions, privacy: .public)")
        // Fail the mount now, with a clear error, rather than on the first `ls`: the daemon must
        // be running, know the drive, and relay its invalidations.
        let client = BridgeClient()
        let session: BridgeSession
        // launchd may keep a message for an agent it can't start (a launch constraint, say)
        // without failing it: give up rather than hang `mount`.
        let timer = Task {
            try await Task.sleep(for: .seconds(15))
            log.error("load \(drive, privacy: .public): the agent didn't answer in 15 s")
            client.reset()
        }
        defer { timer.cancel() }
        do {
            session = try await client.open(drive: drive, readOnly: true)
        } catch {
            let e = Bridge.errno(error)
            log.error("load \(drive, privacy: .public): \(error.localizedDescription, privacy: .public) (errno \(e)); is the daemon running (void daemon start) and the agent registered (voidfs agent-register)?")
            throw fs_errorForPOSIXError(e == ECONNREFUSED ? EIO : e)
        }
        let volume = VoidfsVolume(client: client, session: session, drive: drive, volumeID: FSVolume.Identifier(uuid: Self.uuid(drive)))
        containerStatus = .ready
        log.notice("loaded \(drive, privacy: .public): root inode \(session.info.root), daemon generation \(session.info.generation)")
        return volume
    }

    func unloadResource(resource: FSResource, options: FSTaskOptions) async throws {
        log.notice("unload \(String(describing: resource), privacy: .public)")
    }
}
