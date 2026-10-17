// SPDX-License-Identifier: Apache-2.0
//
// The daemon's server and key, in the shared team Keychain (step 5, item 7): a generic password
// in the data-protection keychain under the access group `HAUTK68F56.dev.voidfs`, one for each
// state directory. Only voidfs's own code, signed by its team with a provisioning profile that
// grants the group, can read it: this app and the jobs that run its binary, not a `void` built in
// a terminal (`bench/results/item7-keychain-probe`).
//
// The app writes it when you sign in from the menu, and hands it over: the login job runs this
// binary (`voidfs daemon-launch`), which reads the key and starts the bundled `void` with it on a
// pipe, once the started process shows it is voidfs's own `void`. A `daemon.json` that the
// terminal left in the state directory moves in, and the file goes, so that no secret stays on
// disk. Nothing here prints or logs the secret, or hands it to anything but that `void`.

import Foundation
import Security

/// A server and the key that signs for it, as `daemon.json` holds them.
struct ServerKey: Codable, Equatable {
    var endpoint: String
    var region: String
    var accessKeyId: String
    var secretAccessKey: String

    /// The server's default region, as the CLI's.
    static let defaultRegion = "us-east-1"

    /// What `void --key-file` reads: the key, without the server.
    var keyFile: Data {
        (try? JSONSerialization.data(withJSONObject: ["accessKeyId": accessKeyId, "secretAccessKey": secretAccessKey])) ?? Data()
    }
}

struct KeychainError: LocalizedError {
    let status: OSStatus
    let doing: String

    var errorDescription: String? {
        let why = SecCopyErrorMessageString(status, nil) as String? ?? "error"
        return "\(doing): \(why) (\(status))"
    }
}

enum ServerKeys {
    static let group = Bridge.appGroup
    static let service = "dev.voidfs.daemon"

    /// One item for each state directory.
    static func account(_ dir: URL) -> String { dir.standardizedFileURL.path }

    private static func query(_ dir: URL) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account(dir),
            kSecAttrAccessGroup as String: group,
            kSecUseDataProtectionKeychain as String: true,
        ]
    }

    /// The key for state directory `dir`, if the Keychain has one.
    static func load(_ dir: URL) throws -> ServerKey? {
        var q = query(dir)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &out)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = out as? Data else { throw KeychainError(status: status, doing: "Reading the server key from the Keychain") }
        return try JSONDecoder().decode(ServerKey.self, from: data)
    }

    /// Keeps `key` for `dir`, in place of the one there was. Readable once the Mac has been
    /// unlocked after it started, on this Mac only, so that the daemon starts at login.
    static func save(_ key: ServerKey, for dir: URL) throws {
        let data = try JSONEncoder().encode(key)
        let label = "voidfs server key (\(account(dir)))"
        var status = SecItemUpdate(query(dir) as CFDictionary, [kSecValueData as String: data, kSecAttrLabel as String: label] as CFDictionary)
        if status == errSecItemNotFound {
            var add = query(dir)
            add[kSecValueData as String] = data
            add[kSecAttrLabel as String] = label
            add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            status = SecItemAdd(add as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw KeychainError(status: status, doing: "Saving the server key in the Keychain") }
    }

    /// Removes the key for `dir`; whether there was one.
    @discardableResult
    static func delete(_ dir: URL) throws -> Bool {
        let status = SecItemDelete(query(dir) as CFDictionary)
        if status == errSecItemNotFound { return false }
        guard status == errSecSuccess else { throw KeychainError(status: status, doing: "Removing the server key from the Keychain") }
        return true
    }

    static func file(_ dir: URL) -> URL { dir.appending(path: "daemon.json") }

    /// The key a `daemon.json` in `dir` holds, if there is one.
    static func fileKey(_ dir: URL) throws -> ServerKey? {
        guard let data = FileManager.default.contents(atPath: file(dir).path) else { return nil }
        return try JSONDecoder().decode(ServerKey.self, from: data)
    }

    /// A `daemon.json` that `void daemon start` or `install` left in `dir` moves in: its key
    /// becomes the Keychain's, unless the Keychain has one already, and the file goes. What
    /// happened, for the log, or `nil` if there was no file. A file that can't be read, or a
    /// Keychain that refuses, leaves the file where it is.
    static func adopt(_ dir: URL) throws -> String? {
        guard let key = try fileKey(dir) else { return nil }
        let kept = try load(dir) != nil
        if !kept { try save(key, for: dir) }
        try FileManager.default.removeItem(at: file(dir))
        return kept ? "deleted daemon.json: the Keychain has the server key already" : "moved the server key from daemon.json into the Keychain, and deleted the file"
    }
}

/// The bundled `void`, started only once the process shows it is voidfs's own: a copy of the app
/// with something else in its place gets nothing.
enum BundledVoid {
    static let requirement = "anchor apple generic and certificate leaf[subject.OU] = \"HAUTK68F56\" and identifier \"dev.voidfs.void\""
    static var url: URL { Bundle.main.bundleURL.appending(path: "Contents/MacOS/void") }

