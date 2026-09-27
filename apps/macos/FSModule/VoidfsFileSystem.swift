// SPDX-License-Identifier: Apache-2.0
//
// The FSKit entry point: recognizes `voidfs://host[:port]/drive` URL resources and loads each as
// one read-only volume.

import CryptoKit
import FSKit
import Foundation
import os

final class VoidfsFileSystem: FSUnaryFileSystem, FSUnaryFileSystemOperations {
    private let log = Logger(subsystem: "dev.voidfs", category: "fs")

    func didFinishLoading() {
        log.notice("module loaded: pid \(getpid()) uid \(getuid()) home \(NSHomeDirectory(), privacy: .public) group \(MountStore.fileURL?.path ?? "none", privacy: .public)")
    }

    private func target(_ resource: FSResource) -> (URL, MountTarget)? {
        guard let r = resource as? FSGenericURLResource, let t = MountTarget(url: r.url) else { return nil }
        return (r.url, t)
    }

    /// A stable container id per endpoint and drive, so the system sees the same volume on
    /// every mount.
    private static func uuid(_ target: MountTarget) -> UUID {
        var b = Array(SHA256.hash(data: Data(target.url.absoluteString.utf8)).prefix(16))
        b[6] = (b[6] & 0x0f) | 0x50
        b[8] = (b[8] & 0x3f) | 0x80
        return UUID(uuid: (b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]))
    }

    func probeResource(resource: FSResource) async throws -> FSProbeResult {
        log.notice("probe \(String(describing: resource), privacy: .public)")
        guard let (_, t) = target(resource) else { return .notRecognized }
        return .usable(name: t.drive, containerID: FSContainerIdentifier(uuid: Self.uuid(t)))
    }

    func loadResource(resource: FSResource, options: FSTaskOptions) async throws -> FSVolume {
        guard let (url, t) = target(resource) else {
            log.error("load: not a voidfs URL: \(String(describing: resource), privacy: .public)")
            throw fs_errorForPOSIXError(EINVAL)
        }
        log.notice("load \(t.url.absoluteString, privacy: .public) options \(options.taskOptions, privacy: .public)")
        guard let credentials = MountStore.credentials(for: url, target: t) else {
            log.error("load: no access key for \(t.url.absoluteString, privacy: .public)")
            containerStatus = .blocked(status: fs_errorForPOSIXError(EAUTH))
            throw fs_errorForPOSIXError(EAUTH)
        }
        let client = VoidfsClient(target: t, credentials: credentials)
        // Fail the mount now, with a clear error, rather than on the first `ls`.
        let drive: DriveInfo
        do {
            drive = try await client.describe()
        } catch {
            log.error("load: \(t.url.absoluteString, privacy: .public): \(String(describing: error), privacy: .public)")
            throw posix(error)
        }
        let volume = VoidfsVolume(client: client, drive: drive, volumeID: FSVolume.Identifier(uuid: Self.uuid(t)))
        containerStatus = .ready
        log.notice("loaded drive \(drive.driveId, privacy: .public) at seq \(drive.seq)")
        return volume
    }

    func unloadResource(resource: FSResource, options: FSTaskOptions) async throws {
        log.notice("unload \(String(describing: resource), privacy: .public)")
    }
}
