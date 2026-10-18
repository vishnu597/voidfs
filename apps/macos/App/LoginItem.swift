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
// Background Task Management keeps a job's launch requirement (its LWCR) across unregistering and
// registering. After the job's program changes, or a copy signed otherwise registers it, launchd
// refuses to start it until it is registered again (`bench/results/item7-keychain`, the live run;
// LaunchRequirement.swift). Turning it on, and opening the menu, notice that and register it
// again, up to twice.

import Foundation
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
            return await repair(why)
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
        // The extensions reach the daemon through the bridge, which a kept requirement refuses
        // as it does the job.
        var agentNote: String?
        if Agent.service.status == .notRegistered {
            try? Agent.service.register()
            agentNote = await Agent.repairIfRefused()?.note
        }
        if service.status == .requiresApproval {
            return "Allow voidfs in System Settings → General → Login Items & Extensions, and the daemon starts."
        }
        let waited = await waitForDaemon(not: before, within: startWait, daemonPid: { await daemonPid() }, refused: refused, registerAgain: registerAgain)
        var text = started(waited, before: before)
        if waited.pid != nil && hadAgent && !FileManager.default.fileExists(atPath: terminalAgent.path) {
            text += " It replaced the terminal's agent (dev.voidfs.daemon)."
        }
        if let agentNote { text += " " + agentNote }
        return text
    }

    /// What waiting for the daemon found.
    struct Wait: Equatable {
        /// The pid of the daemon that answered, if one did.
        var pid: Int?
        /// Why launchd refused the job at first, if it did.
        var refused: String?
        /// How many times the job was registered again.
        var rounds = 0
        /// Registering it again failed.
        var failed = false
        /// Why launchd refuses it still, when no daemon answered after it was registered again.
        var still: String?
    }

    /// What turning it on did, in a sentence or two.
    static func started(_ waited: Wait, before: Int?) -> String {
        var note = ""
        if let why = waited.refused {
            if waited.failed { return "launchd refuses the daemon's job (\(why)), and registering it again failed." }
            note = "launchd refused the job (\(why)), so it was registered again\(waited.rounds > 1 ? ", twice" : ""). "
        }
        guard let pid = waited.pid else {
            if let still = waited.still { return note + "launchd still refuses it (\(still)), so the daemon doesn't start." }
            return note + "Registered, but the daemon didn't answer within \(startWait): see daemon.log in \(Agent.stateDirectory?.path ?? "its state directory")."
        }
        var text = note + "The daemon (pid \(pid)) starts at login."
        if let before { text += " It took over from the daemon that was running (pid \(before))." }
        return text
    }

    /// Waits for a daemon other than `old` to answer. If launchd refuses the job meanwhile, it is
    /// registered again, up to `LaunchRequirement.rounds` times in all, counting those of `start`,
    /// and the wait starts over each time.
    static func waitForDaemon(not old: Int?, within wait: Duration, from start: Wait = Wait(), daemonPid: () async -> Int?, refused: () -> String?,
                              registerAgain: () async -> Bool) async -> Wait {
        var waited = start
        var deadline = ContinuousClock.now + wait
        var checked = ContinuousClock.now - .seconds(1)
        while ContinuousClock.now < deadline {
            if let pid = await daemonPid(), pid != old {
                waited.pid = pid
                return waited
            }
            // launchctl is a process: once a second is enough.
            if waited.rounds < LaunchRequirement.rounds, ContinuousClock.now - checked >= .seconds(1) {
                checked = ContinuousClock.now
                if let why = refused() {
                    waited.refused = waited.refused ?? why
                    waited.rounds += 1
                    guard await registerAgain() else {
                        waited.failed = true
                        return waited
                    }
                    deadline = ContinuousClock.now + wait
                }
            }
            try? await Task.sleep(for: .milliseconds(100))
        }
        if waited.refused != nil { waited.still = refused() }
        return waited
    }

    /// Why launchd refuses the job now, or `nil` if it doesn't: `launchctl print`'s view of it.
    static func refused() -> String? {
        guard let printed = LaunchRequirement.printed(label) else { return nil }
        return LaunchRequirement.refusal(in: printed, own: LaunchRequirement.own)
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
        return await repair(why)
    }

    /// Registers the job again because launchd refuses it for `why`, and waits for the daemon,
    /// registering it once more if launchd still refuses it.
    static func repair(_ why: String) async -> String {
        let before = await daemonPid()
        var waited = Wait(refused: why, rounds: 1)
        guard await registerAgain() else {
            waited.failed = true
            return started(waited, before: before)
        }
        waited = await waitForDaemon(not: before, within: startWait, from: waited, daemonPid: { await daemonPid() }, refused: refused, registerAgain: registerAgain)
        return started(waited, before: before)
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
        await now.refresh(repairing: false)
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
        // The launch's check of the bridge agent, as on 18 October.
        let agent = HostModel()
        agent.enabled = true
        agent.finderEnabled = true
        agent.login = .on
        agent.signedIn = example
        agent.agentNote = Agent.Repair(refused: "its launch requirement names validation category 3 (development), not 6 (Developer ID)", rounds: 2).note
        await draw("agent-registered-again", agent)
        return failed == 0 ? 0 : 1
    }
}

