// SPDX-License-Identifier: Apache-2.0
//
// The menu's transfers (step 5, item 8): the daemon's connection, mounts and upload queue, in the
// words `void status` and `void uploads` use, with the queue's pause, resume, cancel, bandwidth
// limit and clear. The daemon is the only source: the app keeps no state of its own, so what the
// menu shows comes back after the app, the bridge or the daemon restarts. While no daemon runs, the
// menu reads what waits in its journal, writing nothing, as `void status` does.
//
// Cancel stops an upload. For an import, its unsent files aren't uploaded; their sources stay where
// they are. For a mount's change, the user chose the fate (16 October): the copy on this Mac keeps
// the change, not uploaded ("kept"), and the next save of the file uploads it as it is then,
// guarded on the drive's version, so another computer's save meanwhile becomes a conflict. The
// menu says so beside the button, and again after a cancel. Finder's context menu has no cancel.

import Foundation
import Observation
import SQLite3
import SwiftUI

/// `GET /v1/status`: the fields the menu shows (crates/voidfs-daemon/src/api.rs).
struct DaemonStatus: Decodable {
    struct Connection: Decodable {
        let endpoint: String
        let link: String
    }

    struct Uploads: Decodable {
        let uploading, queued, failed, conflicts, pausedItems, unpublished, unpublishedBytes, rate: UInt64
        /// Files with a cancelled mount change, kept on this Mac; older daemons don't send it.
        let kept: UInt64?
        let paused: Bool
        let pausedDrives: [String]
        let bandwidth: UInt64?
    }

    struct Mount: Decodable, Identifiable {
        let drive, mountpoint, adapter, state: String
        let readOnly: Bool
        let error: String?
        var id: String { mountpoint }
    }

    let connection: Connection
    let uploads: Uploads
    let mounts: [Mount]
}

/// `GET /v1/uploads`, and each line of `/v1/uploads/watch`.
struct UploadList: Decodable {
    struct Item: Decodable, Identifiable {
        let id: Int64
        let drive, key, op, state: String
        let toKey: String?
        let paused: Bool
        /// An import's batch; a mount's changes have none.
        let batch: Int64?
        let size, sent: UInt64
        let conflict, error: String?
        var finished: Bool { state == "done" || state == "cancelled" }
        /// A mount's change whose upload was cancelled: on this Mac, not uploaded.
        var kept: Bool { state == "kept" }
        var toSend: Bool { !finished && !kept }
        /// A mount's conflict isn't cancelled: both versions stay on this Mac until it's resolved.
        var cancellable: Bool { toSend && !(batch == nil && conflict != nil) }
    }

    struct Batch: Decodable, Identifiable {
        let id: Int64
        let label: String
        let items, done, failed, cancelled, bytes, sent: UInt64
        let paused: Bool
        var open: UInt64 { items - done - failed - cancelled }
    }

    let items: [Item]
    let batches: [Batch]
    let paused: Bool
    let pausedDrives: [String]
    let bandwidth: UInt64?
    let unpublished, unpublishedBytes, rate: UInt64
    let etaSecs: UInt64?
    /// Items a `limit` left out, and a mount's changes still to send, listed or not.
    let more, mountUnsent: UInt64?

    /// Whether the menu offers Cancel for `i`. A mount's change that waits for a conflict, on its
    /// file or on a folder it's in, stays as it is, as the conflict does: none of it is sent until
    /// the conflict is resolved. The queue knows what waits by inode; the menu goes by the
    /// conflicts' keys, which a limited list holds, as it lists failures before what is queued.
    func cancellable(_ i: Item) -> Bool {
        guard i.cancellable, i.batch == nil else { return i.cancellable }
        let paths = [i.key, i.toKey].compactMap { $0 }
        return !items.contains { c in
            c.batch == nil && c.conflict != nil && c.toSend && c.drive == i.drive && [c.key, c.toKey].compactMap { $0 }.contains { held in
                paths.contains { $0 == held || held.hasSuffix("/") && $0.hasPrefix(held) }
            }
        }
    }
}

/// What a pause, a resume or a cancel applies to, as the daemon reads it.
enum UploadScope {
    case all
    case drive(String)
    case batch(Int64)
    case entry(Int64)

