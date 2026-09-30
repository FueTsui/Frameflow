import AppKit

guard CommandLine.arguments.count == 2 else {
    fatalError("Usage: swift macos-icon.swift output.png")
}

let size = 1024
guard let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: size,
    pixelsHigh: size,
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
), let context = NSGraphicsContext(bitmapImageRep: bitmap) else {
    fatalError("Cannot create icon canvas")
}

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = context
context.cgContext.clear(CGRect(x: 0, y: 0, width: CGFloat(size), height: CGFloat(size)))
context.cgContext.scaleBy(x: 8, y: 8)
NSColor(calibratedRed: 241.0 / 255, green: 246.0 / 255, blue: 252.0 / 255, alpha: 1).setFill()
NSBezierPath(roundedRect: NSRect(x: 8, y: 8, width: 112, height: 112), xRadius: 27, yRadius: 27).fill()
NSColor(calibratedRed: 0, green: 120.0 / 255, blue: 211.0 / 255, alpha: 1).setStroke()

// Convert the SVG's downward y-axis into AppKit's upward y-axis.
func point(_ x: CGFloat, _ y: CGFloat) -> NSPoint {
    return NSPoint(x: x, y: 128 - y)
}

let frame = NSBezierPath()
frame.lineWidth = 6
frame.lineCapStyle = .round
frame.move(to: point(69, 35))
frame.line(to: point(43, 35))
frame.curve(to: point(35, 43), controlPoint1: point(38.58, 35), controlPoint2: point(35, 38.58))
frame.line(to: point(35, 85))
frame.curve(to: point(43, 93), controlPoint1: point(35, 89.42), controlPoint2: point(38.58, 93))
frame.line(to: point(85, 93))
frame.curve(to: point(93, 85), controlPoint1: point(89.42, 93), controlPoint2: point(93, 89.42))
frame.line(to: point(93, 59))
frame.stroke()

let play = NSBezierPath()
play.lineWidth = 5
play.lineJoinStyle = .round
play.move(to: point(58, 53))
play.line(to: point(81, 67))
play.line(to: point(58, 81))
play.close()
play.stroke()

let plus = NSBezierPath()
plus.lineWidth = 5
plus.lineCapStyle = .round
plus.move(to: point(84, 35))
plus.line(to: point(97, 35))
plus.move(to: point(90.5, 28.5))
plus.line(to: point(90.5, 41.5))
plus.stroke()
NSGraphicsContext.restoreGraphicsState()

guard let png = bitmap.representation(using: .png, properties: [:]) else {
    fatalError("Cannot encode icon as PNG")
}
try png.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
