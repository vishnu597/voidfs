// SPDX-License-Identifier: Apache-2.0
//
// The Finder Sync extension (step 5, item 8): badges on the items of mounted voidfs drives, as
// the daemon reports them through the app's agent (`Badges/Badges.swift`), and a context menu
// with the drive's pause or resume, as the menu bar has them. Cancel is the menu bar's only (the
// user's choice, 16 October): there the menu says what it does, keeping the change on this Mac.

import Cocoa
import FinderSync

final class FinderSync: FIFinderSync {
    private lazy var model = BadgeModel(client: BadgeClient(), setDirectories: { urls in
        DispatchQueue.main.async { FIFinderSyncController.default().directoryURLs = urls }
    }, setBadge: { identifier, url in
        DispatchQueue.main.async { FIFinderSyncController.default().setBadgeIdentifier(identifier, for: url) }
    })

    override init() {
        super.init()
        for badge in BadgeArt.all {
            FIFinderSyncController.default().setBadgeImage(BadgeArt.image(badge), label: badge.label, forBadgeIdentifier: badge.identifier)
        }
        model.start()
    }

    // MARK: Badges

    override func beginObservingDirectory(at url: URL) { model.observe(url) }

    override func endObservingDirectory(at url: URL) { model.unobserve(url) }

    override func requestBadgeIdentifier(for url: URL) { model.request(url) }

    // MARK: The context menu

    private var target: BadgeModel.Mount? {
        let c = FIFinderSyncController.default()
        guard let url = c.selectedItemURLs()?.first ?? c.targetedURL() else { return nil }
        return model.mount(of: url)
    }

    override func menu(for menuKind: FIMenuKind) -> NSMenu? {
        guard menuKind == .contextualMenuForItems || menuKind == .contextualMenuForContainer, let mount = target, !mount.readOnly else { return nil }
        let menu = NSMenu(title: "voidfs")
        if mount.paused {
            menu.addItem(withTitle: "Resume Uploads to \(mount.drive)", action: #selector(resume(_:)), keyEquivalent: "")
        } else {
            menu.addItem(withTitle: "Pause Uploads to \(mount.drive)", action: #selector(pause(_:)), keyEquivalent: "")
        }
        return menu
    }

    @IBAction func pause(_ sender: AnyObject?) {
        guard let mount = target else { return }
        Task { try? await model.pause(mount) }
    }

    @IBAction func resume(_ sender: AnyObject?) {
        guard let mount = target else { return }
        Task { try? await model.resume(mount) }
    }
}
