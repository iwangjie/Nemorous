#!/usr/bin/env swift
import AppKit
import CoreGraphics
import Foundation

struct Palette {
    let bgTop: (CGFloat, CGFloat, CGFloat, CGFloat)
    let bgBot: (CGFloat, CGFloat, CGFloat, CGFloat)
    let border: (CGFloat, CGFloat, CGFloat, CGFloat)
    let stroke: (CGFloat, CGFloat, CGFloat, CGFloat)
    let muted: (CGFloat, CGFloat, CGFloat, CGFloat)
    let accent: (CGFloat, CGFloat, CGFloat, CGFloat)
}

func rgba(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1.0) -> (CGFloat, CGFloat, CGFloat, CGFloat) {
    (r / 255.0, g / 255.0, b / 255.0, a)
}

let darkPalette = Palette(
    bgTop: rgba(26, 28, 35),
    bgBot: rgba(13, 14, 18),
    border: (1, 1, 1, 0.14),
    stroke: rgba(245, 247, 250),
    muted: rgba(245, 247, 250, 0.30),
    accent: rgba(56, 189, 248)
)

let lightPalette = Palette(
    bgTop: rgba(255, 255, 255),
    bgBot: rgba(239, 241, 245),
    border: rgba(17, 19, 24, 0.11),
    stroke: rgba(17, 19, 24),
    muted: rgba(17, 19, 24, 0.25),
    accent: rgba(17, 19, 24)
)

func cgColor(_ c: (CGFloat, CGFloat, CGFloat, CGFloat)) -> CGColor {
    CGColor(colorSpace: CGColorSpaceCreateDeviceRGB(), components: [c.0, c.1, c.2, c.3])!
}

// Apple continuous-curvature squircle on a 0..256 coordinate space (top-left origin).
func squirclePath256() -> CGPath {
    let p = CGMutablePath()
    p.move(to: CGPoint(x: 76, y: 0))
    p.addLine(to: CGPoint(x: 180, y: 0))
    p.addCurve(to: CGPoint(x: 256, y: 76), control1: CGPoint(x: 237.5, y: 0), control2: CGPoint(x: 256, y: 18.5))
    p.addLine(to: CGPoint(x: 256, y: 180))
    p.addCurve(to: CGPoint(x: 180, y: 256), control1: CGPoint(x: 256, y: 237.5), control2: CGPoint(x: 237.5, y: 256))
    p.addLine(to: CGPoint(x: 76, y: 256))
    p.addCurve(to: CGPoint(x: 0, y: 180), control1: CGPoint(x: 18.5, y: 256), control2: CGPoint(x: 0, y: 237.5))
    p.addLine(to: CGPoint(x: 0, y: 76))
    p.addCurve(to: CGPoint(x: 76, y: 0), control1: CGPoint(x: 0, y: 18.5), control2: CGPoint(x: 18.5, y: 0))
    p.closeSubpath()
    return p
}

func strokeLine(_ ctx: CGContext, _ x1: CGFloat, _ y1: CGFloat, _ x2: CGFloat, _ y2: CGFloat, width: CGFloat, color: (CGFloat, CGFloat, CGFloat, CGFloat)) {
    ctx.setStrokeColor(cgColor(color))
    ctx.setLineWidth(width)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.beginPath()
    ctx.move(to: CGPoint(x: x1, y: y1))
    ctx.addLine(to: CGPoint(x: x2, y: y2))
    ctx.strokePath()
}

func fillCircle(_ ctx: CGContext, cx: CGFloat, cy: CGFloat, r: CGFloat, color: (CGFloat, CGFloat, CGFloat, CGFloat)) {
    ctx.setFillColor(cgColor(color))
    ctx.fillEllipse(in: CGRect(x: cx - r, y: cy - r, width: r * 2, height: r * 2))
}

func strokeCircle(_ ctx: CGContext, cx: CGFloat, cy: CGFloat, r: CGFloat, width: CGFloat, color: (CGFloat, CGFloat, CGFloat, CGFloat)) {
    ctx.setStrokeColor(cgColor(color))
    ctx.setLineWidth(width)
    ctx.strokeEllipse(in: CGRect(x: cx - r, y: cy - r, width: r * 2, height: r * 2))
}

