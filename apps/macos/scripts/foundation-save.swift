// SPDX-License-Identifier: Apache-2.0
// Build: xcrun swiftc -swift-version 5 foundation-save.swift -o /tmp/foundation-save
// Run: foundation-save <scratch-parent> [case-name]
// Every run owns a new folder. Exit 3 preserves evidence of acknowledged wrong bytes;
// exit 1 means a reported error or unsupported path, and exit 2 means a harness error.

import AppKit
import CryptoKit
import Darwin
import Foundation

let fm = FileManager.default
let oldBytes = Data((0..<32781).map { UInt8(($0 * 17 + 31) % 251) })
let newBytes = Data((0..<65557).map { UInt8(($0 * 23 + 43) % 253) })
var errors = 0

func emit(_ values: [String: Any]) {
    var object = values
    object["time"] = Date().timeIntervalSince1970
    let data = try! JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    print(String(decoding: data, as: UTF8.self))
    fflush(stdout)
}

func sha(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }

func describe(_ error: Error) -> [String: Any] {
    let e = error as NSError
    var result: [String: Any] = ["domain": e.domain, "code": e.code, "message": e.localizedDescription]
    if let underlying = e.userInfo[NSUnderlyingErrorKey] as? Error { result["underlying"] = describe(underlying) }
    return result
}

func xattrs(_ url: URL) -> [String: Any] {
    let size = listxattr(url.path, nil, 0, 0)
    guard size >= 0 else { return ["error": String(cString: strerror(errno))] }
    if size == 0 { return [:] }
    var names = [CChar](repeating: 0, count: size)
    let count = listxattr(url.path, &names, size, 0)
    guard count >= 0 else { return ["error": String(cString: strerror(errno))] }
    var values: [String: Any] = [:]
    for nameBytes in names.prefix(count).split(separator: 0) {
        let name = String(decoding: nameBytes.map { UInt8(bitPattern: $0) }, as: UTF8.self)
        let n = getxattr(url.path, name, nil, 0, 0, 0)
        if n < 0 { values[name] = ["error": String(cString: strerror(errno))]; continue }
        var data = Data(count: n)
        let got = data.withUnsafeMutableBytes { getxattr(url.path, name, $0.baseAddress, n, 0, 0) }
        if got < 0 { values[name] = ["error": String(cString: strerror(errno))]; continue }
        data.count = got
        values[name] = ["size": got, "sha256": sha(data)]
    }
    return values
}