    var json: Any {
        switch self {
        case .all: return "all"
        case .drive(let d): return ["drive": d]
        case .batch(let b): return ["batch": b]
        case .entry(let e): return ["entry": e]
        }
    }
}

/// The CLI's words for the same things (crates/voidfs-cli/src/{output,daemon,uploads,mounts}.rs),
/// so that the menu and `void status` agree.
enum Words {
    static func size(_ bytes: UInt64) -> String {
        if bytes < 1024 { return "\(bytes) B" }
        let units = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"]
        var v = Double(bytes) / 1024, unit = 0
        while v >= 1024 && unit < units.count - 1 {
            v /= 1024
            unit += 1
        }
        return String(format: "%.1f %@", v, units[unit])
    }

    static func duration(_ secs: UInt64) -> String {
        let (d, h, m, s) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60, secs % 60)
        switch (d, h, m) {
        case (0, 0, 0): return "\(s)s"
        case (0, 0, _): return "\(m)m \(s)s"
        case (0, _, _): return "\(h)h \(m)m"
        default: return "\(d)d \(h)h"
        }
    }

    static func link(_ link: String) -> String {
        switch link {
        case "degraded": "degraded: errors or slow answers"
        case "offline": "offline: uploads wait, and reads of what isn't cached fail"
        default: link
        }
    }

    /// `void status`'s words for files with a kept change.
    static func kept(_ files: UInt64) -> String {
        "\(files) \(files == 1 ? "file" : "files") kept on this Mac, not uploaded"
    }

    /// What a cancel says it did to a mount's changes.
    static func keptByCancel(_ changes: UInt64) -> String {
        "\(changes) \(changes == 1 ? "change stays" : "changes stay") on this Mac, not uploaded: saving the file again uploads it as it is then."
    }

    /// What a cancel that changed nothing says.
    static let nothingCancelled = "Nothing was cancelled: it was sent already, or it waits for a conflict, which stays on this Mac until it's resolved."

    /// What `void status` says waits in the journal while no daemon runs, or `nil` for nothing.
    static func waiting(_ w: Journal.Waiting) -> String? {
        let kept = w.kept > 0 ? Self.kept(w.kept) : nil
        guard w.changes > 0 else { return kept }
        let line = "\(w.changes) \(w.changes == 1 ? "change" : "changes") (\(size(w.bytes))) wait in the journal, for the daemon to start"
        return kept.map { "\(line) · \($0)" } ?? line
    }

    /// `void status`'s uploads line.
    static func uploads(_ u: DaemonStatus.Uploads) -> String {
        let kept = (u.kept ?? 0) > 0 ? " · \(Self.kept(u.kept ?? 0))" : ""
        if u.unpublished == 0 { return (u.paused ? "idle · paused" : "idle") + kept }
        let parts = [(u.uploading, "uploading"), (u.queued, "queued"), (u.failed, "failed"), (u.conflicts, "in conflict")].filter { $0.0 > 0 }.map { "\($0.0) \($0.1)" }
        var line = "\(parts.joined(separator: ", ")) · \(size(u.unpublishedBytes)) to send"
        if u.rate > 0 { line += " · \(size(u.rate))/s" }
        if u.paused {
            line += " · paused"
        } else if u.pausedItems > 0 {
            line += " · \(u.pausedItems) paused"
        }
        if !u.pausedDrives.isEmpty { line += " · paused on \(u.pausedDrives.joined(separator: ", "))" }
        if let b = u.bandwidth { line += " · limit \(size(b))/s" }
        return line + kept
    }

    /// An item's state, as `void uploads` says it.
    static func state(_ i: UploadList.Item) -> String {
        switch i.state {
        case "failed" where i.conflict != nil:
            if let v = i.conflict, v != "?" { return "conflict: the drive changed (now version \(v)); this Mac keeps its copy" }
            return "conflict: the drive changed; this Mac keeps its copy"
        case "failed": return "failed: \(i.error ?? "")"
        case _ where i.paused && !i.finished: return "paused"
        case "done" where i.conflict != nil: return "done, over a newer version"
        case "kept": return "kept on this Mac, not uploaded"
        default: return i.state
        }
    }

    static func percent(_ i: UploadList.Item) -> UInt64 {
        i.state == "done" || i.size == 0 ? 100 : i.sent * 100 / i.size
    }

    /// A batch's state, as `void uploads` says it.
    static func batch(_ b: UploadList.Batch) -> String {
        let pct = b.bytes == 0 ? 100 : b.sent * 100 / b.bytes
        switch (b.open, b.failed) {
        case (0, 0) where b.cancelled == b.items: return "cancelled"
        case (0, 0) where b.cancelled > 0: return "done, \(b.cancelled) cancelled"
        case (0, 0): return "done"
        case (0, _): return "failed"
        case _ where b.paused: return "paused at \(pct)%"
        default: return "\(pct)%"
        }
    }

    /// `void status`'s mount line, without the feed's position.
    static func mount(_ m: DaemonStatus.Mount) -> String {
        var l = "\(m.drive) at \(m.mountpoint) · \(m.adapter)"
        if m.readOnly { l += " · read-only" }
        l += m.error.map { " · \(m.state): \($0)" } ?? " · \(m.state)"
        return l
    }
}

