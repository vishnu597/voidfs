// SPDX-License-Identifier: Apache-2.0
//
// `voidfs transfers-selftest <drive> [--cancel | --no-daemon | --look | --eject-busy | --force-eject | --eject-stale] [--png <path>]`: the menu's transfers controls
// against a running daemon, through the same model calls as the menu's buttons, each checked in the
// daemon's status or queue afterwards. It restores the limit and the pauses it changes. `--cancel`
// also cancels the newest unfinished import and clears finished uploads, which can't be undone,
// and cancels the oldest mount change the menu offers Cancel for, which then stays on this Mac, not
// uploaded, until its file is saved again. A mount's conflict, if there is one, is cancelled too, and
// stays a conflict, as does a change that waits for one; the menu offers Cancel for neither.
// `--no-daemon` expects the daemon stopped, and checks that the menu reads what waits in its journal.
// `--look` changes nothing: it prints the menu's uploads line, for drawing the menu as it is.
// `--eject-busy` expects an open file held by another process: the normal action must refuse,
// leaving the mount in place. `--force-eject` explicitly forces one mount of the named drive;
// it can disrupt open files and is only for a disposable test mount. Neither retries the other.
// `--eject-stale` verifies that a missing identity is rejected locally, and that a stale identity
// is refused by the daemon without ejecting the current attachment. It needs an updated daemon.
// `--png` draws the menu's view, offscreen, to a file.
//
// Run it against a test daemon: `-VoidfsStateDirectory <dir>` points the app at one.

import AppKit
import SwiftUI