    struct Refused: LocalizedError {
        let what: String
        var errorDescription: String? { what }
    }

    /// What `void` sees of the environment: launchd's few variables, a `PATH` of the system's own
    /// tools, and `extra`. Nothing of the caller's, which could point it elsewhere.
    static func environment(_ extra: [String: String]) -> [String: String] {
        let mine = ProcessInfo.processInfo.environment
        var env = ["PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": FileManager.default.homeDirectoryForCurrentUser.path]
        for name in ["USER", "LOGNAME", "TMPDIR"] { env[name] = mine[name] }
        return env.merging(extra) { $1 }
    }

    /// Starts `executable` (the bundled `void` unless a test names another) suspended, with
    /// `input` to come on its stdin and its output on `stdout` and `stderr`. Once the process
    /// satisfies `requirement`, it gets the input and runs; otherwise it is killed and this throws.
    static func spawn(_ args: [String], env: [String: String], input: Data, stdout: Int32, stderr: Int32, executable: URL = url) throws -> pid_t {
        var fds: [Int32] = [0, 0]
        guard pipe(&fds) == 0 else { throw Refused(what: "pipe: \(String(cString: strerror(errno)))") }
        let (r, w) = (fds[0], fds[1])
        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        posix_spawn_file_actions_adddup2(&actions, r, 0)
        posix_spawn_file_actions_adddup2(&actions, stdout, 1)
        posix_spawn_file_actions_adddup2(&actions, stderr, 2)
        var attr: posix_spawnattr_t?
        posix_spawnattr_init(&attr)
        defer { posix_spawnattr_destroy(&attr) }
        // Only what the actions name crosses over, signals as they are by default, none blocked.
        posix_spawnattr_setflags(&attr, Int16(POSIX_SPAWN_START_SUSPENDED | POSIX_SPAWN_CLOEXEC_DEFAULT | POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK))
        var all = sigset_t(), none = sigset_t()
        sigfillset(&all)
        sigemptyset(&none)
        posix_spawnattr_setsigdefault(&attr, &all)
        posix_spawnattr_setsigmask(&attr, &none)
        let argv: [UnsafeMutablePointer<CChar>?] = (["void"] + args).map { strdup($0) } + [nil]
        let envp: [UnsafeMutablePointer<CChar>?] = env.map { strdup("\($0.key)=\($0.value)") } + [nil]
        defer { (argv + envp).forEach { free($0) } }
        var pid: pid_t = 0
        let spawned = posix_spawn(&pid, executable.path, &actions, &attr, argv, envp)
        close(r)
        guard spawned == 0 else {
            close(w)
            throw Refused(what: "starting \(executable.path): \(String(cString: strerror(spawned)))")
        }
        // The process the kernel started, not the file, which could change after.
        if let why = check(pid) {
            close(w)
            kill(pid, SIGKILL)
            var status: Int32 = 0
            waitpid(pid, &status, 0)
            throw Refused(what: "refused to hand the server key to \(executable.path): \(why)")
        }
        let wrote = input.withUnsafeBytes { raw -> Bool in
            var at = 0
            while at < raw.count {
                let n = write(w, raw.baseAddress! + at, raw.count - at)
                if n < 0 && errno == EINTR { continue }
                if n <= 0 { return false }
                at += n
            }
            return true
        }
        close(w)
        kill(pid, SIGCONT)
        if !wrote { throw Refused(what: "writing to \(executable.path): \(String(cString: strerror(errno)))") }
        return pid
    }

    /// Why process `pid` isn't voidfs's own `void`, or `nil` if it is.
    static func check(_ pid: pid_t) -> String? {
        var code: SecCode?
        var status = SecCodeCopyGuestWithAttributes(nil, [kSecGuestAttributePid: NSNumber(value: pid)] as CFDictionary, [], &code)
        guard status == errSecSuccess, let code else { return "its code signature: \(message(status))" }
        var requirement: SecRequirement?
        status = SecRequirementCreateWithString(self.requirement as CFString, [], &requirement)
        guard status == errSecSuccess, let requirement else { return "the requirement: \(message(status))" }
        status = SecCodeCheckValidity(code, [], requirement)
        return status == errSecSuccess ? nil : "it isn't voidfs's own void (\(message(status)))"
    }

    static func message(_ status: OSStatus) -> String {
        "\(SecCopyErrorMessageString(status, nil) as String? ?? "error") (\(status))"
    }

