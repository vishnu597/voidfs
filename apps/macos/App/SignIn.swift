// SPDX-License-Identifier: Apache-2.0
//
// Signing in (step 5, item 7): the menu's form, or `voidfs server-key sign-in`, checks a server
// and key with the bundled `void` before the Keychain keeps them (ServerKey.swift), and restarts
// the daemon with them when the app starts it. Signing out removes the key and stops that daemon.
// The secret is shown nowhere: status names the server and the key's id.
//
//     voidfs server-key status
//     voidfs server-key sign-in --endpoint <url> [--region <region>] --key-file <file | ->
//     voidfs server-key sign-out
//     voidfs server-key selftest [--endpoint <url>]

import Foundation

enum SignIn {
    /// Who the app is signed in as for its state directory: never the secret.
    struct Summary: Equatable {
        let endpoint: String
        let region: String
        let accessKeyId: String
    }

    /// How long a check of the server, or a restart of the daemon, may take.
    static let checkWait: Duration = .seconds(30)

    static func summary() -> Summary? {
        guard let dir = Agent.stateDirectory, let key = try? ServerKeys.load(dir) else { return nil }
        return Summary(endpoint: key.endpoint, region: key.region, accessKeyId: key.accessKeyId)
    }

    /// Why `key` doesn't reach its server, or `nil` if it does: `void drives`, which signs with it.
    static func check(_ key: ServerKey) async -> String? {
        let args = ["drives", "--json", "--endpoint", key.endpoint, "--key-file", "/dev/stdin"]
        let (status, err) = await BundledVoid.run(args, env: ["VOIDFS_REGION": key.region], input: key.keyFile, timeout: checkWait)
        if status == 0 { return nil }
        let o = (try? JSONSerialization.jsonObject(with: Data(err.utf8))) as? [String: Any]
        if let m = (o?["error"] as? [String: Any])?["message"] as? String { return m }
        let text = err.trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty ? (status == nil ? "void didn't finish within \(checkWait)" : "void exited \(status!)") : text
    }

    /// A key the form or a command line gives, tidied, or why it can't be one.
    static func key(endpoint: String, region: String, accessKeyId: String, secretAccessKey: String) -> Result<ServerKey, Failure> {
        let endpoint = endpoint.trimmingCharacters(in: .whitespaces)
        let region = region.trimmingCharacters(in: .whitespaces)
        let id = accessKeyId.trimmingCharacters(in: .whitespaces)
        guard let url = URL(string: endpoint), ["http", "https"].contains(url.scheme ?? ""), url.host != nil else {
            return .failure(Failure("The server must be a URL such as https://voidfs.example.com."))
        }
        guard !id.isEmpty, !secretAccessKey.isEmpty else { return .failure(Failure("The access key needs its id and its secret.")) }
        return .success(ServerKey(endpoint: endpoint, region: region.isEmpty ? ServerKey.defaultRegion : region, accessKeyId: id, secretAccessKey: secretAccessKey))
    }

    struct Failure: Error {
        let message: String
        init(_ message: String) { self.message = message }
    }

    /// Checks `key`, keeps it, and restarts the daemon with it if the app starts the daemon.
    /// Whether it was kept, and what happened.
    static func signIn(_ key: ServerKey) async -> (Bool, String) {
        guard let dir = Agent.stateDirectory else { return (false, "There is no home directory for the daemon's state.") }
        if let why = await check(key) { return (false, "The server didn't accept it: \(why)") }
        do {
            try ServerKeys.save(key, for: dir)
        } catch {
            return (false, error.localizedDescription)
        }
        let text = "Signed in to \(key.endpoint) as \(key.accessKeyId)."
        guard LoginItem.service.status == .enabled else { return (true, "\(text) Turn on Start at login to start the daemon with it.") }
        return (true, "\(text) \(await restart())")
    }