func snapshot(_ url: URL) -> [String: Any] {
    do {
        let attrs = try fm.attributesOfItem(atPath: url.path)
        var result: [String: Any] = ["exists": true, "type": String(describing: attrs[.type] ?? "unknown"),
                                   "size": attrs[.size] ?? 0, "mode": attrs[.posixPermissions] ?? 0,
                                   "mtime": (attrs[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0,
                                   "xattrs": xattrs(url)]
        if attrs[.type] as? FileAttributeType == .typeRegular {
            let data = try Data(contentsOf: url)
            result["sha256"] = sha(data)
            result["matchesOld"] = data == oldBytes
            result["matchesNew"] = data == newBytes
        }
        return result
    } catch {
        return ["exists": fm.fileExists(atPath: url.path), "error": describe(error)]
    }
}

func inventory(_ folder: URL) -> [String: Any] {
    var found: [String: Any] = [:]
    guard let items = fm.enumerator(at: folder, includingPropertiesForKeys: nil) else { return found }
    while let url = items.nextObject() as? URL {
        let relative = String(url.resolvingSymlinksInPath().path.dropFirst(folder.resolvingSymlinksInPath().path.count + 1))
        found[relative] = snapshot(url)
    }
    return found
}

func seed(_ url: URL, _ data: Data) throws {
    try data.write(to: url)
    let old = data == oldBytes
    try fm.setAttributes([.posixPermissions: NSNumber(value: old ? 0o640 : 0o600),
                          .modificationDate: Date(timeIntervalSince1970: old ? 1_700_000_123 : 1_700_000_456)], ofItemAtPath: url.path)
    for (name, text) in [("user.voidfs.foundation", old ? "old-metadata" : "new-metadata"),
                         ("com.apple.ResourceFork", old ? "old-resource-fork" : "new-resource-fork")] {
        let attribute = Data(text.utf8)
        let rc = attribute.withUnsafeBytes { setxattr(url.path, name, $0.baseAddress, attribute.count, 0, 0) }
        if rc != 0 { emit(["kind": "metadata-limit", "name": url.lastPathComponent, "attribute": name, "errno": errno, "message": String(cString: strerror(errno))]) }
    }
}

func check(_ caseName: String, _ label: String, _ url: URL, _ expected: Data, evidence: URL) {
    let actual = try? Data(contentsOf: url)
    let ok = actual == expected
    emit(["kind": ok ? "check" : "DATA_LOSS", "case": caseName, "check": label, "ok": ok,
          "expectedSha256": sha(expected), "actual": snapshot(url), "evidenceFolder": evidence.path])
    if !ok {
        emit(["kind": "preserved", "case": caseName, "inventory": inventory(evidence), "folder": evidence.path])
        exit(3)
    }
}

final class ProbeDocument: NSDocument {
    var bytes = Data()
    override class var autosavesInPlace: Bool { true }
    // Version storage is measured separately. These rows isolate AppKit's safe-save path.
    override class var preservesVersions: Bool { false }
    override func data(ofType typeName: String) throws -> Data { bytes }
    override func read(from url: URL, ofType typeName: String) throws { bytes = try Data(contentsOf: url) }
}

func coordinate(_ url: URL, options: NSFileCoordinator.WritingOptions = [], _ body: (URL) throws -> Void) throws {
    var coordinatorError: NSError?
    var writeError: Error?
    var accessed = false
    NSFileCoordinator(filePresenter: nil).coordinate(writingItemAt: url, options: options, error: &coordinatorError) { adjusted in
        accessed = true
        do { try body(adjusted) } catch { writeError = error }
    }
    if let error = coordinatorError { throw error }
    if let error = writeError { throw error }
    if !accessed { throw NSError(domain: "FoundationSaveProbe", code: 1, userInfo: [NSLocalizedDescriptionKey: "coordinator did not call its accessor"]) }
}

func replace(_ original: URL, _ replacement: URL, backup: String? = nil, keep: Bool = false) throws {
    let options: FileManager.ItemReplacementOptions = keep ? .withoutDeletingBackupItem : []
    _ = try fm.replaceItemAt(original, withItemAt: replacement, backupItemName: backup, options: options)
}

func device(_ url: URL) throws -> Int32 {
    var value = stat()
    guard stat(url.path, &value) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
    return value.st_dev
}

let copyStatus: copyfile_callback_t = { what, stage, state, _, _, context in
    let code = errno
    var attribute: UnsafePointer<CChar>?
    _ = copyfile_state_get(state, UInt32(COPYFILE_STATE_XATTRNAME), &attribute)
    let name = context.map { String(cString: $0.assumingMemoryBound(to: CChar.self)) } ?? "unknown"
    emit(["kind": "copyfile-callback", "case": name, "what": what, "stage": stage,
          "attribute": attribute.map { String(cString: $0) } as Any? ?? NSNull(), "errnoAtCallback": code])
    errno = code
    return Int32(COPYFILE_CONTINUE)
}

func copy(_ source: URL, _ destination: URL, flags: copyfile_flags_t, caseName: String, trace: Bool = false) throws {
    if !trace {
        guard copyfile(source.path, destination.path, nil, flags) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        return
    }
    guard let state = copyfile_state_alloc(), let name = strdup(caseName) else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(ENOMEM)) }
    defer { copyfile_state_free(state); free(name) }
    guard copyfile_state_set(state, UInt32(COPYFILE_STATE_STATUS_CB), unsafeBitCast(copyStatus, to: UnsafeRawPointer.self)) == 0,
          copyfile_state_set(state, UInt32(COPYFILE_STATE_STATUS_CTX), name) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
    guard copyfile(source.path, destination.path, state, flags) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
}