    /// Runs `void <args>` with `input` on stdin, as `spawn` does, and waits up to `timeout`: its
    /// exit status (nil if it was killed or didn't start) and what it wrote to stderr.
    static func run(_ args: [String], env: [String: String], input: Data, timeout: Duration) async -> (Int32?, String) {
        await withCheckedContinuation { done in
            Thread.detachNewThread {
                var err: [Int32] = [0, 0]
                guard pipe(&err) == 0 else { done.resume(returning: (nil, "pipe: \(String(cString: strerror(errno)))")); return }
                let null = open("/dev/null", O_WRONLY)
                defer { close(null) }
                let pid: pid_t
                do {
                    pid = try spawn(args, env: environment(env), input: input, stdout: null, stderr: err[1])
                } catch {
                    close(err[0]); close(err[1])
                    done.resume(returning: (nil, error.localizedDescription))
                    return
                }
                close(err[1])
                let timer = DispatchWorkItem { kill(pid, SIGKILL) }
                DispatchQueue.global().asyncAfter(deadline: .now() + .milliseconds(Int(timeout.components.seconds * 1000)), execute: timer)
                let text = String(decoding: FileHandle(fileDescriptor: err[0], closeOnDealloc: true).readDataToEndOfFile(), as: UTF8.self)
                var status: Int32 = 0
                while waitpid(pid, &status, 0) < 0 && errno == EINTR {}
                timer.cancel()
                let exited = status & 0x7f == 0
                done.resume(returning: (exited ? (status >> 8) & 0xff : nil, text))
            }
        }
    }
}

/// `voidfs daemon-launch`: the app's login job (`dev.voidfs.app.daemon`). It opens the state
/// directory's log, moves a `daemon.json` into the Keychain, reads the key, and runs
/// `void daemon run --app --logged` with it on stdin, staying as its parent: launchd watches this
/// process, which passes SIGTERM and SIGINT on and ends as the daemon does.
enum DaemonLaunch {
    /// As the CLI's: a log larger than this is moved to `daemon.log.1` when the daemon starts.
    static let logMax: UInt64 = 8 << 20
    nonisolated(unsafe) static var logFd: Int32 = 2

    static func log(_ line: String) {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let text = "\(f.string(from: Date())) \(line)\n"
        _ = text.withCString { write(logFd, $0, strlen($0)) }
    }

    /// The line `void daemon start` looks for, as the daemon's own.
    static func noKey(_ dir: URL) -> String { "no server key in voidfs.app for \(dir.path): sign in from its menu; not starting" }

    static func run() -> Never {
        guard let dir = Agent.stateDirectory else {
            log("no home directory for the daemon's state")
            exit(1)
        }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        chmod(dir.path, 0o700)
        let logPath = dir.appending(path: "daemon.log").path
        if let size = (try? FileManager.default.attributesOfItem(atPath: logPath))?[.size] as? UInt64, size > logMax {
            _ = rename(logPath, logPath + ".1")
        }
        let fd = open(logPath, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0o600)
        if fd >= 0 { logFd = fd }
        // launchd stops the job with SIGTERM: it goes to the daemon, which stops as asked.
        let child = Child()
        let sources = [SIGTERM, SIGINT, SIGHUP].map { sig in
            signal(sig, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: sig, queue: .global())
            source.setEventHandler { if sig != SIGHUP { child.signal(sig) } }
            source.resume()
            return source
        }
        do {
            if let note = try ServerKeys.adopt(dir) { log(note) }
        } catch {
            log("daemon.json: \(error.localizedDescription)")
        }
        var key: ServerKey?
        do {
            key = try ServerKeys.load(dir)
        } catch {
            log(error.localizedDescription)
        }
        // A Keychain that refused leaves daemon.json, whose key still starts the daemon.
        if key == nil, let file = try? ServerKeys.fileKey(dir) {
            log("using daemon.json's key: the Keychain doesn't have it")
            key = file
        }
        guard let key else {
            log(noKey(dir))
            exit(0)
        }
        let env = BundledVoid.environment(["VOIDFS_STATE_DIR": dir.path, "VOIDFS_REGION": key.region])
        let args = ["daemon", "run", "--app", "--logged", "--endpoint", key.endpoint, "--key-file", "/dev/stdin"]
        let pid: pid_t
        do {
            pid = try BundledVoid.spawn(args, env: env, input: key.keyFile, stdout: logFd, stderr: logFd)
        } catch {
            log("\(error.localizedDescription); not starting")
            // Nothing launchd could fix by running it again.
            exit(0)
        }
        child.started(pid)
        var status: Int32 = 0
        while waitpid(pid, &status, 0) < 0 && errno == EINTR {}
        withExtendedLifetime(sources) {}
        let sig = status & 0x7f
        if sig == 0 { exit((status >> 8) & 0xff) }
        // Killed: so is this, so that launchd runs the job again and says why.
        log("the daemon (pid \(pid)) was killed by signal \(sig)")
        signal(sig, SIG_DFL)
        raise(sig)
        exit(128 + sig)
    }

    /// The daemon's pid once it runs, and a signal that came before.
    final class Child: @unchecked Sendable {
        private let lock = NSLock()
        private var pid: pid_t = 0
        private var pending: Int32 = 0

        func signal(_ sig: Int32) {
            lock.lock(); defer { lock.unlock() }
            if pid > 0 { kill(pid, sig) } else { pending = sig }
        }

        func started(_ pid: pid_t) {
            lock.lock(); defer { lock.unlock() }
            self.pid = pid
            if pending != 0 { kill(pid, pending) }
        }
    }
}
