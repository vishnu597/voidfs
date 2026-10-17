// SPDX-License-Identifier: Apache-2.0
//
// Start at login (step 5, item 7): the daemon, from the app's own copy of `void`
// (`Contents/MacOS/void`, which Xcode's "Bundle void" phase builds), registered with SMAppService
// as the job in `Contents/Library/LaunchAgents/dev.voidfs.app.daemon.plist`. The job runs this
// binary, which hands the daemon the server's key from the Keychain (ServerKey.swift, SignIn.swift).
// The app takes over:
// once registered, its daemon removes the agent `void daemon install` wrote for the same state
// directory and stops a daemon started otherwise, and `void daemon install` refuses while the job
// is loaded. One daemon, started one way, owns the state directory. Turning it off stops the
// daemon, as switching it off in Login Items does.
//
// Background Task Management keeps a job's launch requirement (its LWCR) once across
// unregistering and registering. After the job's program changes, or a copy signed otherwise
// registers it, launchd refuses to start it until it is registered again
// (`bench/results/item7-keychain`, the live run). Turning it on, and opening the menu, notice that
// and register it once more.

import Foundation
import Security
import ServiceManagement
import SwiftUI

enum LoginItem {
    static let service = SMAppService.agent(plistName: "dev.voidfs.app.daemon.plist")
    /// How long turning it on waits for the daemon to answer.
    static let startWait: Duration = .seconds(20)

    enum State: Equatable {
        /// Registered: the daemon starts at login.
        case on
        case off
        /// Registered, but switched off in System Settings → General → Login Items & Extensions.
        case needsApproval
        /// This copy of the app has no job or no `void`: built without the "Bundle void" phase.
        case notBundled
        /// Off, and the app has no server and key for the daemon: not signed in.
        case notConfigured
    }

    static var state: State {
        switch service.status {
        case .enabled: .on
        case .requiresApproval: .needsApproval
        // SMAppService says notFound for a job never registered, as well as for one it can't find.
        case .notRegistered, .notFound: !bundled ? .notBundled : configured ? .off : .notConfigured
        @unknown default: .off
        }
    }

    /// The job's plist and the `void` it starts are in this copy of the app.
    static var bundled: Bool {
        let contents = Bundle.main.bundleURL.appending(path: "Contents")
        return FileManager.default.fileExists(atPath: contents.appending(path: "Library/LaunchAgents/\(label).plist").path)
            && FileManager.default.isExecutableFile(atPath: contents.appending(path: "MacOS/void").path)
    }

    static var describe: String {
        switch state {
        case .on: "on: the daemon starts at login (\(label))"
        case .off: "off"
        case .needsApproval: "needs approval in System Settings → General → Login Items & Extensions"
        case .notBundled: "not in this copy of the app (built without the Bundle void phase)"
        case .notConfigured: "off: the daemon has no server and key yet"
        }
    }

    static let label = "dev.voidfs.app.daemon"
    static let turnedOffNote = "The daemon stopped and doesn't start at login. `void daemon start` starts it until you log out."
    static let notConfiguredHelp = "Sign in to your voidfs server first: the daemon starts with that key."

    /// The daemon has a server and key to start with: the Keychain's, or a `daemon.json` the
    /// terminal kept, which moves into the Keychain when the daemon starts.
    static var configured: Bool {
        guard let dir = Agent.stateDirectory else { return false }
        return (try? ServerKeys.load(dir)) != nil || FileManager.default.fileExists(atPath: ServerKeys.file(dir).path)
    }

    /// The agent `void daemon install` writes, which the app's daemon replaces.
    static var terminalAgent: URL {
        FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/LaunchAgents/dev.voidfs.daemon.plist")
    }