func drawMark(_ ctx: CGContext, id: String, p: Palette) {
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)

    switch id {
    case "aperture":
        // 4 minimal diagonal calibration ticks
        strokeLine(ctx, 64, 64, 82, 82, width: 10, color: p.muted)
        strokeLine(ctx, 192, 64, 174, 82, width: 10, color: p.muted)
        strokeLine(ctx, 64, 192, 82, 174, width: 10, color: p.muted)
        strokeLine(ctx, 192, 192, 174, 174, width: 10, color: p.muted)
        // Sculpted 4-point geometric astroid
        let star = CGMutablePath()
        star.move(to: CGPoint(x: 128, y: 58))
        star.addCurve(to: CGPoint(x: 198, y: 128), control1: CGPoint(x: 128, y: 104), control2: CGPoint(x: 152, y: 128))
        star.addCurve(to: CGPoint(x: 128, y: 198), control1: CGPoint(x: 152, y: 128), control2: CGPoint(x: 128, y: 152))
        star.addCurve(to: CGPoint(x: 58, y: 128), control1: CGPoint(x: 128, y: 152), control2: CGPoint(x: 104, y: 128))
        star.addCurve(to: CGPoint(x: 128, y: 58), control1: CGPoint(x: 104, y: 128), control2: CGPoint(x: 128, y: 104))
        star.closeSubpath()
        ctx.addPath(star)
        ctx.setStrokeColor(cgColor(p.stroke))
        ctx.setLineWidth(14)
        ctx.strokePath()
        // Center precision core
        fillCircle(ctx, cx: 128, cy: 128, r: 14, color: p.accent)

    case "slash":
        // Top-left architectural bracket
        let tl = CGMutablePath()
        tl.move(to: CGPoint(x: 68, y: 144))
        tl.addLine(to: CGPoint(x: 68, y: 78))
        tl.addCurve(to: CGPoint(x: 78, y: 68), control1: CGPoint(x: 68, y: 72.5), control2: CGPoint(x: 72.5, y: 68))
        tl.addLine(to: CGPoint(x: 144, y: 68))
        ctx.addPath(tl)
        ctx.setStrokeColor(cgColor(p.stroke))
        ctx.setLineWidth(17)
        ctx.strokePath()

        // Bottom-right mirrored bracket
        let br = CGMutablePath()
        br.move(to: CGPoint(x: 188, y: 112))
        br.addLine(to: CGPoint(x: 188, y: 178))
        br.addCurve(to: CGPoint(x: 178, y: 188), control1: CGPoint(x: 188, y: 183.5), control2: CGPoint(x: 183.5, y: 188))
        br.addLine(to: CGPoint(x: 112, y: 188))
        ctx.addPath(br)
        ctx.setStrokeColor(cgColor(p.stroke))
        ctx.setLineWidth(17)
        ctx.strokePath()

        // 45-deg precision slash
        strokeLine(ctx, 86, 170, 170, 86, width: 17, color: p.accent)

    case "prism":
        // Draw muted inner structural axes first so primary strokes sit cleanly on top
        strokeLine(ctx, 128, 128, 128, 196, width: 12, color: p.muted)
        strokeLine(ctx, 128, 128, 69, 94, width: 12, color: p.muted)

        // Open isometric hexagon (gap on upper-right edge)
        let hex = CGMutablePath()
        hex.move(to: CGPoint(x: 128, y: 58))
        hex.addLine(to: CGPoint(x: 67, y: 93))
        hex.addLine(to: CGPoint(x: 67, y: 163))
        hex.addLine(to: CGPoint(x: 128, y: 198))
        hex.addLine(to: CGPoint(x: 189, y: 163))
        hex.addLine(to: CGPoint(x: 189, y: 122))
        ctx.addPath(hex)
        ctx.setStrokeColor(cgColor(p.stroke))
        ctx.setLineWidth(14)
        ctx.strokePath()

        // Escaping accent vector beam
        strokeLine(ctx, 128, 128, 188, 93, width: 15, color: p.accent)
        fillCircle(ctx, cx: 188, cy: 93, r: 12, color: p.accent)

    case "orbit":
        // Outer open circle (~298 deg arc, opening on right side)
        let startAngle = CGFloat(31.0 * Double.pi / 180.0)
        let endAngle = CGFloat(-31.0 * Double.pi / 180.0)
        ctx.setStrokeColor(cgColor(p.stroke))
        ctx.setLineWidth(14)
        ctx.setLineCap(.round)
        ctx.beginPath()
        // In flipped (y-down) coordinates, clockwise=false sweeps from +31deg through 180deg to -31deg
        ctx.addArc(center: CGPoint(x: 128, y: 128), radius: 66, startAngle: startAngle, endAngle: endAngle, clockwise: false)
        ctx.strokePath()

        // Inner eccentric ring
        strokeCircle(ctx, cx: 116, cy: 128, r: 34, width: 13, color: p.muted)

        // Focal agent core
        fillCircle(ctx, cx: 176, cy: 128, r: 16, color: p.accent)

    case "aero":
        // Draw muted trailing balance line first so primary strokes sit cleanly on top
        strokeLine(ctx, 68, 104, 110, 146, width: 14, color: p.muted)

        // Sleek open geometric dart/wing
        let wing = CGMutablePath()
        wing.move(to: CGPoint(x: 68, y: 104))
        wing.addLine(to: CGPoint(x: 186, y: 70))
        wing.addLine(to: CGPoint(x: 152, y: 188))
        ctx.addPath(wing)
        ctx.setStrokeColor(cgColor(p.stroke))
        ctx.setLineWidth(16)
        ctx.strokePath()

        // Forward thrust accent vector
        strokeLine(ctx, 74, 182, 142, 114, width: 16, color: p.accent)

    default:
        fatalError("Unknown icon id: \(id)")
    }
}

