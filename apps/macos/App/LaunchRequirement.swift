// SPDX-License-Identifier: Apache-2.0
//
// A launchd job's kept launch requirement (step 5, item 7). Background Task Management keeps a
// job's requirement, its LWCR (signing identifier, team and validation category), across
// unregistering and registering. After an update that changed a job's program, or a switch
// between differently signed copies of the app, launchd refuses the job until it is registered
// again: it doesn't start, or it ends with exit code 78 (EX_CONFIG). On 18 October the bridge agent
// kept a development build's requirement (validation category 3) in the Developer ID copy (6)
// across one round of registering again, and a second round cleared it
// (`bench/results/item7-login`). Both of the app's jobs, the login daemon's (LoginItem.swift) and
// the bridge agent's (Agent.swift), run this binary, so their requirement must name its signature.

import Foundation
import Security

enum LaunchRequirement {
    /// A code signature, as a launch requirement names one.
    struct Signature: Equatable {
        var identifier: String
        var team: String?
        /// launchd's validation category: 3 for a development build, 6 for Developer ID.
        var category: Int?
    }

    /// How many times a refused job is registered again: one round kept the agent's old
    /// requirement on 18 October, and a second cleared it.
    static let rounds = 2

    /// Signed with an Apple Development certificate (validation category 3).
    static let development = "anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.12] exists"
    /// Signed with a Developer ID Application certificate (validation category 6).
    static let developerID = "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"

    static func describe(category: Int) -> String {
        let name = switch category {
        case 1: "platform"
        case 2: "TestFlight"
        case 3: "development"
        case 4: "App Store"
        case 5: "enterprise"
        case 6: "Developer ID"
        case 7: "local signing"
        default: "unknown"
        }
        return "\(category) (\(name))"
    }

    /// `launchctl print` for the job `label` in this login session, or `nil` if launchd hasn't
    /// loaded it.
    static func printed(_ label: String) -> String? {
        let (status, text) = SignIn.launchctl(["print", "gui/\(getuid())/\(label)"])
        return status == 0 ? text : nil
    }

    /// The launch refusal `launchctl print` shows for a job whose program is signed `own`: a
    /// requirement (LWCR) that names another signing identifier, team or validation category, or a
    /// last exit code of 78 (EX_CONFIG), which launchd gives a job it can't start. Neither `void`
    /// nor this binary exits 78. A job launchd runs now isn't refused, whatever its requirement:
    /// another copy's program may be running. Without `own`, as for an unsigned binary, only exit
    /// code 78 counts; without its team or category, those aren't compared.
    static func refusal(in printed: String, own: Signature?) -> String? {
        let lines = printed.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        func tabs(_ line: String) -> Int { line.prefix { $0 == "\t" }.count }
        func trimmed(_ line: String) -> String { line.trimmingCharacters(in: .whitespaces) }
        // The job's own state is one level in; its coalitions' are deeper.
        if lines.first(where: { tabs($0) == 1 && trimmed($0).hasPrefix("state = ") }).map(trimmed) == "state = running" { return nil }
        if let own, let start = lines.firstIndex(where: { trimmed($0) == "LWCR = {" }) {
            let depth = tabs(lines[start])
            var named: [String: String] = [:]
            for line in lines[(start + 1)...] {
                let t = trimmed(line)
                if t == "}" && tabs(line) == depth { break }
                for key in ["signing-identifier", "team-identifier", "validation-category"] where named[key] == nil {
                    let prefix = "\"\(key)\" => "
                    guard t.hasPrefix(prefix) else { continue }
                    let value = t.dropFirst(prefix.count)
                    named[key] = String(value.hasPrefix("\"") && value.hasSuffix("\"") && value.count >= 2 ? value.dropFirst().dropLast() : value)
                }
            }
            if let identifier = named["signing-identifier"], identifier != own.identifier {
                return "its launch requirement names \(identifier), not \(own.identifier)"
            }
            if let team = named["team-identifier"], let ownTeam = own.team, team != ownTeam {
                return "its launch requirement names team \(team), not \(ownTeam)"
            }
            if let category = named["validation-category"].flatMap({ Int($0) }), let ownCategory = own.category, category != ownCategory {
                return "its launch requirement names validation category \(describe(category: category)), not \(describe(category: ownCategory))"
            }
        }
        if lines.contains(where: { trimmed($0).hasPrefix("last exit code = 78") }) {
            return "it can't start the program: exit code 78, EX_CONFIG"
        }
        return nil
    }

    /// This binary's signature: the program both jobs run.
    static let own: Signature? = signature(of: nil)

    /// The signature of this binary, or of the code at `url`. Its validation category is read
    /// from its certificate: Apple Development is 3 and Developer ID 6. Any other is left out, so
    /// that it isn't compared.
    static func signature(of url: URL?) -> Signature? {
        var staticCode: SecStaticCode?
        let satisfies: (SecRequirement) -> Bool
        if let url {
            guard SecStaticCodeCreateWithPath(url as CFURL, [], &staticCode) == errSecSuccess, let staticCode else { return nil }
            // Only the signature's certificates: the requirement says nothing about the files.
            satisfies = { SecStaticCodeCheckValidity(staticCode, SecCSFlags(rawValue: kSecCSBasicValidateOnly), $0) == errSecSuccess }
        } else {
            var code: SecCode?
            guard SecCodeCopySelf([], &code) == errSecSuccess, let code,
                  SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess else { return nil }
            satisfies = { SecCodeCheckValidity(code, [], $0) == errSecSuccess }
        }
        var info: CFDictionary?
        guard let staticCode, SecCodeCopySigningInformation(staticCode, SecCSFlags(rawValue: kSecCSSigningInformation), &info) == errSecSuccess,
              let info = info as? [String: Any], let identifier = info[kSecCodeInfoIdentifier as String] as? String else { return nil }
        func meets(_ text: String) -> Bool {
            var requirement: SecRequirement?
            guard SecRequirementCreateWithString(text as CFString, [], &requirement) == errSecSuccess, let requirement else { return false }
            return satisfies(requirement)
        }
        let category: Int? = meets(developerID) ? 6 : meets(development) ? 3 : nil
        return Signature(identifier: identifier, team: info[kSecCodeInfoTeamIdentifier as String] as? String, category: category)
    }
}