let names = ["data-atomic", "replace", "replace-backup-delete", "replace-backup-keep", "replace-temp-directory",
             "coordinator-atomic", "coordinator-replace", "document-save", "document-autosave",
             "file-version", "resource-fork-offsets", "copyfile-data", "copyfile-xattr", "copyfile-xattr-trace", "copyfile-stat", "copyfile-all", "copyfile-all-new",
             "copyfile-all-no-resource-fork", "copyfile-clone", "clone-fallback", "atomic-open-descriptor"]

guard CommandLine.arguments.count >= 2, CommandLine.arguments.count <= 3 else {
    fputs("usage: foundation-save <scratch-parent> [case-name]\n", stderr)
    fputs("cases: \(names.joined(separator: ", "))\n", stderr)
    exit(2)
}
let selected = CommandLine.arguments.count == 3 ? [CommandLine.arguments[2]] : names
guard selected.allSatisfy(names.contains) else { fputs("unknown case\n", stderr); exit(2) }
let parent = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true).standardizedFileURL
let root = parent.appendingPathComponent("foundation-\(UUID().uuidString.lowercased())", isDirectory: true)

do {
    try fm.createDirectory(at: root, withIntermediateDirectories: true)
    var fs = statfs()
    guard statfs(root.path, &fs) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
    let type = withUnsafePointer(to: &fs.f_fstypename) { $0.withMemoryRebound(to: CChar.self, capacity: 16) { String(cString: $0) } }
    emit(["kind": "run", "probe": "foundation-save", "folder": root.path, "filesystem": type,
          "os": ProcessInfo.processInfo.operatingSystemVersionString, "oldSha256": sha(oldBytes), "newSha256": sha(newBytes),
          "cases": selected, "note": "Local API acknowledgement only; verify publication and server history separately. NSDocument rows disable version storage; file-version measures it independently."])
    for name in selected {
        let folder = root.appendingPathComponent(name, isDirectory: true)
        try fm.createDirectory(at: folder, withIntermediateDirectories: false)
        let original = folder.appendingPathComponent("original.bin")
        let replacement = folder.appendingPathComponent("replacement.bin")
        let backup = folder.appendingPathComponent("backup.bin")
        try seed(original, oldBytes)
        try seed(replacement, newBytes)
        emit(["kind": "before", "case": name, "inventory": inventory(folder)])
        let started = Date().timeIntervalSince1970
        var managedVersion: NSFileVersion?
        do {
            switch name {
            case "data-atomic": try newBytes.write(to: original, options: .atomic)
            case "replace": try replace(original, replacement)
            case "replace-backup-delete": try replace(original, replacement, backup: backup.lastPathComponent)
            case "replace-backup-keep": try replace(original, replacement, backup: backup.lastPathComponent, keep: true)
            case "replace-temp-directory":
                let temporary = try fm.url(for: .itemReplacementDirectory, in: .userDomainMask, appropriateFor: original, create: true)
                defer { try? fm.removeItem(at: temporary) }
                let staged = temporary.appendingPathComponent("staged.bin")
                try seed(staged, newBytes)
                emit(["kind": "replacement-directory", "case": name, "sameVolume": try device(temporary) == device(original)])
                try replace(original, staged)
            case "coordinator-atomic": try coordinate(original) { try newBytes.write(to: $0, options: .atomic) }
            case "coordinator-replace": try coordinate(original, options: .forReplacing) { try replace($0, replacement, backup: backup.lastPathComponent, keep: true) }
            case "document-save", "document-autosave":
                let document = ProbeDocument()
                document.fileURL = original
                document.fileType = "public.data"
                try document.read(from: original, ofType: "public.data")
                document.bytes = newBytes
                try document.writeSafely(to: original, ofType: "public.data", for: name == "document-save" ? .saveOperation : .autosaveInPlaceOperation)
                document.close()
            case "file-version":
                var version: NSFileVersion?
                try coordinate(original) { adjusted in
                    version = try NSFileVersion.addOfItem(at: adjusted, withContentsOf: adjusted, options: [])
                }
                guard let saved = version else { throw NSError(domain: "FoundationSaveProbe", code: 2, userInfo: [NSLocalizedDescriptionKey: "NSFileVersion returned no version"]) }
                managedVersion = saved
                // Keep the system's managed version URL out of logs.
                let versionBytes = try Data(contentsOf: saved.url)
                emit(["kind": "version", "case": name, "sha256": sha(versionBytes), "matchesOld": versionBytes == oldBytes,
                      "otherVersions": NSFileVersion.otherVersionsOfItem(at: original)?.count ?? 0])
                if versionBytes != oldBytes { check(name, "version snapshot", saved.url, oldBytes, evidence: folder) }
                try coordinate(original) { try newBytes.write(to: $0, options: .atomic) }
                check(name, "new bytes before restore", original, newBytes, evidence: folder)
                try coordinate(original, options: .forReplacing) { _ = try saved.replaceItem(at: $0, options: []) }
                check(name, "restored old version", original, oldBytes, evidence: folder)
                try saved.remove()
            case "resource-fork-offsets":
                let fd = open(replacement.path, O_RDONLY)
                guard fd >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
                defer { close(fd) }
                let fork = Data("new-resource-fork".utf8)
                for offset in [0, 1, fork.count, fork.count + 1] {
                    var buffer = Data(count: 4096)
                    errno = 0
                    let count = buffer.withUnsafeMutableBytes { fgetxattr(fd, "com.apple.ResourceFork", $0.baseAddress, 4096, UInt32(offset), 0) }
                    let code = errno
                    if count >= 0 { buffer.count = count }
                    let expected = offset < fork.count ? Data(fork.dropFirst(offset)) : Data()
                    let ok = count >= 0 && buffer == expected
                    if !ok { errors += 1 }
                    emit(["kind": ok ? "check" : "read-limit", "case": name, "check": "resource fork positional read including EOF", "position": offset,
                          "ok": ok, "returnedBytes": count, "errno": code, "expectedBytes": expected.count,
                          "sha256": count >= 0 ? sha(buffer) : NSNull()])
                }
            case "copyfile-data", "copyfile-xattr", "copyfile-xattr-trace", "copyfile-stat", "copyfile-all", "copyfile-all-new", "copyfile-all-no-resource-fork", "copyfile-clone":
                if name == "copyfile-all-no-resource-fork" {
                    for url in [original, replacement] {
                        guard removexattr(url.path, "com.apple.ResourceFork", 0) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
                    }
                    emit(["kind": "metadata-control", "case": name, "inventory": inventory(folder), "note": "Resource forks removed; distinct mode, mtime and other xattrs remain."])
                }
                let flags: copyfile_flags_t
                switch name {
                case "copyfile-data": flags = copyfile_flags_t(COPYFILE_DATA)
                case "copyfile-xattr", "copyfile-xattr-trace": flags = copyfile_flags_t(COPYFILE_XATTR)
                case "copyfile-stat": flags = copyfile_flags_t(COPYFILE_STAT)
                case "copyfile-clone": flags = copyfile_flags_t(COPYFILE_ALL | COPYFILE_CLONE)
                default: flags = copyfile_flags_t(COPYFILE_ALL)
                }
                let destination = ["copyfile-clone", "copyfile-all-new"].contains(name) ? folder.appendingPathComponent("copied.bin") : original
                try copy(replacement, destination, flags: flags, caseName: name, trace: name == "copyfile-xattr-trace")
                check(name, "copied destination", destination, ["copyfile-xattr", "copyfile-xattr-trace", "copyfile-stat"].contains(name) ? oldBytes : newBytes, evidence: folder)
            case "clone-fallback":
                let cloned = folder.appendingPathComponent("cloned.bin")
                if clonefile(replacement.path, cloned.path, 0) != 0 {
                    let code = errno
                    emit(["kind": "clone-result", "case": name, "errno": code, "message": String(cString: strerror(code)), "fallback": "copyfile COPYFILE_ALL"])
                    guard code == ENOTSUP || code == EXDEV || code == EINVAL else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(code)) }
                    try copy(replacement, cloned, flags: copyfile_flags_t(COPYFILE_ALL), caseName: name)
                } else { emit(["kind": "clone-result", "case": name, "errno": 0, "fallback": false]) }
                check(name, "cloned or copied destination", cloned, newBytes, evidence: folder)
            case "atomic-open-descriptor":
                let fd = open(original.path, O_RDONLY)
                guard fd >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
                defer { close(fd) }
                _ = fcntl(fd, F_NOCACHE, 1)
                try newBytes.write(to: original, options: .atomic)
                var heldBytes = Data(count: oldBytes.count + 1)
                let capacity = heldBytes.count
                let n = heldBytes.withUnsafeMutableBytes { pread(fd, $0.baseAddress, capacity, 0) }
                guard n >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
                heldBytes.count = n
                let ok = heldBytes == oldBytes
                emit(["kind": ok ? "check" : "DATA_LOSS", "case": name, "check": "old descriptor keeps complete old bytes", "ok": ok, "sha256": sha(heldBytes), "evidenceFolder": folder.path])
                if !ok { emit(["kind": "preserved", "inventory": inventory(folder), "folder": folder.path]); exit(3) }
            default: fatalError("unhandled case")
            }
            emit(["kind": name == "resource-fork-offsets" ? "completed" : "acknowledged", "case": name, "elapsedMs": (Date().timeIntervalSince1970 - started) * 1000])
            if name != "file-version" && name != "resource-fork-offsets" && name != "clone-fallback" && !name.hasPrefix("copyfile") { check(name, "destination holds acknowledged new bytes", original, newBytes, evidence: folder) }
            if name == "replace-backup-keep" || name == "coordinator-replace" { check(name, "retained backup holds old bytes", backup, oldBytes, evidence: folder) }
            if name == "replace-backup-delete" && fm.fileExists(atPath: backup.path) {
                errors += 1
                emit(["kind": "FAIL", "case": name, "check": "successful default replacement removes backup", "actual": snapshot(backup)])
            }
        } catch {
            errors += 1
            let found = inventory(folder)
            let recoverable = found.values.contains { ($0 as? [String: Any])?["matchesOld"] as? Bool == true }
            let managedBytes = managedVersion.flatMap { try? Data(contentsOf: $0.url) }
            let recoverableVersion = managedBytes == oldBytes
            let nonAtomicCopy = name.hasPrefix("copyfile")
            emit(["kind": nonAtomicCopy ? "PARTIAL_FAILURE" : "operation-error", "case": name, "error": describe(error),
                  "oldBytesRecoverableInCaseFolder": recoverable, "oldBytesRecoverableInManagedVersion": recoverableVersion,
                  "managedVersionSha256": managedBytes.map(sha) as Any? ?? NSNull(), "inventory": found,
                  "note": nonAtomicCopy ? "copyfile overwrite is not atomic; errors may leave a partially replaced destination. Source and destination are retained." : "Failed safe-save/version operation; verify recoverable old bytes."])
            if !recoverable && !recoverableVersion && !nonAtomicCopy {
                emit(["kind": "DATA_LOSS", "case": name, "check": "failed safe save keeps old bytes recoverable", "evidenceFolder": folder.path]); exit(3)
            }
        }
        emit(["kind": "after", "case": name, "inventory": inventory(folder)])
    }
    emit(["kind": "summary", "errors": errors, "folder": root.path, "preserved": true])
    exit(errors == 0 ? 0 : 1)
} catch {
    emit(["kind": "harness-error", "error": describe(error), "folder": root.path])
    exit(2)
}