@Observable
@MainActor
final class Transfers {
    private(set) var status: DaemonStatus?
    private(set) var queue: UploadList?
    /// Why there is no status: no daemon runs, or it answered something else.
    private(set) var problem: String?
    /// The last control's failure, until the next control.
    private(set) var controlError: String?
    /// What the last control did that its button doesn't show: a cancel keeping a mount's changes.
    private(set) var notice: String?
    /// While no daemon runs, what its journal holds for it.
    private(set) var waiting: Journal.Waiting?
    @ObservationIgnored let socket: DaemonSocket?
    @ObservationIgnored private var started = false
    /// The query that lists the queue: the rows the menu shows, which most need showing, unless
    /// all are asked for. With 10,000 changes queued, a whole list is about 1.4 MB a second.
    @ObservationIgnored let listing: String

    /// `limit`: how many changes to list; `nil` lists them all.
    init(limit: Int? = TransfersView.shownItems) {
        socket = Agent.stateDirectory.map { DaemonSocket(path: $0.appending(path: "daemon.sock").path) }
        listing = limit.map { "?limit=\($0)" } ?? ""
        if socket == nil { problem = "No home directory for the daemon's state." }
    }

    /// Follows the daemon until the app quits: its status every 2 seconds, and its queue as
    /// `/v1/uploads/watch` sends it, each second. Both start again when the daemon does.
    func start() {
        guard !started, let socket else { return }
        started = true
        let target = "/v1/uploads/watch" + listing
        Thread.detachNewThread { [weak self] in
            Self.watch(socket, target) { list in Task { @MainActor in self?.queue = list } }
        }
        Task {
            while true {
                await refresh()
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    nonisolated private static func watch(_ socket: DaemonSocket, _ target: String, _ deliver: @escaping (UploadList?) -> Void) -> Never {
        let decoder = JSONDecoder()
        while true {
            // Unlimited, a large import lists every item each second, so a line can run to megabytes.
            try? socket.stream(target, generation: nil, limit: 64 << 20, opened: { _ in }) { line in
                deliver(try? decoder.decode(UploadList.self, from: line))
                return true
            }
            deliver(nil)
            Thread.sleep(forTimeInterval: 2)
        }
    }

    func refresh() async {
        guard let socket else { return }
        do {
            let r = try await socket.request("GET", "/v1/status", generation: nil, limit: 4 << 20)
            guard r.status == 200 else { throw DaemonSocketError.malformed(Self.message(r)) }
            status = try JSONDecoder().decode(DaemonStatus.self, from: r.body)
            (problem, waiting) = (nil, nil)
        } catch DaemonSocketError.notRunning {
            (status, queue) = (nil, nil)
            problem = "The voidfs daemon isn't running: start it with `void daemon start`."
            waiting = Agent.stateDirectory.flatMap(Journal.waiting)
        } catch {
            status = nil
            problem = "The daemon's status failed: \(error)"
        }
    }

    /// The queue now, rather than at the watch's next line: after a control, say.
    func refreshQueue() async {
        guard let socket, let r = try? await socket.request("GET", "/v1/uploads" + listing, generation: nil, limit: 64 << 20), r.status == 200 else { return }
        if let q = try? JSONDecoder().decode(UploadList.self, from: r.body) { queue = q }
    }

    func pause(_ scope: UploadScope) async { await call("POST", "/v1/uploads/pause", scope.json) }
    func resume(_ scope: UploadScope) async { await call("POST", "/v1/uploads/resume", scope.json) }
    /// For an import and for a mount's change, which stays on this Mac; see the top of this file.
    func cancel(_ scope: UploadScope) async {
        let answer = await call("POST", "/v1/uploads/cancel", scope.json)
        let kept = (answer?["kept"] as? NSNumber)?.uint64Value ?? 0
        if kept > 0 {
            notice = Words.keptByCancel(kept)
        } else if (answer?["items"] as? NSNumber)?.uint64Value == 0 {
            notice = Words.nothingCancelled
        }
    }
    func limit(_ bytesPerSecond: UInt64?) async { await call("PUT", "/v1/uploads/limit", ["bytesPerSecond": bytesPerSecond.map { $0 as Any } ?? NSNull()]) }
    func clearFinished() async { await call("POST", "/v1/uploads/clear", nil) }
    /// As `void unmount`: the daemon unmounts it and forgets it; its unpublished changes still go.
    func unmount(_ m: DaemonStatus.Mount) async { await call("POST", "/v1/mounts/unmount", ["target": m.mountpoint]) }

    /// The daemon's answer to a control, when it succeeded.
    @discardableResult
    private func call(_ method: String, _ target: String, _ body: Any?) async -> [String: Any]? {
        guard let socket else { return nil }
        notice = nil
        var answer: [String: Any]?
        do {
            let data = try body.map { try JSONSerialization.data(withJSONObject: $0, options: .fragmentsAllowed) } ?? Data()
            let r = try await socket.request(method, target, generation: nil, contentType: body == nil ? nil : "application/json", body: data, limit: 1 << 20)
            controlError = (200..<300).contains(r.status) ? nil : Self.message(r)
            if controlError == nil { answer = (try? JSONSerialization.jsonObject(with: r.body)) as? [String: Any] }
        } catch DaemonSocketError.notRunning {
            controlError = "The voidfs daemon isn't running."
        } catch {
            controlError = "\(error)"
        }
        await refresh()
        await refreshQueue()
        return answer
    }

    /// The daemon's error message, or its status code.
    nonisolated static func message(_ r: DaemonResponse) -> String {
        let o = (try? JSONSerialization.jsonObject(with: r.body)) as? [String: Any]
        return (o?["error"] as? [String: Any])?["message"] as? String ?? "the daemon answered \(r.status)"
    }

    /// The menu bar's symbol: what most needs looking at.
    var symbol: String {
        guard let u = status?.uploads else { return "exclamationmark.triangle" }
        if status?.connection.link == "offline" || u.failed > 0 || u.conflicts > 0 { return "exclamationmark.triangle" }
        if u.unpublished > 0 { return u.paused ? "pause.circle" : "arrow.up.circle" }
        return "externaldrive.connected.to.line.below"
    }

    /// What `transfers` prints: the menu's lines, laid out as `void status` lays out its rows.
    static func report() async -> (String, Int32) {
        let t = Transfers(limit: nil)
        await t.refresh()
        guard let s = t.status else {
            let waiting = t.waiting.flatMap(Words.waiting).map { "\nuploads   \($0)" } ?? ""
            return ((t.problem ?? "no status") + waiting, 1)
        }
        var rows = [("server", "\(s.connection.endpoint) · \(Words.link(s.connection.link))"), ("uploads", Words.uploads(s.uploads))]
        rows += s.mounts.isEmpty ? [("mounts", "none")] : s.mounts.enumerated().map { ($0.offset == 0 ? "mounts" : "", Words.mount($0.element)) }
        await t.refreshQueue()
        if let q = t.queue {
            for (n, i) in q.items.enumerated() {
                let key = i.toKey.map { "\(i.key) -> \($0)" } ?? i.key
                rows.append((n == 0 ? "changes" : "", "\(i.id) \(i.drive) \(key) \(Words.percent(i))% \(Words.state(i))"))
            }
            for (n, b) in q.batches.reversed().enumerated() {
                rows.append((n == 0 ? "imports" : "", "\(b.id) \(b.label) \(Words.batch(b))"))
            }
        }
        return (rows.map { $0.0.padding(toLength: 10, withPad: " ", startingAt: 0) + $0.1 }.joined(separator: "\n"), 0)
    }
}

/// The daemon's journal, read while no daemon runs, as `void status` reads it.
enum Journal {
    /// What waits: changes to send and their bytes (`voidfs_client::queue::unpublished`), and the
    /// files with a change kept on this Mac (`queue::kept`).
    struct Waiting: Equatable {
        let changes, bytes, kept: UInt64
    }

    /// The newest schema this build reads (crates/voidfs-client/src/store.rs's migrations).
    static let schema: Int64 = 10

    /// Reads `state.sqlite`, writing nothing. `nil` when there is none, or a newer voidfs wrote it.
    /// The connection may write, though: a read-only one can't open the WAL database once a
    /// stopped daemon has removed its shared-memory file.
    static func waiting(in dir: URL) -> Waiting? {
        let path = dir.appending(path: "state.sqlite").path
        guard FileManager.default.fileExists(atPath: path) else { return nil }
        var db: OpaquePointer?
        defer { sqlite3_close(db) }
        guard sqlite3_open_v2(path, &db, SQLITE_OPEN_READWRITE, nil) == SQLITE_OK else { return nil }
        sqlite3_busy_timeout(db, 500)
        func row(_ sql: String) -> [Int64]? {
            var q: OpaquePointer?
            defer { sqlite3_finalize(q) }
            guard sqlite3_prepare_v2(db, sql, -1, &q, nil) == SQLITE_OK, sqlite3_step(q) == SQLITE_ROW else { return nil }
            return (0..<sqlite3_column_count(q)).map { sqlite3_column_int64(q, $0) }
        }
        guard let version = row("PRAGMA user_version")?.first, version <= schema,
              let send = row("SELECT COUNT(*), COALESCE(SUM(MAX(size - sent, 0)), 0) FROM entries WHERE state NOT IN ('done', 'cancelled', 'kept')"),
              let kept = row("SELECT COUNT(*) FROM (SELECT DISTINCT drive, key FROM entries WHERE state = 'kept')")
        else { return nil }
        return Waiting(changes: UInt64(send[0]), bytes: UInt64(send[1]), kept: UInt64(kept[0]))
    }
}

/// The menu's transfers: the server, the queue and its controls, the mounts, and the changes and
/// imports still to send.
struct TransfersView: View {
    let transfers: Transfers
    /// Rows past these are left to `void uploads`, which lists them all.
    static let shownItems = 6, shownBatches = 4

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let problem = transfers.problem {
                Label(problem, systemImage: "exclamationmark.triangle").font(.caption).textSelection(.enabled)
            }
            if let w = transfers.waiting.flatMap(Words.waiting) {
                row("Uploads", w)
            }
            if let s = transfers.status {
                row("Server", "\(s.connection.endpoint) · \(Words.link(s.connection.link))")
                row("Uploads", Words.uploads(s.uploads))
                if let eta = transfers.queue?.etaSecs, s.uploads.unpublished > 0, !s.uploads.paused { row("", "\(Words.duration(eta)) left") }
                controls(s.uploads)
                mounts(s)
            }
            if let q = transfers.queue {
                changes(q)
                imports(q)
            }
            if let n = transfers.notice { Text(n).font(.caption).textSelection(.enabled) }
            if let e = transfers.controlError { Text(e).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
        }
    }

    private func row(_ name: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(name).foregroundStyle(.secondary).frame(width: 56, alignment: .leading)
            Text(value).textSelection(.enabled)
        }
        .font(.caption)
    }

    private func act(_ f: @escaping (Transfers) async -> Void) -> () -> Void {
        { Task { await f(transfers) } }
    }

    private func controls(_ u: DaemonStatus.Uploads) -> some View {
        HStack {
            if u.paused {
                Button("Resume all", action: act { await $0.resume(.all) })
            } else {
                Button("Pause all", action: act { await $0.pause(.all) })
            }
            Menu(u.bandwidth.map { "Limit \(Words.size($0))/s" } ?? "No limit") {
                Button("No limit", action: act { await $0.limit(nil) })
                ForEach([1, 10, 100], id: \.self) { mib in
                    Button("\(mib) MiB/s", action: act { await $0.limit(UInt64(mib) << 20) })
                }
            }
            .fixedSize()
            Spacer()
            if transfers.queue?.batches.contains(where: { $0.open == 0 }) == true {
                Button("Clear finished", action: act { await $0.clearFinished() })
            }
        }
        .controlSize(.small)
    }

    @ViewBuilder
    private func mounts(_ s: DaemonStatus) -> some View {
        if !s.mounts.isEmpty {
            Divider()
            ForEach(s.mounts) { m in
                HStack(alignment: .firstTextBaseline) {
                    Text(Words.mount(m)).font(.caption).textSelection(.enabled)
                    Spacer()
                    if s.uploads.pausedDrives.contains(m.drive) {
                        Button("Resume", action: act { await $0.resume(.drive(m.drive)) })
                    } else {
                        Button("Pause", action: act { await $0.pause(.drive(m.drive)) })
                    }
                    Button("Unmount", action: act { await $0.unmount(m) })
                }
                .controlSize(.small)
            }
        }
    }

    /// What Cancel does to a mount's change, beside the button.
    static let keepsHere = "Cancel keeps a change on this Mac, not uploaded. Saving the file again uploads it as it is then."

    /// Unfinished changes: the uploading ones first, then those that need looking at, then those
    /// kept on this Mac.
    @ViewBuilder
    private func changes(_ q: UploadList) -> some View {
        let order = ["uploading": 0, "failed": 1, "queued": 2, "kept": 4]
        let open = q.items.filter { !$0.finished }.sorted { (order[$0.state] ?? 3, $0.id) < (order[$1.state] ?? 3, $1.id) }
        // A limited list counts what it leaves out; an older daemon lists everything.
        let total = open.count + Int(q.more ?? 0)
        let saved = q.mountUnsent.map(Int.init) ?? open.filter { $0.batch == nil && $0.toSend }.count
        let kept = transfers.status?.uploads.kept.map(Int.init) ?? Set(open.filter(\.kept).map { "\($0.drive)/\($0.key)" }).count
        if total > 0 {
            Divider()
            if saved > 0 {
                Text("\(saved) \(saved == 1 ? "change is" : "changes are") saved on this Mac and not yet on the drive. \(Self.keepsHere)").font(.caption).foregroundStyle(.secondary)
            }
            if kept > 0 {
                Text("\(Words.kept(UInt64(kept))): saving one again uploads it.").font(.caption).foregroundStyle(.secondary)
            }
            ForEach(open.prefix(Self.shownItems)) { i in
                HStack(alignment: .firstTextBaseline) {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(i.toKey.map { "\(i.key) → \($0)" } ?? i.key).lineLimit(1).truncationMode(.middle)
                        Text("\(i.drive) · \(Words.size(i.size)) · \(Words.percent(i))% · \(Words.state(i))").foregroundStyle(.secondary).lineLimit(2)
                    }
                    .font(.caption)
                    .textSelection(.enabled)
                    Spacer()
                    if i.toSend {
                        if i.paused {
                            Button("Resume", action: act { await $0.resume(.entry(i.id)) })
                        } else {
                            Button("Pause", action: act { await $0.pause(.entry(i.id)) })
                        }
                        if q.cancellable(i) {
                            Button("Cancel", action: act { await $0.cancel(.entry(i.id)) })
                                .help(i.batch == nil ? Self.keepsHere : "Cancel this file's upload. Its source stays where it is.")
                        }
                    }
                }
                .controlSize(.small)
            }
            if total > Self.shownItems {
                Text("and \(total - Self.shownItems) more: `void uploads` lists them all").font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    /// Imports, newest first.
    @ViewBuilder
    private func imports(_ q: UploadList) -> some View {
        if !q.batches.isEmpty {
            Divider()
            ForEach(q.batches.reversed().prefix(Self.shownBatches)) { b in
                HStack(alignment: .firstTextBaseline) {
                    Text("\(b.label) · \(b.done) of \(b.items) · \(Words.size(b.bytes)) · \(Words.batch(b))").font(.caption).lineLimit(2).textSelection(.enabled)
                    Spacer()
                    if b.open > 0 {
                        if b.paused {
                            Button("Resume", action: act { await $0.resume(.batch(b.id)) })
                        } else {
                            Button("Pause", action: act { await $0.pause(.batch(b.id)) })
                        }
                        Button("Cancel", action: act { await $0.cancel(.batch(b.id)) })
                    }
                }
                .controlSize(.small)
            }
        }
    }
}
