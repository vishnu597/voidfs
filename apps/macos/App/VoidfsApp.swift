// SPDX-License-Identifier: Apache-2.0
//
// voidfs.app: the menu-bar host FSKit requires. For the spike it stores an access key where the
// extension can read it, shows whether the extension is enabled, and mounts drives.
//
// It also runs headless, for scripts:
//
//     voidfs.app/Contents/MacOS/voidfs status
//     voidfs.app/Contents/MacOS/voidfs save voidfs://127.0.0.1:9000/spike <access-key-id>   (secret on stdin)
//     voidfs.app/Contents/MacOS/voidfs mount voidfs://127.0.0.1:9000/spike [mount-point]
//     voidfs.app/Contents/MacOS/voidfs settings
//     voidfs.app/Contents/MacOS/voidfs agent-register | agent-unregister | agent-status
//
// `agent` is how launchd runs it as the signed-bundle probe's helper (LaunchAgents/).

import FSKit
import ServiceManagement
import SwiftUI

let extensionBundleID = "dev.voidfs.app.fskit"

@main
enum Launcher {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        if args.first == "agent" { Agent.serve() }
        if let command = args.first, ["status", "save", "mount", "settings", "agent-register", "agent-unregister", "agent-status"].contains(command) {
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
            case "save":
                guard args.count == 3, let url = URL(string: args[1]), let target = MountTarget(url: url) else {
                    print("usage: save voidfs://host:port/drive <access-key-id>  (secret on stdin)")
                    return 2
                }
                guard let secret = readLine(strippingNewline: true), !secret.isEmpty else { print("no secret on stdin"); return 2 }
                try MountStore.save(target, .init(accessKeyId: args[2], secretAccessKey: secret))
                print("saved credentials for \(target.url.absoluteString) in \(MountStore.fileURL!.path)")
            case "mount":
                guard args.count >= 2, let url = URL(string: args[1]), let target = MountTarget(url: url) else {
                    print("usage: mount voidfs://host:port/drive [mount-point]")
                    return 2
                }
                let start = ContinuousClock.now
                let path = try await Mounter.mount(target, at: args.count > 2 ? URL(filePath: args[2]) : nil)
                print("mounted at \(path.path) in \(ContinuousClock.now - start)")
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

/// The signed-bundle probe's helper: the app itself, which launchd runs from the bundle's
/// `Contents/Library/LaunchAgents` plist once it is registered and the user allows it. It answers
/// the extension's XPC echo on the App-Group-prefixed name, the only kind the sandbox lets the
/// extension reach.
enum Agent {
    static let service = SMAppService.agent(plistName: "dev.voidfs.agent.plist")

    static var status: String {
        switch service.status {
        case .notRegistered: "not registered"
        case .enabled: "enabled"
        case .requiresApproval: "requires approval in System Settings → General → Login Items & Extensions"
        case .notFound: "not found in the bundle"
        @unknown default: "unknown (\(service.status.rawValue))"
        }
    }

    static func serve() -> Never {
        let delegate = EchoDelegate()
        let listener = NSXPCListener(machServiceName: "\(MountStore.appGroup).agent")
        listener.delegate = delegate
        listener.resume()
        withExtendedLifetime((listener, delegate)) { RunLoop.main.run() }
        exit(0)
    }
}

final class Echo: NSObject, XPCEcho {
    func echo(_ data: Data, reply: @escaping (Data) -> Void) { reply(data) }
}

final class EchoDelegate: NSObject, NSXPCListenerDelegate {
    func listener(_ listener: NSXPCListener, shouldAcceptNewConnection connection: NSXPCConnection) -> Bool {
        connection.exportedInterface = NSXPCInterface(with: XPCEcho.self)
        connection.exportedObject = Echo()
        connection.resume()
        return true
    }
}

enum Mounter {
    /// Mounts a drive. `FSClient.mountSingleVolume` (macOS 27) mounts in /Volumes but needs the
    /// `com.apple.developer.fskit.mount` entitlement, which the team's profile does not grant
    /// (fskitd answers EPERM). mount(8) needs no entitlement and works for the logged-in user, so
    /// it is the fallback, into `~/voidfs/<drive>` unless told otherwise.
    static func mount(_ target: MountTarget, at mountPoint: URL? = nil) async throws -> URL {
        do {
            return try await FSClient.shared.mountSingleVolume(resource: FSGenericURLResource(url: target.url), bundleID: extensionBundleID, options: [])
        } catch let e as NSError where e.domain == NSPOSIXErrorDomain && e.code == Int(EPERM) {
            let dir = mountPoint ?? FileManager.default.homeDirectoryForCurrentUser.appending(path: "voidfs/\(target.drive)")
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let p = Process()
            p.executableURL = URL(filePath: "/sbin/mount")
            p.arguments = ["-F", "-t", "voidfs", target.url.absoluteString, dir.path]
            let err = Pipe()
            p.standardError = err
            try p.run()
            p.waitUntilExit()
            guard p.terminationStatus == 0 else {
                let message = String(data: err.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
                throw NSError(domain: "dev.voidfs", code: Int(p.terminationStatus), userInfo: [NSLocalizedDescriptionKey: message.trimmingCharacters(in: .whitespacesAndNewlines)])
            }
            return dir
        }
    }
}

struct VoidfsApp: App {
    @State private var model = HostModel()

    var body: some Scene {
        MenuBarExtra("voidfs", systemImage: "externaldrive.connected.to.line.below") {
            HostView(model: model)
        }
        .menuBarExtraStyle(.window)
    }
}

@Observable
@MainActor
final class HostModel {
    var url = "voidfs://127.0.0.1:9000/spike"
    var accessKeyId = ""
    var secret = ""
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

    func saveAndMount() async {
        guard let u = URL(string: url), let target = MountTarget(url: u) else { status = "Not a voidfs:// URL"; return }
        do {
            if !accessKeyId.isEmpty, !secret.isEmpty {
                try MountStore.save(target, .init(accessKeyId: accessKeyId, secretAccessKey: secret))
                secret = ""
            }
            status = "Mounting…"
            let path = try await Mounter.mount(target)
            status = "Mounted at \(path.path)"
            NSWorkspace.shared.open(path)
        } catch {
            status = "Mount failed: \(error.localizedDescription)"
        }
    }
}

struct HostView: View {
    @Bindable var model: HostModel

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
            TextField("voidfs://host:port/drive", text: $model.url)
            TextField("Access key id", text: $model.accessKeyId)
            SecureField("Secret access key", text: $model.secret)
            HStack {
                Button("Mount") { Task { await model.saveAndMount() } }
                Spacer()
                Button("Quit") { NSApplication.shared.terminate(nil) }
            }
            if !model.status.isEmpty { Text(model.status).font(.caption).textSelection(.enabled) }
        }
        .padding()
        .frame(width: 340)
        .task { await model.refresh() }
    }
}
