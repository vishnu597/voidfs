// SPDX-License-Identifier: Apache-2.0
//
// The app's side of the keychain probe: SecItem calls on the data-protection keychain under
// an access group, as the app's sign-in would make them.
import Foundation
import Security

let args = CommandLine.arguments
let group = ProcessInfo.processInfo.environment["PROBE_GROUP"].flatMap { $0.isEmpty ? nil : $0 }
var base: [String: Any] = [
    kSecClass as String: kSecClassGenericPassword,
    kSecAttrService as String: "dev.voidfs.probe",
    kSecAttrAccount as String: "probe",
    kSecUseDataProtectionKeychain as String: true,
]
if let group { base[kSecAttrAccessGroup as String] = group }
let started = ContinuousClock.now
var status: OSStatus
var text = ""
switch args.count > 1 ? args[1] : "" {
case "write":
    var add = base
    add[kSecValueData as String] = Data(args[2].utf8)
    add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    status = SecItemAdd(add as CFDictionary, nil)
    if status == errSecDuplicateItem {
        status = SecItemUpdate(base as CFDictionary, [kSecValueData as String: Data(args[2].utf8)] as CFDictionary)
    }
    text = "wrote"
case "read":
    var q = base
    q[kSecReturnData as String] = true
    q[kSecReturnAttributes as String] = true
    var out: CFTypeRef?
    status = SecItemCopyMatching(q as CFDictionary, &out)
    if let d = out as? [String: Any] {
        let v = (d[kSecValueData as String] as? Data).map { String(decoding: $0, as: UTF8.self) } ?? "?"
        text = "read \"\(v)\" group=\(d[kSecAttrAccessGroup as String] ?? "?")"
    }
case "delete":
    status = SecItemDelete(base as CFDictionary)
    text = "deleted"
default:
    print("usage: writer read|write <value>|delete (PROBE_GROUP=<access group>)")
    exit(2)
}
let ms = Double((ContinuousClock.now - started).components.attoseconds) / 1e15
if status == errSecSuccess {
    print("swift ok: \(text) (\(String(format: "%.1f", ms)) ms)")
} else {
    print("swift error: OSStatus \(status) \(SecCopyErrorMessageString(status, nil) as String? ?? "") (\(String(format: "%.1f", ms)) ms)")
    exit(1)
}