@MainActor
enum TransfersSelfTest {
    static func run(_ args: [String]) async -> Int32 {
        var rest = args
        // UserDefaults still reads the original process arguments to select the test daemon.
        if let i = rest.firstIndex(of: "-VoidfsStateDirectory"), i + 1 < rest.count {
            rest.removeSubrange(i...(i + 1))
        }
        var png: String?
        if let i = rest.firstIndex(of: "--png"), i + 1 < rest.count {
            png = rest[i + 1]
            rest.removeSubrange(i...(i + 1))
        }
        let cancel = rest.contains("--cancel"), noDaemon = rest.contains("--no-daemon"), look = rest.contains("--look")
        let ejectBusy = rest.contains("--eject-busy"), forceEject = rest.contains("--force-eject")
        let ejectStale = rest.contains("--eject-stale")
        rest.removeAll { ["--cancel", "--no-daemon", "--look", "--eject-busy", "--force-eject", "--eject-stale"].contains($0) }
        guard rest.count == 1, let drive = rest.first, !drive.hasPrefix("-"), [cancel, noDaemon, look, ejectBusy, forceEject, ejectStale].filter({ $0 }).count <= 1 else {
            print("usage: transfers-selftest <drive> [--cancel | --no-daemon | --look | --eject-busy | --force-eject | --eject-stale] [--png <path>]")
            return 2
        }
        // All of the queue, to find the changes it controls; the menu lists only what it shows.
        let t = Transfers(limit: nil)
        var failed = 0
        func check(_ what: String, _ ok: Bool, _ detail: @autoclosure () -> String = "") {
            print("\(ok ? "ok  " : "FAIL") \(what)\(ok ? "" : ": \(detail())")")
            if !ok { failed += 1 }
            fflush(stdout)
        }
        /// The queue's changes show on its next listing, or within a second or two.
        func eventually(_ ok: () -> Bool) async -> Bool {
            for _ in 0..<30 {
                await t.refresh()
                await t.refreshQueue()
                if ok() { return true }
                try? await Task.sleep(for: .milliseconds(100))
            }
            return false
        }

        await t.refresh()
        await t.refreshQueue()
        if noDaemon {
            // The daemon is stopped: the menu says so and reads what waits in its journal.
            check("no daemon answers", t.status == nil && t.problem?.contains("isn't running") == true, t.problem ?? "")
            let words = t.waiting.flatMap(Words.waiting)
            check("the journal is read", words != nil, "nothing read")
            print("journal: \(words ?? "")")
            if let png { check("the view draws to \(png)", await draw(TransfersView(transfers: t), to: png)) }
            print(failed == 0 ? "transfers self-test passed" : "transfers self-test: \(failed) failed")
            return failed == 0 ? 0 : 1
        }
        guard let before = t.status?.uploads, let queue = t.queue else {
            print("FAIL no daemon status: \(t.problem ?? "")")
            return 1
        }
        print("transfers self-test: drive \(drive), \(Words.uploads(before))")
        if look {
            if let png { check("the view draws to \(png)", await draw(TransfersView(transfers: t), to: png)) }
            print(failed == 0 ? "transfers self-test passed" : "transfers self-test: \(failed) failed")
            return failed == 0 ? 0 : 1
        }
        if ejectBusy || forceEject || ejectStale {
            guard let mount = t.status?.mounts.first(where: { $0.drive == drive && $0.state == "mounted" }) else {
                print("FAIL no mounted test drive \(drive)")
                return 1
            }
            if ejectStale {
                guard let instance = mount.instance, !instance.isEmpty else {
                    print("FAIL the daemon needs an updated mount instance before testing guarded eject")
                    return 1
                }
                let legacy = try? JSONDecoder().decode(DaemonStatus.Mount.self, from: Data("""
                    {"drive":"legacy","mountpoint":"/voidfs-selftest-never-mounted","adapter":"fskit","state":"mounted","readOnly":false,"error":null}
                    """.utf8))
                check("older mount JSON decodes without an instance", legacy != nil && legacy?.instance == nil)
                if let legacy {
                    await t.unmount(legacy, force: true)
                    check("force without an instance is rejected locally", t.controlError == Transfers.forceNeedsIdentity, t.controlError ?? "no refusal")
                }
                let stale = DaemonStatus.Mount(drive: mount.drive, mountpoint: mount.mountpoint, adapter: mount.adapter,
                                               state: mount.state, readOnly: mount.readOnly, error: mount.error, instance: "stale-" + instance)
                await t.unmount(stale, force: true)
                check("a stale force eject is refused", t.controlError?.contains("Close this dialog and try again.") == true, t.controlError ?? "no refusal")
                check("a stale force eject leaves the current attachment mounted", t.status?.mounts.contains {
                    $0.id == mount.id && $0.instance == instance && $0.state == "mounted"
                } == true)
            } else {
                await t.unmount(mount, force: forceEject)
            }
            if ejectBusy {
                let error = t.controlError?.lowercased() ?? ""
                check("normal unmount reports a busy refusal", error.contains("busy") || error.contains("in use"), t.controlError ?? "no refusal")
                check("normal refusal leaves the mount in place", t.status?.mounts.contains { $0.id == mount.id && $0.state == "mounted" } == true)
            } else if forceEject {
                check("explicit force eject succeeds", t.controlError == nil, t.controlError ?? "")
                check("explicit force eject removes the mount", t.status?.mounts.contains { $0.id == mount.id } == false)
            }
            if let png { check("the view draws to \(png)", await draw(TransfersView(transfers: t), to: png)) }
            print(failed == 0 ? "transfers self-test passed" : "transfers self-test: \(failed) failed")
            return failed == 0 ? 0 : 1
        }

        await t.limit(123_456)
        check("a limit reaches the daemon", await eventually { t.status?.uploads.bandwidth == 123_456 }, "\(String(describing: t.status?.uploads.bandwidth))")
        await t.limit(nil)
        check("no limit clears it", await eventually { t.status?.uploads.bandwidth == nil }, "\(String(describing: t.status?.uploads.bandwidth))")
        await t.limit(before.bandwidth)
        check("the limit is as it was", await eventually { t.status?.uploads.bandwidth == before.bandwidth })

        if !before.paused {
            await t.pause(.all)
            check("pause all pauses", await eventually { t.status?.uploads.paused == true && t.queue?.paused == true })
            // A failure or a conflict outranks the pause.
            let problem = (t.status?.uploads.failed ?? 0) + (t.status?.uploads.conflicts ?? 0) > 0
            check("the symbol says paused", t.status?.uploads.unpublished == 0 || t.symbol == (problem ? "exclamationmark.triangle" : "pause.circle"), t.symbol)
            await t.resume(.all)
            check("resume all resumes", await eventually { t.status?.uploads.paused == false })
        }

        if !before.pausedDrives.contains(drive) {
            await t.pause(.drive(drive))
            check("pause \(drive) pauses the drive", await eventually { t.status?.uploads.pausedDrives.contains(drive) == true }, "\(t.status?.uploads.pausedDrives ?? [])")
            await t.resume(.drive(drive))
            check("resume \(drive) resumes it", await eventually { t.status?.uploads.pausedDrives.contains(drive) == false }, "\(t.status?.uploads.pausedDrives ?? [])")
        }

        if let b = queue.batches.last(where: { $0.open > 0 && !$0.paused }) {
            await t.pause(.batch(b.id))
            check("pause import \(b.id) pauses it", await eventually { t.queue?.batches.first { $0.id == b.id }?.paused == true })
            await t.resume(.batch(b.id))
            check("resume import \(b.id) resumes it", await eventually { t.queue?.batches.first { $0.id == b.id }?.paused == false })
        } else {
            print("skip no unpaused, unfinished import")
        }
        if let i = queue.items.first(where: { $0.toSend && !$0.paused }) {
            await t.pause(.entry(i.id))
            check("pause change \(i.id) pauses it", await eventually { t.queue?.items.first { $0.id == i.id }.map { $0.paused || $0.finished } ?? true })
            await t.resume(.entry(i.id))
            check("resume change \(i.id) resumes it", await eventually { t.queue?.items.first { $0.id == i.id }.map { !$0.paused } ?? true })
        } else {
            print("skip no unpaused, unfinished change")
        }

        if cancel {
            if let b = t.queue?.batches.last(where: { $0.open > 0 }) {
                await t.cancel(.batch(b.id))
                check("cancel import \(b.id) cancels its unfinished items", await eventually { t.queue?.batches.first { $0.id == b.id }.map { $0.open == 0 && $0.cancelled > 0 } ?? false })
                await t.clearFinished()
                check("clear finished forgets import \(b.id)", await eventually { t.queue?.batches.contains { $0.id == b.id } == false })
            } else {
                print("skip no unfinished import to cancel")
            }
            if let c = t.queue?.items.first(where: { $0.batch == nil && $0.conflict != nil && $0.toSend }) {
                check("a mount's conflict has no Cancel", t.queue?.cancellable(c) == false)
                await t.cancel(.entry(c.id))
                check("cancelling conflict \(c.id) leaves it a conflict", await eventually { t.queue?.items.first { $0.id == c.id }.map { $0.conflict != nil && $0.toSend } ?? false })
                check("the menu says nothing was cancelled", t.notice == Words.nothingCancelled, t.notice ?? "no notice")
            }
            // A mount's change to a conflict's file, made after the conflict, waits for it: the
            // menu has no Cancel for it, and a cancel leaves it as it is.
            if let q = t.queue, let w = q.items.first(where: { $0.batch == nil && $0.conflict == nil && $0.cancellable && !q.cancellable($0) }) {
                print("change \(w.id) (\(w.key), \(w.state)) waits for a conflict")
                await t.cancel(.entry(w.id))
                let after = t.queue?.items.first { $0.id == w.id }
                check("cancelling change \(w.id), which waits for a conflict, leaves it waiting", after.map { $0.state == w.state && !$0.kept } ?? false, "\(String(describing: after?.state))")
                check("the menu says nothing was cancelled", t.notice == Words.nothingCancelled, t.notice ?? "no notice")
            } else {
                print("skip no mount change waiting for a conflict")
            }
            if let q = t.queue, let i = q.items.first(where: { $0.batch == nil && $0.conflict == nil && q.cancellable($0) }) {
                let keptBefore = t.status?.uploads.kept ?? 0
                await t.cancel(.entry(i.id))
                check("cancel change \(i.id) keeps it on this Mac", await eventually { t.queue?.items.first { $0.id == i.id }?.kept == true }, "\(String(describing: t.queue?.items.first { $0.id == i.id }?.state))")
                check("the menu says what the cancel did", t.notice?.contains("on this Mac, not uploaded") == true, t.notice ?? "no notice")
                check("the uploads line counts the kept file", await eventually { (t.status?.uploads.kept ?? 0) > keptBefore && Words.uploads(t.status!.uploads).contains("kept on this Mac, not uploaded") },
                      t.status.map { Words.uploads($0.uploads) } ?? "")
                check("the kept change says so", t.queue?.items.first { $0.id == i.id }.map(Words.state) == "kept on this Mac, not uploaded")
            } else {
                print("skip no mount change waiting to cancel")
            }
        }
        check("no control failed", t.controlError == nil, t.controlError ?? "")

        if let png {
            check("the view draws to \(png)", await draw(TransfersView(transfers: t), to: png))
        }
        print(failed == 0 ? "transfers self-test passed" : "transfers self-test: \(failed) failed")
        return failed == 0 ? 0 : 1
    }

    /// Draws `view` in an offscreen window, controls included, which needs no screen access.
    static func draw(_ view: some View, to path: String) async -> Bool {
        let host = NSHostingView(rootView: view.padding().frame(width: 400).fixedSize(horizontal: false, vertical: true).background(Color(nsColor: .windowBackgroundColor)))
        host.appearance = NSAppearance(named: .aqua)
        host.frame.size = host.fittingSize
        let window = NSWindow(contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        // SwiftUI draws on the run loop's next pass.
        try? await Task.sleep(for: .milliseconds(500))
        host.frame.size = host.fittingSize
        host.layoutSubtreeIfNeeded()
        guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return false }
        host.cacheDisplay(in: host.bounds, to: rep)
        guard let data = rep.representation(using: .png, properties: [:]) else { return false }
        return (try? data.write(to: URL(filePath: path))) != nil
    }
}