/// `voidfs login-item selftest`: the launch-refusal check against `launchctl print` as it was in
/// the Keychain slice's live run and the 18 October check, and the wait that registers the job
/// again, with stand-ins for the daemon and launchd. It registers nothing.
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

    /// A requirement as launchd prints it; without `identifier`, it names none.
    static func lwcr(_ identifier: String?, category: Int = 3) -> String {
        """
        \tLWCR = {
        \t\t"reqs" => {
        \(identifier.map { "\t\t\t\"signing-identifier\" => \"\($0)\"\n" } ?? "")\t\t\t"validation-category" => \(category)
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
        // The Keychain slice's development build, and the Developer ID copy of 18 October.
        let development = LaunchRequirement.Signature(identifier: me, team: "HAUTK68F56", category: 3)
        let developerID = LaunchRequirement.Signature(identifier: me, team: "HAUTK68F56", category: 6)
        func refusal(_ printed: String, _ own: LaunchRequirement.Signature? = development) -> String? {
            LaunchRequirement.refusal(in: printed, own: own)
        }

        // The check.
        let kept = refusal(printed(lwcr("dev.voidfs.void"), state: "spawn scheduled", exit: "78: EX_CONFIG"))
        check(kept == "its launch requirement names dev.voidfs.void, not dev.voidfs.app", "a requirement kept from the first slice's job is a refusal: \(kept ?? "none")")
        let keptOnly = refusal(printed(lwcr("dev.voidfs.void"), state: "spawn scheduled", exit: "(never exited)"))
        check(keptOnly != nil, "so is that requirement before launchd has tried the program")
        check(refusal(printed(lwcr(me))) == nil, "the job's own requirement, running, isn't")
        let config = refusal(printed(lwcr(me), state: "spawn scheduled", exit: "78: EX_CONFIG"))
        check(config == "it can't start the program: exit code 78, EX_CONFIG", "exit code 78 is, whatever the requirement: \(config ?? "none")")
        check(refusal(printed("", state: "not running", exit: "0")) == nil, "no requirement and a clean exit isn't")
        check(refusal(printed("", state: "not running", exit: "1")) == nil, "nor is the daemon's own failure (exit 1)")
        // A requirement that names no identifier, then another block that does.
        let outside = printed(lwcr(nil) + "\n\tother = {\n\t\t\"signing-identifier\" => \"dev.voidfs.void\"\n\t}", state: "spawn scheduled")
        check(refusal(outside) == nil, "a signing identifier after the requirement's block doesn't count")
        check(refusal(printed(lwcr("dev.voidfs.void"), state: "spawn scheduled"), nil) == nil, "unsigned, only exit code 78 counts")
        // On 18 October `login-item on` caught this only by exit code 78, once launchd had tried.
        let category = refusal(printed(lwcr(me), state: "spawn scheduled"), developerID)
        check(category == "its launch requirement names validation category 3 (development), not 6 (Developer ID)",
              "a development build's requirement kept for the Developer ID copy is, before launchd has tried the program: \(category ?? "none")")
        check(refusal(printed(lwcr(me, category: 6), state: "spawn scheduled"), developerID) == nil, "the Developer ID copy's own requirement isn't")
        check(refusal(printed(lwcr("dev.voidfs.void"), exit: "78: EX_CONFIG")) == nil, "a job launchd runs isn't, whatever its requirement or last exit")
        let own = LaunchRequirement.own
        check(own?.identifier == me, "this binary's signing identifier is the job's program's: \(own?.identifier ?? "none")")

        // The wait, with stand-ins.
        do {
            var registered = 0, polls = 0
            let waited = await LoginItem.waitForDaemon(not: nil, within: .seconds(2), daemonPid: {
                polls += 1
                return registered > 0 && polls > 3 ? 4242 : nil
            }, refused: { registered == 0 ? "its launch requirement names dev.voidfs.void, not dev.voidfs.app" : nil }, registerAgain: {
                registered += 1
                return true
            })
            check(waited.pid == 4242 && waited.refused != nil && waited.rounds == 1 && registered == 1, "refused, it is registered again once and the daemon answers (pid \(waited.pid.map(String.init) ?? "none"), registered \(registered))")
        }
        do {
            // As the bridge agent was on 18 October: kept after one round, cleared by a second.
            let states = [printed(lwcr(me), state: "spawn scheduled"), printed(lwcr(me), state: "spawn scheduled", exit: "78: EX_CONFIG"), printed(lwcr(me, category: 6))]
            var registered = 0, polls = 0
            let waited = await LoginItem.waitForDaemon(not: nil, within: .seconds(5), daemonPid: {
                polls += 1
                return registered == 2 && polls > 25 ? 4242 : nil
            }, refused: { refusal(states[registered], developerID) }, registerAgain: {
                registered += 1
                return true
            })
            check(waited.pid == 4242 && waited.rounds == 2 && registered == 2 && waited.refused == category, "kept after one round, it is registered again a second time and the daemon answers (registered \(registered))")
            let note = LoginItem.started(waited, before: nil)
            check(note.hasPrefix("launchd refused the job (its launch requirement names validation category 3 (development), not 6 (Developer ID)), so it was registered again, twice. The daemon (pid 4242) starts at login."), "the note says so: \(note)")
        }
        do {
            // Refused from the second check, a second in, and from then on.
            var registered = 0, checks = 0
            let wait: Duration = .milliseconds(1500)
            let start = ContinuousClock.now
            let waited = await LoginItem.waitForDaemon(not: nil, within: wait, daemonPid: { nil }, refused: {
                checks += 1
                return checks == 1 ? nil : "it can't start the program: exit code 78, EX_CONFIG"
            }, registerAgain: {
                // A third time would wait for ever: fail it, so that the check below sees it.
                registered += 1
                return registered <= LaunchRequirement.rounds
            })
            let took = ContinuousClock.now - start
            check(waited.pid == nil && waited.rounds == 2 && registered == 2 && !waited.failed && took > wait + .milliseconds(1800),
                  "refused still, it is registered again only twice, and the wait starts over each time (\(took))")
            let note = LoginItem.started(waited, before: nil)
            check(note.hasSuffix("launchd still refuses it (it can't start the program: exit code 78, EX_CONFIG), so the daemon doesn't start."), "and it says launchd still refuses it: \(note)")
        }
        do {
            // Turning it on while it's on, and the menu, register it once before the wait.
            var registered = 0
            let first = "its launch requirement names dev.voidfs.void, not dev.voidfs.app"
            let waited = await LoginItem.waitForDaemon(not: nil, within: .milliseconds(1200), from: LoginItem.Wait(refused: first, rounds: 1), daemonPid: { nil },
                                                       refused: { "it can't start the program: exit code 78, EX_CONFIG" }, registerAgain: {
                registered += 1
                return registered == 1
            })
            check(registered == 1 && waited.rounds == 2 && waited.refused == first && waited.still != nil, "after a round of their own, the wait registers it once more, and keeps the first reason")
        }
        do {
            var registered = 0, checks = 0, polls = 0
            let waited = await LoginItem.waitForDaemon(not: 4141, within: .seconds(5), daemonPid: {
                polls += 1
                return polls < 15 ? 4141 : 4242
            }, refused: {
                checks += 1
                return nil
            }, registerAgain: {
                registered += 1
                return true
            })
            check(waited.pid == 4242 && waited.refused == nil && registered == 0, "not refused, it waits past the old daemon for the new one and registers nothing again")
            check(checks >= 1 && checks <= 3, "and asks launchctl about once a second (\(checks) times in \(polls) polls)")
        }
        do {
            var registered = 0
            let start = ContinuousClock.now
            let waited = await LoginItem.waitForDaemon(not: nil, within: .seconds(5), daemonPid: { nil }, refused: { "it can't start the program: exit code 78, EX_CONFIG" }, registerAgain: {
                registered += 1
                return false
            })
            check(waited.pid == nil && waited.failed && registered == 1 && ContinuousClock.now - start < .seconds(1), "if registering again fails, it says so at once")
            check(LoginItem.started(waited, before: nil) == "launchd refuses the daemon's job (it can't start the program: exit code 78, EX_CONFIG), and registering it again failed.", "with the reason")
        }
        let note = LoginItem.started(LoginItem.Wait(pid: 4242, refused: "its launch requirement names dev.voidfs.void, not dev.voidfs.app", rounds: 1), before: nil)
        check(note.hasPrefix("launchd refused the job (its launch requirement names dev.voidfs.void, not dev.voidfs.app), so it was registered again. The daemon (pid 4242) starts at login."), "the note says why: \(note)")

        // This Mac, read only.
        print("this binary: \(AgentSelfTest.describe(own))")
        print("this Mac: the job is \(LoginItem.describe); launchd \(LoginItem.refused().map { "refuses it: \($0)" } ?? "doesn't refuse it")")
        print(failed == 0 ? "login item self-test passed" : "login item self-test: \(failed) failed")
        return failed == 0 ? 0 : 1
    }
}