    /// Removes the key; if the app starts the daemon, a `daemon.json` goes too, and the daemon
    /// stops until the next sign-in.
    static func signOut() async -> String {
        guard let dir = Agent.stateDirectory else { return "There is no home directory for the daemon's state." }
        let had: Bool
        do {
            had = try ServerKeys.delete(dir)
        } catch {
            return error.localizedDescription
        }
        guard LoginItem.service.status == .enabled else { return had ? "Signed out." : "Not signed in." }
        try? FileManager.default.removeItem(at: ServerKeys.file(dir))
        let pid = await stopDaemon()
        return "Signed out" + (pid.map { ": the daemon (pid \($0)) stopped, and starts again once you sign in." } ?? ".")
    }

    /// Stops the daemon that answers, and waits until it and its job have gone: its pid.
    static func stopDaemon() async -> Int? {
        guard let dir = Agent.stateDirectory, let pid = await LoginItem.daemonPid() else { return nil }
        let socket = DaemonSocket(path: dir.appending(path: "daemon.sock").path)
        _ = try? await socket.request("POST", "/v1/stop", generation: nil, contentType: "application/json", body: Data("{}".utf8), limit: 1 << 20)
        let deadline = ContinuousClock.now + .seconds(60)
        while ContinuousClock.now < deadline, await LoginItem.daemonPid() != nil || jobRunning() {
            try? await Task.sleep(for: .milliseconds(100))
        }
        return pid
    }

    /// Whether launchd runs the app's job now.
    static func jobRunning() -> Bool {
        let (status, out) = launchctl(["print", "gui/\(getuid())/\(LoginItem.label)"])
        return status == 0 && out.contains("\tstate = running")
    }

    @discardableResult
    static func launchctl(_ args: [String]) -> (Int32, String) {
        let p = Process()
        p.executableURL = URL(filePath: "/bin/launchctl")
        p.arguments = args
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        do { try p.run() } catch { return (-1, "") }
        let text = String(decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        p.waitUntilExit()
        return (p.terminationStatus, text)
    }

    /// Stops the app's daemon and has launchd start it again, with the key the Keychain has now.
    static func restart() async -> String {
        let before = await stopDaemon()
        launchctl(["kickstart", "gui/\(getuid())/\(LoginItem.label)"])
        guard let pid = await LoginItem.daemonPid(within: LoginItem.startWait, not: before) else {
            return "The daemon didn't answer within \(LoginItem.startWait): see daemon.log in \(Agent.stateDirectory?.path ?? "its state directory")."
        }
        return "The daemon (pid \(pid)) runs with it."
    }

    /// A key file as the CLI reads it: the JSON `void keys generate --format json` prints, or
    /// `[export ]NAME=value` lines naming `VOIDFS_ACCESS_KEY_ID` and `VOIDFS_SECRET_ACCESS_KEY`,
    /// and here also `VOIDFS_ENDPOINT` and `VOIDFS_REGION`.
    static func keyFile(_ text: String) -> [String: String]? {
        if text.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix("{") {
            guard let o = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any],
                  let id = o["accessKeyId"] as? String, let secret = o["secretAccessKey"] as? String else { return nil }
            return ["VOIDFS_ACCESS_KEY_ID": id, "VOIDFS_SECRET_ACCESS_KEY": secret]
        }
        var found: [String: String] = [:]
        for raw in text.split(whereSeparator: \.isNewline) {
            var line = raw.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("export ") { line.removeFirst("export ".count) }
            guard let eq = line.firstIndex(of: "=") else { continue }
            let name = line[..<eq].trimmingCharacters(in: .whitespaces)
            var value = line[line.index(after: eq)...].trimmingCharacters(in: .whitespaces)
            for q in ["'", "\""] where value.count >= 2 && value.hasPrefix(q) && value.hasSuffix(q) {
                value = String(value.dropFirst().dropLast())
            }
            if ["VOIDFS_ACCESS_KEY_ID", "VOIDFS_SECRET_ACCESS_KEY", "VOIDFS_ENDPOINT", "VOIDFS_REGION"].contains(name) { found[name] = value }
        }
        return found["VOIDFS_ACCESS_KEY_ID"] != nil && found["VOIDFS_SECRET_ACCESS_KEY"] != nil ? found : nil
    }

