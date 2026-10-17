// SPDX-License-Identifier: Apache-2.0
//
// voidfs.app: the menu-bar host FSKit requires. It shows whether the extensions are enabled and
// whether the daemon starts at login (LoginItem.swift), the daemon's transfers with their controls
// (Transfers.swift), and asks the daemon to mount drives, which it does with the extension (the
// `fskit` adapter) and remembers. You sign in to the server from the menu: the app keeps the
// server and key in the Keychain and hands them to the daemon it starts at login (ServerKey.swift,
// SignIn.swift). The extensions hold none.
//
// It also runs headless, for scripts:
//
//     voidfs.app/Contents/MacOS/voidfs status
//     voidfs.app/Contents/MacOS/voidfs transfers
//     voidfs.app/Contents/MacOS/voidfs transfers-selftest <drive> [--cancel] [--png <path>]
//     voidfs.app/Contents/MacOS/voidfs mount <drive> [mount-point]
//     voidfs.app/Contents/MacOS/voidfs settings
//     voidfs.app/Contents/MacOS/voidfs agent-register | agent-unregister | agent-status
//     voidfs.app/Contents/MacOS/voidfs login-item on | off | status | look <folder> | selftest
//     voidfs.app/Contents/MacOS/voidfs server-key status | sign-in … | sign-out | selftest
//     voidfs.app/Contents/MacOS/voidfs bridge-check
//     voidfs.app/Contents/MacOS/voidfs bridge-selftest <drive> [main|daemon-restart|agent-restart|remote <key>|refused|wire] [--socket <path>]
//     voidfs.app/Contents/MacOS/voidfs badges-selftest <drive> [main|daemon-restart|refused] [--socket <path>] [--peer <path>] [--metrics <url>] [--files <n>] [--png <path>]
//     voidfs.app/Contents/MacOS/voidfs badges-check
//
// `agent` is how launchd runs it as the extensions' bridge to the daemon (Agent.swift, BadgeAgent.swift).
// `daemon-launch` is how launchd runs it to start the daemon at login, from the app's copy of
// `void`, with the Keychain's key (LoginItem.swift, ServerKey.swift).

import FSKit
import FinderSync
import ServiceManagement
import SwiftUI

let extensionBundleID = "dev.voidfs.app.fskit"

@main
enum Launcher {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        if args.first == "daemon-launch" { DaemonLaunch.run() }
        if args.first == "agent" { Agent.serve() }
        if args.first == "login-item", args.dropFirst().first == "look" {
            // Drawing needs AppKit's main thread, as the transfers self-test's does.
            Task { @MainActor in exit(await LoginItem.look(Array(args.dropFirst(2)))) }
            NSApplication.shared.setActivationPolicy(.prohibited)
            NSApplication.shared.run()
        }
        if args.first == "transfers-selftest" {
            // It draws with AppKit, which needs the main thread that dispatchMain() gives up.
            Task { @MainActor in exit(await TransfersSelfTest.run(Array(args.dropFirst()))) }
            NSApplication.shared.setActivationPolicy(.prohibited)
            NSApplication.shared.run()
        }
        if let command = args.first, ["status", "transfers", "mount", "settings", "agent-register", "agent-unregister", "agent-status", "login-item", "server-key", "bridge-check", "bridge-selftest", "badges-selftest", "badges-check"].contains(command) {
            Task {
                let code = await HostCommands.run(args)
                exit(code)
            }
            dispatchMain()
        }
        VoidfsApp.main()
    }
}

