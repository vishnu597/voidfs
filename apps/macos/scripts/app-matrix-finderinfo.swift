// SPDX-License-Identifier: Apache-2.0
// Build: xcrun swiftc -swift-version 5 app-matrix-finderinfo.swift -o /private/tmp/finderinfo-probe
// Run: finderinfo-probe <scratch-parent> [listing|full]. Each run owns a new synthetic folder.
// FinderInfo is a fixed 32-byte structure; APFS treats its all-zero value as absent.
import CryptoKit
import Darwin
import Foundation

let fm = FileManager.default
guard [2, 3].contains(CommandLine.arguments.count) else { fputs("finderinfo-probe <scratch-parent> [listing|full]\n", stderr); exit(2) }
let scope = CommandLine.arguments.count == 3 ? CommandLine.arguments[2] : "full"
guard ["listing", "full"].contains(scope) else { fputs("invalid scope\n", stderr); exit(2) }
let parent = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let root = parent.appendingPathComponent("finderinfo-\(UUID().uuidString.lowercased())", isDirectory: true)
var failures = 0
var policyDifferences = 0

func emit(_ record: [String: Any]) {
    let bytes = try! JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
    print(String(decoding: bytes, as: UTF8.self))
    fflush(stdout)
}

func check(_ name: String, _ ok: Bool, _ detail: [String: Any] = [:]) {
    if !ok { failures += 1 }
    emit(detail.merging(["kind": "check", "name": name, "ok": ok], uniquingKeysWith: { _, new in new }))
}

func policy(_ name: String, _ matchesAPFS: Bool, _ detail: [String: Any]) {
    if scope == "full" { check(name, matchesAPFS, detail) }
    else {
        if !matchesAPFS { policyDifferences += 1 }
        emit(detail.merging(["kind": "policy-observation", "name": name, "matchesAPFS": matchesAPFS, "scope": "policy unchanged by listing-only fix"], uniquingKeysWith: { _, new in new }))
    }
}

func set(_ path: String, _ name: String, _ bytes: Data, flags: Int32 = 0) -> Int32 {
    bytes.withUnsafeBytes { setxattr(path, name, $0.baseAddress, bytes.count, 0, flags) }
}

func names(_ path: String) throws -> [String] {
    let size = listxattr(path, nil, 0, 0)
    guard size >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
    var bytes = [CChar](repeating: 0, count: size)
    let n = listxattr(path, &bytes, size, 0)
    guard n >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
    return bytes.prefix(n).split(separator: 0).map { String(decoding: $0.map { UInt8(bitPattern: $0) }, as: UTF8.self) }
}

func value(_ path: String, _ name: String) -> (Data?, Int32) {
    let size = getxattr(path, name, nil, 0, 0, 0)
    guard size >= 0 else { return (nil, errno) }
    var data = Data(count: size)
    let n = data.withUnsafeMutableBytes { getxattr(path, name, $0.baseAddress, size, 0, 0) }
    guard n >= 0 else { return (nil, errno) }
    data.count = n
    return (data, 0)
}

do {
    try fm.createDirectory(at: root, withIntermediateDirectories: true)
    emit(["kind": "run", "root": root.path, "scope": scope, "os": ProcessInfo.processInfo.operatingSystemVersionString])
    let file = root.appendingPathComponent("finderinfo.bin")
    try Data("FinderInfo control\n".utf8).write(to: file)
    let zero = Data(repeating: 0, count: 32)
    var nonzero = zero
    nonzero.replaceSubrange(0..<8, with: Data("TEXTttxt".utf8))
    check("all-zero FinderInfo set succeeds", set(file.path, "com.apple.FinderInfo", zero) == 0, ["errno": errno])
    let zeroNames = try names(file.path)
    let (_, absentError) = value(file.path, "com.apple.FinderInfo")
    check("all-zero FinderInfo omitted from list", !zeroNames.contains("com.apple.FinderInfo"), ["names": zeroNames])
    check("all-zero FinderInfo reads absent", absentError == ENOATTR, ["errno": absentError])
    let created = set(file.path, "com.apple.FinderInfo", nonzero, flags: Int32(XATTR_CREATE))
    policy("nonzero create succeeds after all-zero value", created == 0, ["rc": created, "errno": created == 0 ? 0 : errno])
    check("ordinary nonzero FinderInfo set succeeds", set(file.path, "com.apple.FinderInfo", nonzero) == 0)
    let (actual, readError) = value(file.path, "com.apple.FinderInfo")
    check("nonzero FinderInfo listed and readable", try names(file.path).contains("com.apple.FinderInfo") && actual == nonzero && readError == 0, ["errno": readError])
    let cleared = set(file.path, "com.apple.FinderInfo", zero, flags: Int32(XATTR_REPLACE))
    check("replace nonzero with all-zero succeeds", cleared == 0, ["errno": errno])
    check("cleared FinderInfo omitted and absent", try !names(file.path).contains("com.apple.FinderInfo") && value(file.path, "com.apple.FinderInfo").1 == ENOATTR)
    let replaceAbsent = set(file.path, "com.apple.FinderInfo", nonzero, flags: Int32(XATTR_REPLACE))
    policy("replace absent FinderInfo refuses ENOATTR", replaceAbsent == -1 && errno == ENOATTR, ["rc": replaceAbsent, "errno": replaceAbsent == 0 ? 0 : errno])
    let invalid = set(file.path, "com.apple.FinderInfo", Data(repeating: 0, count: 33))
    check("wrong-size FinderInfo rejected explicitly", invalid == -1, ["rc": invalid, "errno": invalid == 0 ? 0 : errno])
    check("generic all-zero xattr set succeeds", set(file.path, "user.voidfs.zero", zero) == 0)
    check("generic empty xattr set succeeds", set(file.path, "user.voidfs.empty", Data()) == 0)
    check("generic all-zero xattr stays listed and readable", try names(file.path).contains("user.voidfs.zero") && value(file.path, "user.voidfs.zero").0 == zero)
    check("generic empty xattr stays listed and readable", try names(file.path).contains("user.voidfs.empty") && value(file.path, "user.voidfs.empty").0 == Data())
    emit(["kind": "summary", "root": root.path, "scope": scope, "failures": failures, "policyDifferences": policyDifferences,
          "result": failures > 0 ? "fail" : policyDifferences > 0 ? "listing passes; storage policy differences remain" : "pass"])
    exit(failures == 0 ? 0 : 1)
} catch {
    emit(["kind": "error", "message": String(describing: error), "root": root.path])
    exit(2)
}
