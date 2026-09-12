import AppKit
import Foundation

guard CommandLine.arguments.count == 3 else {
    fputs("Usage: inset-icon.swift SOURCE OUTPUT\n", stderr)
    exit(64)
}

guard let source = NSImage(contentsOfFile: CommandLine.arguments[1]) else {
    fputs("Unable to read application icon.\n", stderr)
    exit(1)
}

let canvasSize = NSSize(width: 1024, height: 1024)
guard let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: Int(canvasSize.width),
    pixelsHigh: Int(canvasSize.height),
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
), let context = NSGraphicsContext(bitmapImageRep: bitmap) else {
    fputs("Unable to create icon canvas.\n", stderr)
    exit(1)
}

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = context
NSColor.clear.setFill()
NSBezierPath(rect: NSRect(origin: .zero, size: canvasSize)).fill()
source.draw(in: NSRect(x: 64, y: 64, width: 896, height: 896))
NSGraphicsContext.restoreGraphicsState()

guard let png = bitmap.representation(using: NSBitmapImageRep.FileType.png, properties: [:]) else {
    fputs("Unable to encode padded icon.\n", stderr)
    exit(1)
}

do {
    try png.write(to: URL(fileURLWithPath: CommandLine.arguments[2]))
} catch {
    fputs("Unable to write padded icon: \(error)\n", stderr)
    exit(1)
}