enum HostCommands {
    static func run(_ args: [String]) async -> Int32 {
        do {
            switch args[0] {
            case "status":
                let modules = try await FSClient.shared.installedExtensions
                for m in modules {
                    print("\(m.bundleIdentifier)\tenabled=\(m.isEnabled)\t\(m.url.path)")
                }
                if !modules.contains(where: { $0.bundleIdentifier == extensionBundleID }) {
                    print("\(extensionBundleID) is not installed")
                    return 1
                }
            case "transfers":
                // The menu's transfers as text, laid out like `void status`.
                let (text, code) = await Transfers.report()
                print(text)
                return code
            case "mount":
                guard args.count == 2 || args.count == 3 else {
                    print("usage: mount <drive> [mount-point]")
                    return 2
                }
                let start = ContinuousClock.now
                let path = try await Mounter.mount(args[1], at: args.count > 2 ? URL(filePath: args[2]).standardizedFileURL.path : nil)
                print("mounted at \(path) in \(ContinuousClock.now - start)")
            case "settings":
                print(FSClient.shared.openFileSystemExtensionsSettings() ? "opened System Settings" : "could not open System Settings")
            case "agent-register":
                do { try Agent.service.register() } catch { print("register: \(error.localizedDescription)") }
                print("agent: \(Agent.status)")
                return Agent.service.status == .enabled ? 0 : 1
            case "agent-unregister":
                try await Agent.service.unregister()
                print("agent: \(Agent.status)")
            case "agent-status":
                print("agent: \(Agent.status)")
            case "login-item":
                switch args.dropFirst().first {
                case "on":
                    print(await LoginItem.turnOn())
                    print("login item: \(LoginItem.describe)")
                    return LoginItem.state == .on ? 0 : 1
                case "off":
                    print(await LoginItem.turnOff())
                    print("login item: \(LoginItem.describe)")
                case "status", nil:
                    print("login item: \(LoginItem.describe)")
                    if LoginItem.service.status == .enabled, let why = LoginItem.refused() {
                        print("launchd refuses the job: \(why); `login-item on` registers it again")
                    }
                case "selftest":
                    return await LoginItemSelfTest.run()
                default:
                    print("usage: login-item on | off | status | look <folder> | selftest")
                    return 2
                }
            case "server-key":
                return await SignIn.command(Array(args.dropFirst()))
            case "bridge-check":
                // The installed agent must refuse anything but the extension, this app included.
                let client = BridgeClient()
                do {
                    _ = try await client.open(drive: "check", readOnly: true, watch: false)
                    print("bridge-check: the agent accepted the app: FAIL")
                    return 1
                } catch {
                    let e = error as NSError
                    print("bridge-check: refused (\(e.domain) \(e.code)): ok")
                }
            case "bridge-selftest":
                return await SelfTest.run(Array(args.dropFirst()))
            case "badges-selftest":
                return await BadgesSelfTest.run(Array(args.dropFirst()))
            case "badges-check":
                // The installed agent's badge service must refuse anything but the Finder extension.
                let client = BadgeClient()
                do {
                    _ = try await client.mounts()
                    withExtendedLifetime(client) {}
                    print("badges-check: the agent accepted the app: FAIL")
                    return 1
                } catch {
                    let e = error as NSError
                    print("badges-check: refused (\(e.domain) \(e.code)): ok")
                }
            default:
                return 2
            }
            return 0
        } catch {
            let e = error as NSError
            print("error: \(e.domain) \(e.code): \(e.localizedDescription) \(e.userInfo)")
            return 1
        }
    }
}

enum Mounter {
    /// Asks the daemon to mount `drive`, as `void mount` does: it mounts with the extension (the
    /// `fskit` adapter) into `~/voidfs/<drive>` unless told otherwise, and mounts it again when it
    /// starts. Returns the mountpoint.
    static func mount(_ drive: String, at mountPoint: String? = nil) async throws -> String {
        guard let dir = Agent.stateDirectory else { throw NSError(domain: "dev.voidfs", code: 1, userInfo: [NSLocalizedDescriptionKey: "no home directory"]) }
        var request: [String: Any] = ["drive": drive]
        if let mountPoint { request["mountpoint"] = mountPoint }
        let socket = DaemonSocket(path: dir.appending(path: "daemon.sock").path)
        let response: DaemonResponse
        do {
            response = try await socket.request("POST", "/v1/mounts", generation: nil, contentType: "application/json", body: try JSONSerialization.data(withJSONObject: request), limit: 1 << 20)
        } catch DaemonSocketError.notRunning {
            throw NSError(domain: "dev.voidfs", code: 1, userInfo: [NSLocalizedDescriptionKey: "the voidfs daemon isn't running: start it with `void daemon start`"])
        }
        let o = (try? JSONSerialization.jsonObject(with: response.body)) as? [String: Any]
        guard (200..<300).contains(response.status), let path = o?["mountpoint"] as? String else {
            let message = (o?["error"] as? [String: Any])?["message"] as? String ?? "the daemon answered \(response.status)"
            throw NSError(domain: "dev.voidfs", code: response.status, userInfo: [NSLocalizedDescriptionKey: message])
        }
        return path
    }
}

