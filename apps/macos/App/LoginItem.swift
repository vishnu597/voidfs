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
        if service.status == .enabled { return "The daemon already starts at login." }
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
        guard let pid = await daemonPid(within: startWait, not: before) else {
            return "Registered, but the daemon didn't answer within \(startWait): see daemon.log in \(Agent.stateDirectory?.path ?? "its state directory")."
        }
        var text = "The daemon (pid \(pid)) starts at login."
        if let before { text += " It took over from the daemon that was running (pid \(before))." }
        if hadAgent && !FileManager.default.fileExists(atPath: terminalAgent.path) {
            text += " It replaced the terminal's agent (dev.voidfs.daemon)."
        }
        return text
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