    /// `voidfs server-key …`.
    static func command(_ args: [String]) async -> Int32 {
        switch args.first {
        case "status", nil:
            guard let dir = Agent.stateDirectory else { return 1 }
            if let s = summary() {
                print("signed in to \(s.endpoint) (region \(s.region)) as \(s.accessKeyId), for \(dir.path)")
            } else {
                print("not signed in, for \(dir.path)")
            }
            if FileManager.default.fileExists(atPath: ServerKeys.file(dir).path) { print("daemon.json: there, with a key the terminal kept") }
            print("login item: \(LoginItem.describe)")
            return 0
        case "sign-in":
            var endpoint = "", region = "", file: String?
            var rest = Array(args.dropFirst())
            while rest.count >= 2 {
                switch rest[0] {
                case "--endpoint": endpoint = rest[1]
                case "--region": region = rest[1]
                case "--key-file": file = rest[1]
                // The defaults' argument domain, which Agent.stateDirectory reads, as a test does.
                case "-VoidfsStateDirectory": break
                default: print("unknown option \(rest[0])"); return 2
                }
                rest.removeFirst(2)
            }
            guard rest.isEmpty, let file else {
                print("usage: server-key sign-in --endpoint <url> [--region <region>] --key-file <file | ->")
                return 2
            }
            let data = file == "-" ? FileHandle.standardInput.readDataToEndOfFile() : FileManager.default.contents(atPath: file)
            guard let data, let found = keyFile(String(decoding: data, as: UTF8.self)) else {
                print("\(file == "-" ? "stdin" : file): not a key file: it needs an access key id and secret")
                return 2
            }
            if endpoint.isEmpty { endpoint = found["VOIDFS_ENDPOINT"] ?? "" }
            if region.isEmpty { region = found["VOIDFS_REGION"] ?? "" }
            switch key(endpoint: endpoint, region: region, accessKeyId: found["VOIDFS_ACCESS_KEY_ID"]!, secretAccessKey: found["VOIDFS_SECRET_ACCESS_KEY"]!) {
            case .failure(let f):
                print(f.message)
                return 2
            case .success(let k):
                let (kept, note) = await signIn(k)
                print(note)
                return kept ? 0 : 1
            }
        case "sign-out":
            print(await signOut())
            return 0
        case "selftest":
            return await KeychainSelfTest.run(Array(args.dropFirst()))
        default:
            print("usage: server-key status | sign-in --endpoint <url> [--region <region>] --key-file <file | -> | sign-out | selftest [--endpoint <url>]")
            return 2
        }
    }
}