    /// Registers the daemon's job, and the bridge's if it isn't registered, and waits for the
    /// daemon to answer. What happened, in a sentence or two.
    static func turnOn() async -> String {
        if service.status == .enabled {
            guard let why = refused() else { return "The daemon already starts at login." }
            let before = await daemonPid()
            guard await registerAgain() else { return "launchd refuses the daemon's job (\(why)), and registering it again failed." }
            return started(await daemonPid(within: startWait, not: before), before: before, again: why)
        }
        guard configured else { return notConfiguredHelp }
        let hadAgent = FileManager.default.fileExists(atPath: terminalAgent.path)
        // A daemon running now makes way for the app's.
        let before = await daemonPid()
        do {
            try service.register()
        } catch {
            if service.status != .requiresApproval { return "Couldn't register the daemon: \(error.localizedDescription)" }
        }
        // The extensions reach the daemon through the bridge.
        if Agent.service.status == .notRegistered { try? Agent.service.register() }
        if service.status == .requiresApproval {
            return "Allow voidfs in System Settings → General → Login Items & Extensions, and the daemon starts."
        }
        let (pid, again) = await waitForDaemon(not: before, within: startWait, daemonPid: { await daemonPid() }, refused: refused, registerAgain: registerAgain)
        var text = started(pid, before: before, again: again)
        if pid != nil && hadAgent && !FileManager.default.fileExists(atPath: terminalAgent.path) {
            text += " It replaced the terminal's agent (dev.voidfs.daemon)."
        }
        return text
    }

    /// What turning it on did, in a sentence or two.
    static func started(_ pid: Int?, before: Int?, again: String?) -> String {
        let note = again.map { "launchd refused the job (\($0)), so it was registered again. " } ?? ""
        guard let pid else {
            return note + "Registered, but the daemon didn't answer within \(startWait): see daemon.log in \(Agent.stateDirectory?.path ?? "its state directory")."
        }
        var text = note + "The daemon (pid \(pid)) starts at login."
        if let before { text += " It took over from the daemon that was running (pid \(before))." }
        return text
    }

    /// Waits for a daemon other than `old` to answer. If launchd refuses the job meanwhile, it is
    /// registered again, once, and the wait starts over. The daemon's pid, if one answered, and
    /// why the job was registered again, if it was.
    static func waitForDaemon(not old: Int?, within wait: Duration, daemonPid: () async -> Int?, refused: () -> String?,
                              registerAgain: () async -> Bool) async -> (Int?, String?) {
        var deadline = ContinuousClock.now + wait
        var again: String?
        var checked = ContinuousClock.now - .seconds(1)
        while ContinuousClock.now < deadline {
            if let pid = await daemonPid(), pid != old { return (pid, again) }
            // launchctl is a process: once a second is enough.
            if again == nil, ContinuousClock.now - checked >= .seconds(1) {
                checked = ContinuousClock.now
                if let why = refused() {
                    again = why
                    guard await registerAgain() else { return (nil, why) }
                    deadline = ContinuousClock.now + wait
                }
            }
            try? await Task.sleep(for: .milliseconds(100))
        }
        return (nil, again)
    }

    /// Why launchd refuses the job now, or `nil` if it doesn't: `launchctl print`'s view of it.
    static func refused() -> String? {
        let (status, printed) = SignIn.launchctl(["print", "gui/\(getuid())/\(label)"])
        guard status == 0 else { return nil }
        return refusal(in: printed, identifier: ownIdentifier)
    }

    /// The launch refusal `launchctl print` shows for a job: a requirement (LWCR) that names
    /// another signing identifier than `identifier`, the job's program's, or a last exit code of
    /// 78 (EX_CONFIG), which launchd gives a job it can't start. Neither `void` nor this binary
    /// exits 78.
    static func refusal(in printed: String, identifier: String?) -> String? {
        let lines = printed.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        func tabs(_ line: String) -> Int { line.prefix { $0 == "\t" }.count }
        if let identifier, let start = lines.firstIndex(where: { $0.trimmingCharacters(in: .whitespaces) == "LWCR = {" }) {
            let depth = tabs(lines[start])
            for line in lines[(start + 1)...] {
                let t = line.trimmingCharacters(in: .whitespaces)
                if t == "}" && tabs(line) == depth { break }
                let key = "\"signing-identifier\" => \""
                if t.hasPrefix(key), t.hasSuffix("\"") {
                    let named = String(t.dropFirst(key.count).dropLast())
                    if named != identifier { return "its launch requirement names \(named), not \(identifier)" }
                }
            }
        }
        if lines.contains(where: { $0.trimmingCharacters(in: .whitespaces).hasPrefix("last exit code = 78") }) {
            return "it can't start the program: exit code 78, EX_CONFIG"
        }
        return nil
    }

