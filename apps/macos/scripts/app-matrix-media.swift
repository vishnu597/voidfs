// SPDX-License-Identifier: Apache-2.0
// Build: xcrun swiftc -swift-version 5 -module-cache-path /private/tmp/voidfs-media-cache apps/macos/scripts/app-matrix-media.swift -o /private/tmp/voidfs-media
// make <movie.mov> <frames> <width> <height> <seed>; inspect <movie.mov>
import Foundation
import AVFoundation
import CoreVideo

func emit(_ fields: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: fields, options: [.sortedKeys])
    print(String(decoding: data, as: UTF8.self))
    fflush(stdout)
}
func failure(_ text: String) -> Never { fputs(text + "\n", stderr); exit(1) }
let args = Array(CommandLine.arguments.dropFirst())
guard args.count >= 2 else { failure("make <movie.mov> <frames> <width> <height> <seed> | inspect <movie.mov>") }
let url = URL(fileURLWithPath: args[1])
if args[0] == "make" {
    guard args.count == 6, let frames = Int(args[2]), let width = Int(args[3]), let height = Int(args[4]), let seed = UInt32(args[5]), frames > 0, width > 0, height > 0 else { failure("invalid make arguments") }
    let start = Date()
    do {
        let writer = try AVAssetWriter(outputURL: url, fileType: .mov)
        let input = AVAssetWriterInput(mediaType: .video, outputSettings: [AVVideoCodecKey: AVVideoCodecType.proRes422, AVVideoWidthKey: width, AVVideoHeightKey: height])
        input.expectsMediaDataInRealTime = false
        let adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: input, sourcePixelBufferAttributes: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA, kCVPixelBufferWidthKey as String: width, kCVPixelBufferHeightKey as String: height])
        guard writer.canAdd(input) else { failure("ProRes writer input unsupported") }
        writer.add(input)
        guard writer.startWriting() else { failure("startWriting: \(String(describing: writer.error))") }
        writer.startSession(atSourceTime: .zero)
        for frame in 0..<frames {
            while !input.isReadyForMoreMediaData {
                if writer.status == .failed { failure("writer failed: \(String(describing: writer.error))") }
                Thread.sleep(forTimeInterval: 0.002)
            }
            var pixel: CVPixelBuffer?
            let result = CVPixelBufferPoolCreatePixelBuffer(nil, adaptor.pixelBufferPool!, &pixel)
            guard result == kCVReturnSuccess, let pixel else { failure("pixel buffer: \(result)") }
            CVPixelBufferLockBaseAddress(pixel, [])
            let stride = CVPixelBufferGetBytesPerRow(pixel)
            let bytes = CVPixelBufferGetBaseAddress(pixel)!.assumingMemoryBound(to: UInt8.self)
            // Deterministic spatial noise gives each clip unique real encoded frames; no filler/dedup surrogate.
            var state = seed &+ UInt32(frame) &* 1664525 &+ 1013904223
            for y in 0..<height {
                for x in 0..<width {
                    state ^= state << 13; state ^= state >> 17; state ^= state << 5
                    let index = y * stride + x * 4
                    bytes[index] = UInt8(truncatingIfNeeded: state)
                    bytes[index + 1] = UInt8(truncatingIfNeeded: state >> 8)
                    bytes[index + 2] = UInt8(truncatingIfNeeded: state >> 16)
                    bytes[index + 3] = 255
                }
            }
            CVPixelBufferUnlockBaseAddress(pixel, [])
            guard adaptor.append(pixel, withPresentationTime: CMTime(value: Int64(frame), timescale: 30)) else { failure("append: \(String(describing: writer.error))") }
        }
        input.markAsFinished()
        let done = DispatchSemaphore(value: 0)
        writer.finishWriting { done.signal() }
        guard done.wait(timeout: .now() + 60) == .success, writer.status == .completed else { failure("finish: \(String(describing: writer.error))") }
        let size = (try FileManager.default.attributesOfItem(atPath: url.path)[.size] as! NSNumber).uint64Value
        emit(["operation": "make", "path": url.path, "codec": "ProRes422", "frames": frames, "width": width, "height": height, "seed": seed, "bytes": size, "elapsedSeconds": Date().timeIntervalSince(start)])
    } catch { failure("\(error)") }
} else if args[0] == "inspect" {
    let start = Date()
    do {
        let asset = AVURLAsset(url: url)
        guard let track = asset.tracks(withMediaType: .video).first else { failure("no video track") }
        let duration = CMTimeGetSeconds(asset.duration)
        var reads: [[String: Any]] = []
        // Scrub at nonsequential points, decoding real frames rather than only reading a movie header.
        for fraction in [0.0, 0.8, 0.2, 0.6, 0.4, 0.95] {
            let seek = Date()
            let reader = try AVAssetReader(asset: asset)
            reader.timeRange = CMTimeRange(start: CMTime(seconds: duration * fraction, preferredTimescale: 600), duration: CMTime(seconds: min(0.5, duration * (1 - fraction)), preferredTimescale: 600))
            let output = AVAssetReaderTrackOutput(track: track, outputSettings: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA])
            reader.add(output)
            guard reader.startReading() else { failure("reader start: \(String(describing: reader.error))") }
            var frames = 0
            while let sample = output.copyNextSampleBuffer() {
                guard CMSampleBufferGetImageBuffer(sample) != nil else { failure("decoded sample has no image") }
                frames += 1
            }
            guard frames > 0, reader.status != .failed else { failure("scrub decoded no frames: \(String(describing: reader.error))") }
            reads.append(["fraction": fraction, "decodedFrames": frames, "elapsedSeconds": Date().timeIntervalSince(seek)])
        }
        emit(["operation": "inspect", "path": url.path, "durationSeconds": duration, "width": track.naturalSize.width, "height": track.naturalSize.height, "scrubs": reads, "elapsedSeconds": Date().timeIntervalSince(start)])
    } catch { failure("\(error)") }
} else { failure("unknown operation") }