struct VoidfsApp: App {
    @State private var model = HostModel()
    @State private var transfers: Transfers

    init() {
        let transfers = Transfers()
        transfers.start()
        _transfers = State(initialValue: transfers)
    }

    var body: some Scene {
        MenuBarExtra {
            HostView(model: model, transfers: transfers)
        } label: {
            Image(systemName: transfers.symbol)
        }
        .menuBarExtraStyle(.window)
    }
}

@Observable
@MainActor
final class HostModel {
    var drive = ""
    var status = ""
    var enabled: Bool?
    var finderEnabled: Bool?
    var login = LoginItem.state
    var loginNote = ""
    var loginBusy = false
    var signedIn = SignIn.summary()
    var signingIn = false
    var form = SignInFields()
    var keyNote = ""
    var keyBusy = false

    func refresh() async {
        signedIn = SignIn.summary()
        login = LoginItem.state
        finderEnabled = FIFinderSyncController.isExtensionEnabled
        do {
            let modules = try await FSClient.shared.installedExtensions
            enabled = modules.first { $0.bundleIdentifier == extensionBundleID }?.isEnabled
            status = enabled == nil ? "The file system extension is not installed." : ""
        } catch {
            status = "Could not list file system extensions: \(error.localizedDescription)"
        }
        // After an update that changed the job's program, launchd refuses it until it is
        // registered again.
        if let note = await LoginItem.repairIfRefused() {
            loginNote = note
            login = LoginItem.state
        }
    }

    func setLogin(_ on: Bool) async {
        loginBusy = true
        loginNote = on ? "Starting the daemon…" : "Stopping the daemon…"
        loginNote = on ? await LoginItem.turnOn() : await LoginItem.turnOff()
        login = LoginItem.state
        loginBusy = false
    }

    func signIn() async {
        switch SignIn.key(endpoint: form.endpoint, region: form.region, accessKeyId: form.accessKeyId, secretAccessKey: form.secretAccessKey) {
        case .failure(let f):
            keyNote = f.message
        case .success(let key):
            keyBusy = true
            keyNote = "Checking the server…"
            let (kept, note) = await SignIn.signIn(key)
            keyNote = note
            if kept {
                signingIn = false
                form = SignInFields()
            }
            signedIn = SignIn.summary()
            login = LoginItem.state
            keyBusy = false
        }
    }

    func signOut() async {
        keyBusy = true
        keyNote = await SignIn.signOut()
        signedIn = SignIn.summary()
        login = LoginItem.state
        keyBusy = false
    }

    /// Fills the form from a key file, as `void keys generate` writes them.
    func importKeyFile() {
        let panel = NSOpenPanel()
        panel.message = "A key file from `void keys generate --format json` or `--format env`"
        panel.canChooseDirectories = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        guard let text = try? String(contentsOf: url, encoding: .utf8), let found = SignIn.keyFile(text) else {
            keyNote = "\(url.lastPathComponent) isn't a key file: it needs an access key id and secret."
            return
        }
        form.accessKeyId = found["VOIDFS_ACCESS_KEY_ID"] ?? ""
        form.secretAccessKey = found["VOIDFS_SECRET_ACCESS_KEY"] ?? ""
        if let e = found["VOIDFS_ENDPOINT"] { form.endpoint = e }
        if let r = found["VOIDFS_REGION"] { form.region = r }
        keyNote = "Read the key from \(url.lastPathComponent)."
    }

    func mount() async {
        let drive = drive.trimmingCharacters(in: .whitespaces)
        guard !drive.isEmpty else { status = "Name a drive"; return }
        do {
            status = "Mounting…"
            let path = try await Mounter.mount(drive)
            status = "Mounted at \(path)"
            NSWorkspace.shared.open(URL(filePath: path))
        } catch {
            status = "Mount failed: \(error.localizedDescription)"
        }
    }
}

