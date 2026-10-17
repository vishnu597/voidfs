// SPDX-License-Identifier: Apache-2.0
//
// The badges' pictures and labels: an SF Symbol on a white disc, drawn at the 320-point size
// Finder scales down from. The label is what Finder shows for the badge in the Get Info window
// and as the item's sync status.

import AppKit

enum BadgeArt {
    struct Badge {
        let identifier: String
        let symbol: String
        let color: NSColor
        let label: String
    }

    static let all: [Badge] = [
        Badge(identifier: "saved", symbol: "checkmark.circle.fill", color: .systemGreen, label: "Saved"),
        Badge(identifier: "pending", symbol: "clock.circle.fill", color: .systemGray, label: "Waiting to save"),
        Badge(identifier: "saving", symbol: "arrow.up.circle.fill", color: .systemBlue, label: "Saving"),
        Badge(identifier: "error", symbol: "xmark.circle.fill", color: .systemRed, label: "Not saved: an error"),
        Badge(identifier: "conflict", symbol: "exclamationmark.triangle.fill", color: .systemOrange, label: "Not saved: changed on the drive"),
        Badge(identifier: "kept", symbol: "minus.circle.fill", color: .systemPurple, label: "Not uploaded: kept on this Mac"),
    ]

    static let side: CGFloat = 320

    static func image(_ badge: Badge) -> NSImage {
        NSImage(size: NSSize(width: side, height: side), flipped: false) { rect in
            NSColor.white.setFill()
            NSBezierPath(ovalIn: rect.insetBy(dx: 8, dy: 8)).fill()
            let config = NSImage.SymbolConfiguration(pointSize: side * 0.7, weight: .bold).applying(.init(paletteColors: [.white, badge.color]))
            guard let symbol = NSImage(systemSymbolName: badge.symbol, accessibilityDescription: badge.label)?.withSymbolConfiguration(config) else { return false }
            let size = symbol.size
            let scale = min(rect.width * 0.86 / size.width, rect.height * 0.86 / size.height)
            let drawn = NSSize(width: size.width * scale, height: size.height * scale)
            symbol.draw(in: NSRect(x: rect.midX - drawn.width / 2, y: rect.midY - drawn.height / 2, width: drawn.width, height: drawn.height))
            return true
        }
    }

    /// The pictures side by side as a PNG, for the self-test's evidence.
    static func sheet() -> Data? {
        let n = CGFloat(all.count)
        let cell: CGFloat = 96
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(cell * n), pixelsHigh: Int(cell), bitsPerSample: 8, samplesPerPixel: 4,
                                         hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else { return nil }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        NSColor(white: 0.85, alpha: 1).setFill()
        NSRect(x: 0, y: 0, width: cell * n, height: cell).fill()
        for (i, badge) in all.enumerated() {
            image(badge).draw(in: NSRect(x: CGFloat(i) * cell + 8, y: 8, width: cell - 16, height: cell - 16))
        }
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:])
    }
}
