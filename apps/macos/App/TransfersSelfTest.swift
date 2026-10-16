// SPDX-License-Identifier: Apache-2.0
//
// `voidfs transfers-selftest <drive> [--cancel] [--png <path>]`: the menu's transfers controls
// against a running daemon, through the same model calls as the menu's buttons, each checked in the
// daemon's status or queue afterwards. It restores the limit and the pauses it changes. `--cancel`
// also cancels the newest unfinished import and clears finished uploads, which can't be undone; a
// mount's changes are never cancelled. `--png` draws the menu's view, offscreen, to a file.
//
// Run it against a test daemon: `-VoidfsStateDirectory <dir>` points the app at one.

import AppKit
import SwiftUI

@MainActor
enum TransfersSelfTest {
    static func run(_ args: [String]) async -> Int32 {
        var rest = args
        var png: String?
        if let i = rest.firstIndex(of: "--png"), i + 1 < rest.count {
            png = rest[i + 1]
            rest.removeSubrange(i...(i + 1))
        }
        let cancel = rest.contains("--cancel")
        rest.removeAll { $0 == "--cancel" }
        guard let drive = rest.first else {
            print("usage: transfers-selftest <drive> [--cancel] [--png <path>]")
            return 2
        }
        let t = Transfers()
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
        guard let before = t.status?.uploads, let queue = t.queue else {
            print("FAIL no daemon status: \(t.problem ?? "")")
            return 1
        }
        print("transfers self-test: drive \(drive), \(Words.uploads(before))")

        await t.limit(123_456)
        check("a limit reaches the daemon", await eventually { t.status?.uploads.bandwidth == 123_456 }, "\(String(describing: t.status?.uploads.bandwidth))")
        await t.limit(nil)
        check("no limit clears it", await eventually { t.status?.uploads.bandwidth == nil }, "\(String(describing: t.status?.uploads.bandwidth))")
        await t.limit(before.bandwidth)
        check("the limit is as it was", await eventually { t.status?.uploads.bandwidth == before.bandwidth })

        if !before.paused {
            await t.pause(.all)
            check("pause all pauses", await eventually { t.status?.uploads.paused == true && t.queue?.paused == true })
            check("the symbol says paused", t.status?.uploads.unpublished == 0 || t.symbol == "pause.circle", t.symbol)
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
        if let i = queue.items.first(where: { !$0.finished && !$0.paused }) {
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