    /// This binary's signing identifier: the job's program's, since the job runs it.
    static var ownIdentifier: String? {
        var code: SecCode?
        var staticCode: SecStaticCode?
        var info: CFDictionary?
        guard SecCodeCopySelf([], &code) == errSecSuccess, let code,
              SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode,
              SecCodeCopySigningInformation(staticCode, [], &info) == errSecSuccess else { return nil }
        return (info as? [String: Any])?[kSecCodeInfoIdentifier as String] as? String
    }

    /// Unregisters the job and registers it again, which gives it a new launch requirement.
    static func registerAgain() async -> Bool {
        try? await service.unregister()
        do {
            try service.register()
        } catch {
            return service.status == .requiresApproval
        }
        return true
    }

    /// Once per run of the app, while the job is on: registers it again if launchd refuses it,
    /// as after an update that changed its program. What happened, or `nil` if nothing did.
    @MainActor static var repaired = false
    @MainActor
    static func repairIfRefused() async -> String? {
        guard !repaired, service.status == .enabled else { return nil }
        repaired = true
        guard let why = refused() else { return nil }
        let before = await daemonPid()
        guard await registerAgain() else { return "launchd refuses the daemon's job (\(why)), and registering it again failed." }
        return started(await daemonPid(within: startWait, not: before), before: before, again: why)
    }

    /// Unregisters the job, which stops the daemon.
    static func turnOff() async -> String {
        do {
            try await service.unregister()
        } catch {
            return "Couldn't unregister the daemon: \(error.localizedDescription)"
        }
        return turnedOffNote
    }

    /// The pid of the daemon that answers on the socket, if one does.
    static func daemonPid() async -> Int? {
        guard let dir = Agent.stateDirectory else { return nil }
        let socket = DaemonSocket(path: dir.appending(path: "daemon.sock").path)
        guard let r = try? await socket.request("GET", "/v1/status", generation: nil, limit: 1 << 20), r.status == 200,
              let o = (try? JSONSerialization.jsonObject(with: r.body)) as? [String: Any] else { return nil }
        return (o["daemon"] as? [String: Any])?["pid"] as? Int
    }

    /// The pid of a daemon other than `old` once one answers.
    static func daemonPid(within wait: Duration, not old: Int?) async -> Int? {
        let deadline = ContinuousClock.now + wait
        while ContinuousClock.now < deadline {
            if let pid = await daemonPid(), pid != old { return pid }
            try? await Task.sleep(for: .milliseconds(100))
        }
        return nil
    }