/// `voidfs server-key selftest [--endpoint <url>]`: the Keychain store, `daemon.json`'s move, the
/// check of the bundled `void`, and the login job's launcher end to end, in a throwaway state
/// directory with throwaway keys, removed at the end. It registers nothing and leaves the app's
/// own state directory and key alone. Without `--endpoint`, the daemon starts offline.
enum KeychainSelfTest {
    static func run(_ args: [String]) async -> Int32 {
        let endpoint = args.count == 2 && args[0] == "--endpoint" ? args[1] : "http://127.0.0.1:9"
        var template = Array("/private/tmp/vks.XXXXXX".utf8CString)
        guard mkdtemp(&template) != nil else { print("mkdtemp failed"); return 1 }
        let dir = URL(filePath: String(cString: template))
        var failed = 0
        func check(_ ok: Bool, _ what: String) {
            print("\(ok ? "ok  " : "FAIL") \(what)")
            if !ok { failed += 1 }
        }
        let token = { String(format: "%08x", arc4random()) }
        let k1 = ServerKey(endpoint: endpoint, region: "us-east-1", accessKeyId: "VFSELFTEST1", secretAccessKey: "selftest-\(token())")
        let k2 = ServerKey(endpoint: "http://127.0.0.1:8", region: "eu-west-1", accessKeyId: "VFSELFTEST2", secretAccessKey: "selftest-\(token())")
        defer {
            _ = try? ServerKeys.delete(dir)
            try? FileManager.default.removeItem(at: dir)
        }
        print("state directory \(dir.path), server \(endpoint)")

        // The store.
        do {
            check(try ServerKeys.load(dir) == nil, "no key at first")
            try ServerKeys.save(k1, for: dir)
            check(try ServerKeys.load(dir) == k1, "a saved key reads back")
            try ServerKeys.save(k2, for: dir)
            check(try ServerKeys.load(dir) == k2, "saving again replaces it")
            check(try ServerKeys.delete(dir), "it is removed")
            check(try ServerKeys.load(dir) == nil && !(try ServerKeys.delete(dir)), "and gone")
        } catch {
            check(false, "the Keychain: \(error.localizedDescription)")
            return 1
        }

        // daemon.json moves in, unless the Keychain has a key, and goes either way.
        func writeFile(_ k: ServerKey) {
            FileManager.default.createFile(atPath: ServerKeys.file(dir).path, contents: try! JSONEncoder().encode(k), attributes: [.posixPermissions: 0o600])
        }
        do {
            check(try ServerKeys.adopt(dir) == nil, "no daemon.json, nothing to move")
            writeFile(k1)
            let moved = try ServerKeys.adopt(dir)
            let afterMove = try ServerKeys.load(dir)
            check(moved?.hasPrefix("moved") == true && afterMove == k1 && !FileManager.default.fileExists(atPath: ServerKeys.file(dir).path), "daemon.json's key moves in and the file goes: \(moved ?? "nil")")
            writeFile(k2)
            let kept = try ServerKeys.adopt(dir)
            let afterKeep = try ServerKeys.load(dir)
            check(kept?.hasPrefix("deleted") == true && afterKeep == k1 && !FileManager.default.fileExists(atPath: ServerKeys.file(dir).path), "the Keychain's key wins over a later file, which goes: \(kept ?? "nil")")
        } catch {
            check(false, "moving daemon.json: \(error.localizedDescription)")
        }

        // Only voidfs's own void gets a key.
        let null = open("/dev/null", O_WRONLY)
        defer { close(null) }
        func exits(_ executable: URL, _ args: [String]) -> Result<Int32, Error> {
            Result {
                let pid = try BundledVoid.spawn(args, env: BundledVoid.environment([:]), input: k1.keyFile, stdout: null, stderr: null, executable: executable)
                var status: Int32 = 0
                while waitpid(pid, &status, 0) < 0 && errno == EINTR {}
                return status
            }
        }
        if case .success(let s) = exits(BundledVoid.url, ["version"]) { check(s == 0, "the bundled void passes the check and runs") } else { check(false, "the bundled void passes the check") }
        let cat = exits(URL(filePath: "/bin/cat"), [])
        if case .failure(let e) = cat { check(e.localizedDescription.contains("isn't voidfs's own void"), "/bin/cat is refused: \(e.localizedDescription)") } else { check(false, "/bin/cat is refused") }
        let copy = dir.appending(path: "void-adhoc")
        try? FileManager.default.copyItem(at: BundledVoid.url, to: copy)
        let sign = Process()
        sign.executableURL = URL(filePath: "/usr/bin/codesign")
        sign.arguments = ["--force", "--sign", "-", "--identifier", "dev.voidfs.void", copy.path]
        sign.standardError = FileHandle.nullDevice
        try? sign.run()
        sign.waitUntilExit()
        if case .failure(let e) = exits(copy, ["version"]) { check(true, "an ad hoc copy named dev.voidfs.void is refused: \(e.localizedDescription)") } else { check(false, "an ad hoc copy is refused") }
        try? FileManager.default.removeItem(at: copy)

        // The launcher: as launchd runs the job, pointed at this directory.
        let log = dir.appending(path: "daemon.log")
        let socket = DaemonSocket(path: dir.appending(path: "daemon.sock").path)
        func launch() -> Process? {
            let p = Process()
            p.executableURL = Bundle.main.executableURL
            p.arguments = ["daemon-launch", "-VoidfsStateDirectory", dir.path]
            p.environment = ["HOME": FileManager.default.homeDirectoryForCurrentUser.path, "PATH": "/usr/bin:/bin"]
            p.standardOutput = FileHandle.nullDevice
            p.standardError = FileHandle.nullDevice
            do { try p.run() } catch { check(false, "running the launcher: \(error)"); return nil }
            return p
        }
        func status() async -> [String: Any]? {
            guard let r = try? await socket.request("GET", "/v1/status", generation: nil, limit: 1 << 20), r.status == 200 else { return nil }
            return (try? JSONSerialization.jsonObject(with: r.body)) as? [String: Any]
        }
        func answers() async -> [String: Any]? {
            let deadline = ContinuousClock.now + .seconds(20)
            while ContinuousClock.now < deadline {
                if let s = await status() { return s }
                try? await Task.sleep(for: .milliseconds(100))
            }
            return nil
        }
        func ends(_ p: Process) async -> Bool {
            let deadline = ContinuousClock.now + .seconds(30)
            while p.isRunning && ContinuousClock.now < deadline { try? await Task.sleep(for: .milliseconds(50)) }
            return !p.isRunning
        }
        func logText() -> String { (try? String(contentsOf: log, encoding: .utf8)) ?? "" }

        writeFile(k2)
        guard let p = launch() else { return 1 }
        if let s = await answers() {
            let daemon = s["daemon"] as? [String: Any], conn = s["connection"] as? [String: Any]
            let pid = daemon?["pid"] as? Int
            check(conn?["endpoint"] as? String == k1.endpoint, "the launcher's daemon answers (pid \(pid ?? -1)) with the Keychain's server")
            check(pid != nil && pid != Int(p.processIdentifier), "it runs void as its child (launcher pid \(p.processIdentifier))")
            check(!FileManager.default.fileExists(atPath: ServerKeys.file(dir).path) && logText().contains("deleted daemon.json: the Keychain has the server key already"), "daemon.json went, the Keychain's key kept")
            _ = try? await socket.request("POST", "/v1/stop", generation: nil, contentType: "application/json", body: Data("{}".utf8), limit: 1 << 20)
            check(await ends(p) && p.terminationReason == .exit && p.terminationStatus == 0, "stopped as asked, the launcher exits 0 (\(p.terminationStatus))")
        } else {
            check(false, "the launcher's daemon answers: \(logText())")
            p.terminate()
        }
        if let p = launch(), await answers() != nil {
            p.terminate()
            check(await ends(p) && p.terminationReason == .exit && p.terminationStatus == 0 && logText().contains("stopping (SIGTERM)"), "SIGTERM reaches the daemon, which stops; the launcher exits 0")
        } else { check(false, "a second start") }
        if let p = launch(), let s = await answers(), let pid = (s["daemon"] as? [String: Any])?["pid"] as? Int {
            kill(pid_t(pid), SIGKILL)
            check(await ends(p) && p.terminationReason == .uncaughtSignal && p.terminationStatus == SIGKILL, "a daemon killed takes the launcher with it, for launchd to run again (\(p.terminationReason == .uncaughtSignal ? "signal" : "exit") \(p.terminationStatus))")
        } else { check(false, "a third start") }
        _ = try? ServerKeys.delete(dir)
        if let p = launch() {
            check(await ends(p) && p.terminationStatus == 0 && logText().contains(DaemonLaunch.noKey(dir)), "with no key it says so and exits 0")
        }
        check(!logText().contains(k1.secretAccessKey) && !logText().contains(k2.secretAccessKey), "no secret in the log")
        print(failed == 0 ? "keychain self-test passed" : "keychain self-test: \(failed) failed")
        return failed == 0 ? 0 : 1
    }
}