func renderIcon(id: String, palette: Palette, size: Int, outputPath: String) {
    let s = CGFloat(size)
    let colorSpace = CGColorSpaceCreateDeviceRGB()
    let bitmapInfo = CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue)
    guard let ctx = CGContext(
        data: nil,
        width: size,
        height: size,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: colorSpace,
        bitmapInfo: bitmapInfo.rawValue
    ) else {
        fatalError("Failed to create CGContext")
    }

    ctx.setAllowsAntialiasing(true)
    ctx.setShouldAntialias(true)
    ctx.interpolationQuality = .high

    // Flip to top-left origin
    ctx.translateBy(x: 0, y: s)
    ctx.scaleBy(x: 1, y: -1)

    // Apple HIG macOS icon safe zone: 824 / 1024 squircle centered in 1024 canvas
    let pad = s * (100.0 / 1024.0)
    let box = s - pad * 2.0
    let scale = box / 256.0
    let sq = squirclePath256()

    // 1. Subtle macOS Dock drop shadow behind the squircle
    ctx.saveGState()
    ctx.setShadow(
        offset: CGSize(width: 0, height: -s * 0.012),
        blur: s * 0.026,
        color: CGColor(colorSpace: colorSpace, components: [0, 0, 0, 0.24])
    )
    ctx.translateBy(x: pad, y: pad)
    ctx.scaleBy(x: scale, y: scale)
    ctx.addPath(sq)
    ctx.setFillColor(cgColor(palette.bgBot))
    ctx.fillPath()
    ctx.restoreGState()

    // 2. Clipped squircle background gradient + vector mark + hairline border
    ctx.saveGState()
    ctx.translateBy(x: pad, y: pad)
    ctx.scaleBy(x: scale, y: scale)
    ctx.addPath(sq)
    ctx.clip()

    let grad = CGGradient(
        colorsSpace: colorSpace,
        colors: [cgColor(palette.bgTop), cgColor(palette.bgBot)] as CFArray,
        locations: [0.0, 1.0]
    )!
    ctx.drawLinearGradient(grad, start: CGPoint(x: 128, y: 0), end: CGPoint(x: 128, y: 256), options: [])

    // Draw the icon mark
    drawMark(ctx, id: id, p: palette)

    // Subtle crisp inner hairline border
    ctx.addPath(sq)
    ctx.setStrokeColor(cgColor(palette.border))
    ctx.setLineWidth(4.5)
    ctx.strokePath()
    ctx.restoreGState()

    guard let image = ctx.makeImage() else {
        fatalError("Failed to create CGImage")
    }
    let rep = NSBitmapImageRep(cgImage: image)
    guard let png = rep.representation(using: .png, properties: [:]) else {
        fatalError("Failed to encode PNG")
    }
    let url = URL(fileURLWithPath: outputPath)
    try! FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    try! png.write(to: url)
}

let cwd = FileManager.default.currentDirectoryPath
let icons = ["aperture", "slash", "prism", "orbit", "aero"]

for id in icons {
    renderIcon(id: id, palette: darkPalette, size: 512, outputPath: "\(cwd)/resources/appicons/\(id)_dark.png")
    renderIcon(id: id, palette: lightPalette, size: 512, outputPath: "\(cwd)/resources/appicons/\(id)_light.png")
}

// Default static bundle & README icons (Aperture Dark, 1024x1024)
renderIcon(id: "aperture", palette: darkPalette, size: 1024, outputPath: "\(cwd)/resources/icon.png")
renderIcon(id: "aperture", palette: darkPalette, size: 1024, outputPath: "\(cwd)/assets/icon.png")

print("Generated \(icons.count * 2) runtime app icons and default 1024x1024 icons.")
