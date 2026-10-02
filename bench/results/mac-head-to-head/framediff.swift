// SPDX-License-Identifier: Apache-2.0
// framediff <dir> <t0> <x> <y> <w> <h> [threshold]: for frames named <unix time>.png, prints each frame whose
// crop (in image pixels) differs from the previous frame's by more than threshold (mean abs diff per byte).
import CoreGraphics
import Foundation
import ImageIO
let a = CommandLine.arguments
let dir = a[1], t0 = Double(a[2])!
let rect = CGRect(x: Double(a[3])!, y: Double(a[4])!, width: Double(a[5])!, height: Double(a[6])!)
let threshold = a.count > 7 ? Double(a[7])! : 0.5
let files = try! FileManager.default.contentsOfDirectory(atPath: dir).filter { $0.hasSuffix(".png") }
    .sorted { Double($0.dropLast(4))! < Double($1.dropLast(4))! }
func pixels(_ path: String) -> [UInt8] {
    let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil)!
    let img = CGImageSourceCreateImageAtIndex(src, 0, nil)!.cropping(to: rect)!
    let w = img.width, h = img.height
    var buf = [UInt8](repeating: 0, count: w * h * 4)
    let ctx = CGContext(data: &buf, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                        space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
    return buf
}
var prev: [UInt8]? = nil
for f in files {
    let p = pixels(dir + "/" + f)
    var d = 0.0
    if let q = prev { var s = 0; for i in 0..<p.count { s += abs(Int(p[i]) - Int(q[i])) }; d = Double(s) / Double(p.count) }
    if prev == nil || d > threshold { print(String(format: "%8.2f s  diff %6.2f  %@", Double(f.dropLast(4))! - t0, d, f)) }
    prev = p
}