    /// `login-item look <folder>`: draws the menu's setup rows, offscreen, in each state, with
    /// what this Mac says now first. Registers nothing.
    @MainActor
    static func look(_ args: [String]) async -> Int32 {
        guard args.count == 1 else {
            print("usage: login-item look <folder>")
            return 2
        }
        let dir = URL(filePath: args[0])
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let now = HostModel()
        await now.refresh()
        print("now: login item \(describe); file system extension enabled=\(now.enabled.map(String.init) ?? "not installed"); Finder extension enabled=\(now.finderEnabled.map(String.init) ?? "unknown"); signed in: \(now.signedIn != nil)")
        var failed = 0
        func draw(_ name: String, _ model: HostModel) async {
            let path = dir.appending(path: "\(name).png").path
            let ok = await TransfersSelfTest.draw(SetupView(model: model), to: path)
            print("\(ok ? "drew" : "FAILED to draw") \(path)")
            if !ok { failed += 1 }
        }
        await draw("now", now)
        let notes: [(String, State, String)] = [
            ("on", .on, "The daemon (pid 4242) starts at login. It replaced the terminal's agent (dev.voidfs.daemon)."),
            ("off", .off, turnedOffNote),
            ("needs-approval", .needsApproval, ""),
            ("not-bundled", .notBundled, ""),
            ("not-configured", .notConfigured, ""),
        ]
        let example = SignIn.Summary(endpoint: "https://voidfs.example.com", region: ServerKey.defaultRegion, accessKeyId: "VFEXAMPLE0000000000")
        for (i, (name, state, note)) in notes.enumerated() {
            let m = HostModel()
            m.enabled = i != 1
            m.finderEnabled = i != 2
            m.login = state
            m.loginNote = note
            m.signedIn = state == .notConfigured ? nil : example
            await draw(name, m)
        }
        // Signing in: the form, filled in with a secret it doesn't show, and a refusal.
        let form = HostModel()
        form.enabled = true
        form.finderEnabled = true
        form.login = .notConfigured
        form.signedIn = nil
        form.signingIn = true
        form.form = SignInFields(endpoint: "https://voidfs.example.com", region: "", accessKeyId: "VFEXAMPLE0000000000", secretAccessKey: "not-a-real-secret")
        form.keyNote = "The server didn't accept it: 403 InvalidAccessKeyId: the access key isn't known"
        await draw("sign-in", form)
        let signedIn = HostModel()
        signedIn.enabled = true
        signedIn.finderEnabled = true
        signedIn.login = .on
        signedIn.signedIn = example
        signedIn.keyNote = "Signed in to https://voidfs.example.com as VFEXAMPLE0000000000. The daemon (pid 4242) runs with it."
        await draw("signed-in", signedIn)
        return failed == 0 ? 0 : 1
    }
}

/// `voidfs login-item selftest`: the launch-refusal check against `launchctl print` as it was in
/// the Keychain slice's live run, and the wait that registers the job again, with stand-ins for the
/// daemon and launchd. It registers nothing.
enum LoginItemSelfTest {
    /// The job as `launchctl print` showed it, with `lwcr` in place of its requirement.
    static func printed(_ lwcr: String, state: String = "running", exit: String = "(never exited)") -> String {
        """
        gui/501/dev.voidfs.app.daemon = {
        \tactive count = 1
        \tpath = (submitted by smd.340)
        \tstate = \(state)

        \tprogram identifier = Contents/MacOS/voidfs (mode: 2)
        \tparent bundle identifier = dev.voidfs.app
        \targuments = {
        \t\tvoidfs
        \t\tdaemon-launch
        \t}
        \(lwcr)
        \tdomain = gui/501 [100020]
        \truns = 1
        \tlast exit code = \(exit)
        }
        """
    }

    static func lwcr(_ identifier: String) -> String {
        """
        \tLWCR = {
        \t\t"reqs" => {
        \t\t\t"signing-identifier" => "\(identifier)"
        \t\t\t"validation-category" => 3
        \t\t\t"team-identifier" => "HAUTK68F56"
        \t\t}
        \t\t"vers" => 1
        \t\t"comp" => 1
        \t\t"ccat" => 0
        \t}
        """
    }

