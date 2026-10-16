// SPDX-License-Identifier: Apache-2.0
//
// voidfs.app: the menu-bar host FSKit requires. It shows whether the extension is enabled, the
// daemon's transfers with their controls (Transfers.swift), and asks the daemon to mount drives,
// which it does with the extension (the `fskit` adapter) and remembers. The daemon holds the server
// and key (`void daemon start`); the extension and the app hold none.
//
// It also runs headless, for scripts:
//
//     voidfs.app/Contents/MacOS/voidfs status
//     voidfs.app/Contents/MacOS/voidfs transfers
//     voidfs.app/Contents/MacOS/voidfs transfers-selftest <drive> [--cancel] [--png <path>]
//     voidfs.app/Contents/MacOS/voidfs mount <drive> [mount-point]
//     voidfs.app/Contents/MacOS/voidfs settings
//     voidfs.app/Contents/MacOS/voidfs agent-register | agent-unregister | agent-status
//     voidfs.app/Contents/MacOS/voidfs bridge-check
//     voidfs.app/Contents/MacOS/voidfs bridge-selftest <drive> [main|daemon-restart|agent-restart|remote <key>|refused|wire] [--socket <path>]
//     voidfs.app/Contents/MacOS/voidfs badges-selftest <drive> [main|daemon-restart|refused] [--socket <path>] [--peer <path>] [--metrics <url>] [--files <n>] [--png <path>]
//     voidfs.app/Contents/MacOS/voidfs badges-check
//
// `agent` is how launchd runs it as the extensions' bridge to the daemon (Agent.swift, BadgeAgent.swift).

import FSKit
import ServiceManagement
import SwiftUI

let extensionBundleID = "dev.voidfs.app.fskit"

@main
enum Launcher {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        if args.first == "agent" { Agent.serve() }
        if args.first == "transfers-selftest" {
            // It draws with AppKit, which needs the main thread that dispatchMain() gives up.
            Task { @MainActor in exit(await TransfersSelfTest.run(Array(args.dropFirst()))) }
            NSApplication.shared.setActivationPolicy(.prohibited)
            NSApplication.shared.run()
        }
        if let command = args.first, ["status", "transfers", "mount", "settings", "agent-register", "agent-unregister", "agent-status", "bridge-check", "bridge-selftest", "badges-selftest", "badges-check"].contains(command) {
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

    func refresh() async {
        do {
            let modules = try await FSClient.shared.installedExtensions
            enabled = modules.first { $0.bundleIdentifier == extensionBundleID }?.isEnabled
            status = enabled == nil ? "The file system extension is not installed." : ""
        } catch {
            status = "Could not list file system extensions: \(error.localizedDescription)"
        }
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
            switch model.enabled {
            case true?: Label("File system extension enabled", systemImage: "checkmark.circle")
            case false?:
                Label("File system extension disabled", systemImage: "exclamationmark.triangle")
                Button("Open File System Extensions settings") { _ = FSClient.shared.openFileSystemExtensionsSettings() }
            case nil: EmptyView()
            }
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
