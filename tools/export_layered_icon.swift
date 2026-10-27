#!/usr/bin/env swift
// Export the supplied transparent logo without redrawing it.
import Foundation
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

guard CommandLine.arguments.count == 3 else {
    fatalError("usage: swift tools/export_layered_icon.swift source.png repository-root")
}
let sourceURL = URL(fileURLWithPath: CommandLine.arguments[1])
let root = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
guard let decoder = CGImageSourceCreateWithURL(sourceURL as CFURL, nil),
      let source = CGImageSourceCreateImageAtIndex(decoder, 0, nil) else {
    fatalError("Cannot decode source logo")
}
let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
func context(_ width: Int, _ height: Int) -> CGContext {
    guard let result = CGContext(data: nil, width: width, height: height,
                                bitsPerComponent: 8, bytesPerRow: width * 4,
                                space: colorSpace,
                                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue |
                                    CGBitmapInfo.byteOrder32Big.rawValue) else {
        fatalError("Cannot create RGBA canvas")
    }
    result.interpolationQuality = .high
    return result
}
let decoded = context(source.width, source.height)
decoded.draw(source, in: CGRect(x: 0, y: 0, width: source.width, height: source.height))
let pixels = decoded.data!.assumingMemoryBound(to: UInt8.self)
var left = source.width, right = 0, bottom = source.height, top = 0
for y in 0..<source.height {
    for x in 0..<source.width where pixels[(y * source.width + x) * 4 + 3] > 0 {
        left = min(left, x); right = max(right, x + 1)
        bottom = min(bottom, y); top = max(top, y + 1)
    }
}
guard left < right && bottom < top else { fatalError("Source is fully transparent") }
let centerX = Double(left + right) / 2, centerY = Double(bottom + top) / 2
var sourceRadius = 0.0
for y in bottom..<top {
    for x in left..<right where pixels[(y * source.width + x) * 4 + 3] > 0 {
        sourceRadius = max(sourceRadius, hypot(Double(x) + 0.5 - centerX,
                                              Double(y) + 0.5 - centerY))
    }
}
// Match the SDK's 288 px layer canvas and its central 192 px viewport.
// Our 180 px content circle leaves room for filtering, masking and layer motion.
let size = 288, scale = 89.0 / sourceRadius
let foreground = context(size, size)
foreground.draw(source, in: CGRect(x: 144 - centerX * scale, y: 144 - centerY * scale,
                                  width: Double(source.width) * scale,
                                  height: Double(source.height) * scale))
let foregroundImage = foreground.makeImage()!
let background = context(size, size)
background.setFillColor(CGColor(red: 0, green: 0, blue: 0, alpha: 1))
background.fill(CGRect(x: 0, y: 0, width: size, height: size))
let backgroundImage = background.makeImage()!
// A conventional flat startup icon and a preview of the central two-thirds.
let flat = context(512, 512)
flat.setFillColor(CGColor(red: 0, green: 0, blue: 0, alpha: 1))
flat.fill(CGRect(x: 0, y: 0, width: 512, height: 512))
let flatScale = scale * 768 / Double(size)
flat.draw(source, in: CGRect(x: 256 - centerX * flatScale, y: 256 - centerY * flatScale,
                            width: Double(source.width) * flatScale,
                            height: Double(source.height) * flatScale))
let flatImage = flat.makeImage()!
func save(_ image: CGImage, _ relativePath: String) {
    let url = root.appendingPathComponent(relativePath)
    try! FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                             withIntermediateDirectories: true)
    guard let encoder = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        fatalError("Cannot create PNG: \(url.path)")
    }
    CGImageDestinationAddImage(encoder, image, nil)
    guard CGImageDestinationFinalize(encoder) else { fatalError("Cannot write PNG") }
}
let mediaRoots = ["harmony/AppScope/resources/base/media", "harmony/entry/src/main/resources/base/media"]
for media in mediaRoots {
    save(foregroundImage, media + "/app_icon_gta5_foreground.png")
    save(backgroundImage, media + "/app_icon_gta5_background.png")
    save(flatImage, media + "/app_icon_gta5.png")
}
save(flatImage, "docs/assets/app-icon-gta5-layered-preview.png")
print("Exported 288×288 layers and 512×512 flat preview; source alpha bounds: \(left),\(bottom),\(right),\(top); scale: \(scale)")