    static func run() async -> Int32 {
        var failed = 0
        func check(_ ok: Bool, _ what: String) {
            print("\(ok ? "ok  " : "FAIL") \(what)")
            if !ok { failed += 1 }
        }
        let me = "dev.voidfs.app"

        // The check.
        let kept = LoginItem.refusal(in: printed(lwcr("dev.voidfs.void"), state: "spawn scheduled", exit: "78: EX_CONFIG"), identifier: me)
        check(kept == "its launch requirement names dev.voidfs.void, not dev.voidfs.app", "a requirement kept from the first slice's job is a refusal: \(kept ?? "none")")
        let keptOnly = LoginItem.refusal(in: printed(lwcr("dev.voidfs.void"), state: "spawn scheduled", exit: "(never exited)"), identifier: me)
        check(keptOnly != nil, "so is that requirement before launchd has tried the program")
        check(LoginItem.refusal(in: printed(lwcr(me)), identifier: me) == nil, "the job's own requirement, running, isn't")
        let config = LoginItem.refusal(in: printed(lwcr(me), state: "spawn scheduled", exit: "78: EX_CONFIG"), identifier: me)
        check(config == "it can't start the program: exit code 78, EX_CONFIG", "exit code 78 is, whatever the requirement: \(config ?? "none")")
        check(LoginItem.refusal(in: printed("", state: "not running", exit: "0"), identifier: me) == nil, "no requirement and a clean exit isn't")
        check(LoginItem.refusal(in: printed("", state: "not running", exit: "1"), identifier: me) == nil, "nor is the daemon's own failure (exit 1)")
        let outside = printed(lwcr(me) + "\n\tother = {\n\t\t\"signing-identifier\" => \"dev.voidfs.void\"\n\t}")
        check(LoginItem.refusal(in: outside, identifier: me) == nil, "a signing identifier after the requirement's block doesn't count")
        check(LoginItem.refusal(in: printed(lwcr("dev.voidfs.void")), identifier: nil) == nil, "unsigned, only exit code 78 counts")
        let own = LoginItem.ownIdentifier
        check(own == me, "this binary's signing identifier is the job's program's: \(own ?? "none")")

        // The wait, with stand-ins.
        do {
            var registered = 0, polls = 0
            let (pid, again) = await LoginItem.waitForDaemon(not: nil, within: .seconds(2), daemonPid: {
                polls += 1
                return registered > 0 && polls > 3 ? 4242 : nil
            }, refused: { registered == 0 ? "its launch requirement names dev.voidfs.void, not dev.voidfs.app" : nil }, registerAgain: {
                registered += 1
                return true
            })
            check(pid == 4242 && again != nil && registered == 1, "refused, it is registered again once and the daemon answers (pid \(pid.map(String.init) ?? "none"), registered \(registered))")
        }
        do {
            // Refused from the second check, a second in, and from then on.
            var registered = 0, checks = 0
            let wait: Duration = .milliseconds(1500)
            let start = ContinuousClock.now
            let (pid, again) = await LoginItem.waitForDaemon(not: nil, within: wait, daemonPid: { nil }, refused: {
                checks += 1
                return checks == 1 ? nil : "it can't start the program: exit code 78, EX_CONFIG"
            }, registerAgain: {
                // A second time would wait for ever: fail it, so that the check below sees it.
                registered += 1
                return registered == 1
            })
            let took = ContinuousClock.now - start
            check(pid == nil && again != nil && registered == 1 && took > wait + .milliseconds(800), "refused still, it is registered again only once, and the wait starts over (\(took))")
        }
        do {
            var registered = 0, checks = 0, polls = 0
            let (pid, again) = await LoginItem.waitForDaemon(not: 4141, within: .seconds(5), daemonPid: {
                polls += 1
                return polls < 15 ? 4141 : 4242
            }, refused: {
                checks += 1
                return nil
            }, registerAgain: {
                registered += 1
                return true
            })
            check(pid == 4242 && again == nil && registered == 0, "not refused, it waits past the old daemon for the new one and registers nothing again")
            check(checks >= 1 && checks <= 3, "and asks launchctl about once a second (\(checks) times in \(polls) polls)")
        }
        do {
            var registered = 0
            let start = ContinuousClock.now
            let (pid, again) = await LoginItem.waitForDaemon(not: nil, within: .seconds(5), daemonPid: { nil }, refused: { "it can't start the program: exit code 78, EX_CONFIG" }, registerAgain: {
                registered += 1
                return false
            })
            check(pid == nil && again != nil && registered == 1 && ContinuousClock.now - start < .seconds(1), "if registering again fails, it says so at once")
        }
        let note = LoginItem.started(4242, before: nil, again: "its launch requirement names dev.voidfs.void, not dev.voidfs.app")
        check(note.hasPrefix("launchd refused the job (its launch requirement names dev.voidfs.void, not dev.voidfs.app), so it was registered again. The daemon (pid 4242) starts at login."), "the note says why: \(note)")

        // This Mac, read only.
        print("this Mac: the job is \(LoginItem.describe); launchd \(LoginItem.refused().map { "refuses it: \($0)" } ?? "doesn't refuse it")")
        print(failed == 0 ? "login item self-test passed" : "login item self-test: \(failed) failed")
        return failed == 0 ? 0 : 1
    }
}