struct HostView: View {
    @Bindable var model: HostModel
    let transfers: Transfers

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("voidfs").font(.headline)
            SetupView(model: model)
            TransfersView(transfers: transfers)
            Divider()
            TextField("Drive", text: $model.drive)
            HStack {
                Button("Mount") { Task { await model.mount() } }
                Spacer()
                Button("Quit") { NSApplication.shared.terminate(nil) }
            }
            if !model.status.isEmpty { Text(model.status).font(.caption).textSelection(.enabled) }
        }
        .padding()
        .frame(width: 400)
        .task { await model.refresh() }
    }
}

/// What voidfs needs switched on: the extensions, and the daemon at login.
struct SetupView: View {
    @Bindable var model: HostModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            switch model.enabled {
            case true?: Label("File system extension enabled", systemImage: "checkmark.circle")
            case false?:
                Label("File system extension disabled", systemImage: "exclamationmark.triangle")
                Button("Open File System Extensions settings") { _ = FSClient.shared.openFileSystemExtensionsSettings() }
            case nil: EmptyView()
            }
            switch model.finderEnabled {
            case true?: Label("Finder extension enabled", systemImage: "checkmark.circle")
            case false?:
                Label("Finder extension disabled: no sync badges", systemImage: "exclamationmark.triangle")
                Button("Open Finder extension settings") { FIFinderSyncController.showExtensionManagementInterface() }
            case nil: EmptyView()
            }
            ServerKeyView(model: model)
            LoginItemView(model: model)
        }
    }
}

/// Start at login: the daemon, from the app's copy of `void`.
struct LoginItemView: View {
    @Bindable var model: HostModel

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            let on = Binding(get: { model.login == .on || model.login == .needsApproval }, set: { v in Task { await model.setLogin(v) } })
            Toggle("Start the daemon at login", isOn: on)
                .disabled(model.loginBusy || model.login == .notBundled || model.login == .notConfigured)
            switch model.login {
            case .needsApproval:
                Label("Waiting for your approval in Login Items", systemImage: "exclamationmark.triangle")
                Button("Open Login Items settings") { SMAppService.openSystemSettingsLoginItems() }
            case .notBundled:
                Text("This copy of voidfs has no daemon to start: build it with the Bundle void phase.").font(.caption)
            case .notConfigured:
                Text(LoginItem.notConfiguredHelp).font(.caption).textSelection(.enabled)
            case .on, .off: EmptyView()
            }
            if !model.loginNote.isEmpty { Text(model.loginNote).font(.caption).textSelection(.enabled) }
        }
    }
}

/// The sign-in form's fields; the secret stays here only until the form closes.
struct SignInFields: Equatable {
    var endpoint = ""
    var region = ""
    var accessKeyId = ""
    var secretAccessKey = ""
}

/// The server and key the daemon starts with, in the Keychain: who you are signed in as, or a
/// form to sign in with.
struct ServerKeyView: View {
    @Bindable var model: HostModel

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let s = model.signedIn {
                Label("Signed in to \(s.endpoint) as \(s.accessKeyId)", systemImage: "key")
                Button("Sign out") { Task { await model.signOut() } }.disabled(model.keyBusy)
            } else if model.signingIn {
                SignInFormView(model: model)
            } else {
                Label("Not signed in to a voidfs server", systemImage: "key.slash")
                Button("Sign in…") {
                    model.signingIn = true
                    model.keyNote = ""
                }
            }
            if !model.keyNote.isEmpty { Text(model.keyNote).font(.caption).textSelection(.enabled) }
        }
    }
}

struct SignInFormView: View {
    @Bindable var model: HostModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            TextField("Server", text: $model.form.endpoint, prompt: Text("https://voidfs.example.com"))
            TextField("Region", text: $model.form.region, prompt: Text(ServerKey.defaultRegion))
            TextField("Access key ID", text: $model.form.accessKeyId)
            SecureField("Secret access key", text: $model.form.secretAccessKey)
            HStack {
                Button("Import key file…") { model.importKeyFile() }
                Spacer()
                Button("Cancel") {
                    model.signingIn = false
                    model.form = SignInFields()
                    model.keyNote = ""
                }
                Button("Sign in") { Task { await model.signIn() } }.keyboardShortcut(.defaultAction)
            }
            .disabled(model.keyBusy)
        }
        .textFieldStyle(.roundedBorder)
    }
}
