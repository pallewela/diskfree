#!/usr/bin/env swift
// Renders the diskfree app icon at any pixel size, straight from code.
//
// All geometry is expressed on a 1024pt design canvas and scaled, so every
// size in the .icns is drawn at native resolution instead of being downsampled.
//
// Usage:
//   swift scripts/make-icon.swift iconset <out-dir>   # PNGs for iconutil
//   swift scripts/make-icon.swift png <size> <file>   # one PNG
//   swift scripts/make-icon.swift preview <file>      # light/dark size sheet

import CoreGraphics
import Foundation
import ImageIO

// MARK: - Canvas constants (1024 pt design space)

let canvas: CGFloat = 1024
let bodyInset: CGFloat = 100
let bodyRect = CGRect(x: bodyInset, y: bodyInset,
                      width: canvas - bodyInset * 2, height: canvas - bodyInset * 2)

/// Platter: a full disc with one slice cut away at the upper right.
let discCenter = CGPoint(x: 512, y: 504)
let discRadius: CGFloat = 268
let cutFrom: CGFloat = 12       // degrees, clockwise from 12 o'clock
let cutTo: CGFloat = 78
let cutInner: CGFloat = 0       // the slice runs to the centre

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: a)
}

let mintLight = rgb(0x74F5CD)
let mintDeep = rgb(0x21C79A)

// MARK: - Geometry helpers

func point(_ center: CGPoint, _ radius: CGFloat, _ deg: CGFloat) -> CGPoint {
    let t = deg * .pi / 180
    return CGPoint(x: center.x + radius * sin(t), y: center.y - radius * cos(t))
}

/// Superellipse standing in for Apple's continuous-corner squircle.
func squircle(_ rect: CGRect, n: CGFloat = 4.6, samples: Int = 720) -> CGPath {
    let a = rect.width / 2, b = rect.height / 2
    let p = CGMutablePath()
    for i in 0...samples {
        let t = CGFloat(i) / CGFloat(samples) * 2 * .pi
        let ct = cos(t), st = sin(t)
        let x = rect.midX + a * copysign(pow(abs(ct), 2 / n), ct)
        let y = rect.midY + b * copysign(pow(abs(st), 2 / n), st)
        if i == 0 { p.move(to: CGPoint(x: x, y: y)) } else { p.addLine(to: CGPoint(x: x, y: y)) }
    }
    p.closeSubpath()
    return p
}

/// Arc along a circle, clockwise from `from` to `to` degrees.
func arcPoints(center: CGPoint, radius: CGFloat, from: CGFloat, to: CGFloat) -> [CGPoint] {
    let steps = max(16, Int(abs(to - from) * 1.5))
    return (0...steps).map { i in
        point(center, radius, from + (to - from) * CGFloat(i) / CGFloat(steps))
    }
}

/// The removed slice: outer arc plus two straight cut faces meeting at the centre.
func cutPath(center: CGPoint, outer: CGFloat, inner: CGFloat,
             from: CGFloat, to: CGFloat) -> CGPath {
    let p = CGMutablePath()
    let pts = arcPoints(center: center, radius: outer, from: from, to: to)
    p.move(to: pts[0])
    for pt in pts.dropFirst() { p.addLine(to: pt) }
    p.addLine(to: point(center, inner, to))
    if inner > 1 {
        let back = arcPoints(center: center, radius: inner, from: to, to: from)
        for pt in back.dropFirst() { p.addLine(to: pt) }
    }
    p.closeSubpath()
    return p
}

/// The platter itself: a disc with the slice already taken out (one closed path).
func platterPath(center: CGPoint, radius: CGFloat,
                 from: CGFloat, to: CGFloat) -> CGPath {
    let p = CGMutablePath()
    let pts = arcPoints(center: center, radius: radius, from: to, to: from + 360)
    p.move(to: pts[0])
    for pt in pts.dropFirst() { p.addLine(to: pt) }
    p.addLine(to: center)
    p.closeSubpath()
    return p
}

func circlePath(_ center: CGPoint, _ r: CGFloat) -> CGPath {
    CGPath(ellipseIn: CGRect(x: center.x - r, y: center.y - r, width: r * 2, height: r * 2), transform: nil)
}

// MARK: - Drawing

func drawIcon(in ctx: CGContext, pixelWidth: Int, flat: Bool = false) {
    let s = CGFloat(pixelWidth) / canvas
    // y-down design space: (0,0) is the top-left of the 1024 pt canvas.
    ctx.translateBy(x: 0, y: CGFloat(pixelWidth))
    ctx.scaleBy(x: s, y: -s)
    ctx.setShouldAntialias(true)
    ctx.interpolationQuality = .high

    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let small = pixelWidth <= 32
    let tiny = pixelWidth <= 64
    let body = squircle(bodyRect)
    let discR = small ? discRadius * 1.06 : discRadius
    let cutA = small ? cutFrom - 7 : cutFrom
    let cutB = small ? cutTo + 7 : cutTo

    // --- Slab -------------------------------------------------------------
    if !flat {
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: 16), blur: 30,
                  color: CGColor(srgbRed: 0, green: 0, blue: 0, alpha: small ? 0.4 : 0.5))
    ctx.addPath(body)
    ctx.setFillColor(CGColor(srgbRed: 0.02, green: 0.02, blue: 0.03, alpha: 1))
    ctx.fillPath()
    ctx.restoreGState()

    ctx.saveGState()
    ctx.addPath(body)
    ctx.clip()
    let slab = CGGradient(colorsSpace: space,
                          colors: [rgb(0x2C3540), rgb(0x0C1016)] as CFArray,
                          locations: [0, 1])!
    ctx.drawLinearGradient(slab,
                           start: CGPoint(x: 512, y: bodyRect.minY),
                           end: CGPoint(x: 512, y: bodyRect.maxY),
                           options: [])
    if !small {
        let sheen = CGGradient(colorsSpace: space,
                               colors: [CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.09),
                                        CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.0)] as CFArray,
                               locations: [0, 1])!
        ctx.drawRadialGradient(sheen,
                               startCenter: CGPoint(x: 280, y: 230), startRadius: 0,
                               endCenter: CGPoint(x: 280, y: 230), endRadius: 660,
                               options: [])
    }
    let vignette = CGGradient(colorsSpace: space,
                              colors: [CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 0),
                                       CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 0.4)] as CFArray,
                              locations: [0.5, 1])!
    ctx.drawLinearGradient(vignette,
                           start: CGPoint(x: 512, y: 520),
                           end: CGPoint(x: 512, y: bodyRect.maxY),
                           options: [])
    ctx.restoreGState()

    // Rim light on the inner edge of the slab.
    ctx.saveGState()
    ctx.addPath(squircle(bodyRect.insetBy(dx: 1.5, dy: 1.5)))
    ctx.setLineWidth(3)
    ctx.setStrokeColor(CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.12))
    ctx.strokePath()
    ctx.restoreGState()
    } else {
        // Plate-aware variant: transparent background, glyph only.
        // The system draws legacy .icns artwork at ~80% inside its glass plate,
        // so the glyph is drawn larger to end up the usual size on the tile.
        ctx.translateBy(x: 512, y: 512)
        ctx.scaleBy(x: 1.45, y: 1.45)
        ctx.translateBy(x: -512, y: -512)
    }

    // --- Platter ----------------------------------------------------------
    let platter = platterPath(center: discCenter, radius: discR, from: cutA, to: cutB)
    let cut = cutPath(center: discCenter, outer: discR, inner: cutInner,
                      from: cutA, to: cutB)

    // The slice sits in a shallow recess, so the notch reads as depth, not paint.
    ctx.saveGState()
    ctx.addPath(cut)
    ctx.clip()
    let recess = CGGradient(colorsSpace: space,
                            colors: [CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 0.5),
                                     CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 0.0)] as CFArray,
                            locations: [0, 0.9])!
    ctx.drawRadialGradient(recess,
                           startCenter: discCenter, startRadius: cutInner,
                           endCenter: discCenter, endRadius: discRadius,
                           options: [])
    ctx.restoreGState()

    // Shadow under the platter. Filled even-odd so the slice stays empty.
    ctx.saveGState()
    if !small {
        ctx.setShadow(offset: CGSize(width: 0, height: 12), blur: 22,
                      color: CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 0.45))
    }
    ctx.addPath(platter)
    ctx.setFillColor(rgb(0x2A6CF0))
    ctx.fillPath()
    ctx.restoreGState()

    // Platter body: cool blue glass, lit from the upper left.
    ctx.saveGState()
    ctx.addPath(platter)
    ctx.clip()
    let face = CGGradient(colorsSpace: space,
                          colors: [rgb(0x63D9FA), rgb(0x2F7BF5), rgb(0x17389F)] as CFArray,
                          locations: [0, 0.48, 1])!
    ctx.drawLinearGradient(face,
                           start: CGPoint(x: discCenter.x - discRadius * 0.8, y: discCenter.y - discRadius),
                           end: CGPoint(x: discCenter.x + discRadius * 0.7, y: discCenter.y + discRadius),
                           options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])

    // Specular bloom.
    let bloom = CGGradient(colorsSpace: space,
                           colors: [CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.20),
                                    CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.0)] as CFArray,
                           locations: [0, 1])!
    ctx.drawRadialGradient(bloom,
                           startCenter: CGPoint(x: discCenter.x - 80, y: discCenter.y - 110), startRadius: 0,
                           endCenter: CGPoint(x: discCenter.x - 80, y: discCenter.y - 110), endRadius: discRadius * 0.85,
                           options: [])

    // Platter grooves: only at sizes where they can actually be seen.
    ctx.restoreGState()

    // Thick glass rim: bright on the lit side, faint opposite.
    ctx.saveGState()
    ctx.addPath(platter)
    ctx.clip()
    ctx.addPath(strokedPath(circlePath(discCenter, discR - 2.5), width: 5))
    ctx.clip()
    let rim = CGGradient(colorsSpace: space,
                         colors: [CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.55),
                                  CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.10)] as CFArray,
                         locations: [0, 1])!
    ctx.drawLinearGradient(rim,
                           start: CGPoint(x: discCenter.x - discRadius, y: discCenter.y - discRadius),
                           end: CGPoint(x: discCenter.x + discRadius, y: discCenter.y + discRadius),
                           options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    ctx.restoreGState()

    // --- The cut ----------------------------------------------------------
    // Mint contour marks the space you get back.
    let lineWidth: CGFloat = pixelWidth <= 32 ? 34 : (pixelWidth <= 64 ? 20 : 10)
    let edgeFrom = arcPoints(center: discCenter, radius: max(cutInner, 1), from: cutA, to: cutB)
    let contour = CGMutablePath()
    contour.move(to: point(discCenter, discR, cutA))
    contour.addLine(to: edgeFrom[0])
    for pt in edgeFrom.dropFirst() { contour.addLine(to: pt) }
    contour.addLine(to: point(discCenter, discR, cutB))

    // Cross-section along the cut: a narrow facet reads as material thickness.
    if !tiny {
        ctx.saveGState()
        ctx.addPath(platter)
        ctx.clip()
        ctx.addPath(contour)
        ctx.setLineWidth(lineWidth + 26)
        ctx.setLineJoin(.round)
        ctx.setLineCap(.round)
        ctx.setStrokeColor(CGColor(srgbRed: 0.06, green: 0.13, blue: 0.42, alpha: 0.16))
        ctx.strokePath()
        ctx.restoreGState()
    }

    // Inner shade along the cut: the platter is solid, the slice was taken out of it.
    if !tiny {
        ctx.saveGState()
        ctx.addPath(platter)
        ctx.clip()
        ctx.setShadow(offset: .zero, blur: 18,
                      color: CGColor(srgbRed: 0.02, green: 0.05, blue: 0.14, alpha: 0.75))
        ctx.addPath(contour)
        ctx.setLineWidth(lineWidth + 6)
        ctx.setLineJoin(.round)
        ctx.setLineCap(.round)
        ctx.setStrokeColor(CGColor(srgbRed: 0.02, green: 0.05, blue: 0.14, alpha: 0.55))
        ctx.strokePath()
        ctx.restoreGState()

        ctx.saveGState()
        ctx.setShadow(offset: .zero, blur: 16,
                      color: CGColor(srgbRed: 0.30, green: 0.95, blue: 0.76, alpha: 0.65))
        ctx.addPath(contour)
        ctx.setLineWidth(lineWidth + 4)
        ctx.setLineJoin(.round)
        ctx.setLineCap(.round)
        ctx.setStrokeColor(CGColor(srgbRed: 0.30, green: 0.95, blue: 0.76, alpha: 0.55))
        ctx.strokePath()
        ctx.restoreGState()
    }

    ctx.saveGState()
    ctx.addPath(platter)
    ctx.clip()
    ctx.addPath(contour)
    ctx.setLineWidth(lineWidth)
    ctx.setLineJoin(.round)
    ctx.setLineCap(.round)
    ctx.replacePathWithStrokedPath()
    ctx.clip()
    let mint = CGGradient(colorsSpace: space,
                          colors: [mintLight, mintDeep, mintLight] as CFArray,
                          locations: [0, 0.55, 1])!
    ctx.drawLinearGradient(mint,
                           start: CGPoint(x: discCenter.x, y: discCenter.y - discRadius),
                           end: CGPoint(x: discCenter.x, y: discCenter.y + discRadius),
                           options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    ctx.restoreGState()
}

func strokedPath(_ path: CGPath, width: CGFloat) -> CGPath {
    path.copy(strokingWithWidth: width, lineCap: .round, lineJoin: .round, miterLimit: 10)
}

// MARK: - Output

func render(pixelWidth: Int, flat: Bool = false) -> CGImage {
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let ctx = CGContext(data: nil, width: pixelWidth, height: pixelWidth,
                        bitsPerComponent: 8, bytesPerRow: 0, space: space,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    drawIcon(in: ctx, pixelWidth: pixelWidth, flat: flat)
    return ctx.makeImage()!
}

func writePNG(_ image: CGImage, to url: URL) {
    guard let dest = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil) else {
        fatalError("cannot write \(url.path)")
    }
    CGImageDestinationAddImage(dest, image, nil)
    guard CGImageDestinationFinalize(dest) else { fatalError("cannot finalize \(url.path)") }
}

func makePreview(to url: URL) {
    let w = 1920, h = 1180
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                        space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.translateBy(x: 0, y: CGFloat(h))
    ctx.scaleBy(x: 1, y: -1)

    ctx.setFillColor(rgb(0xF3F2EE))
    ctx.fill(CGRect(x: 0, y: 0, width: CGFloat(w) / 2, height: CGFloat(h)))
    ctx.setFillColor(rgb(0x1A1C20))
    ctx.fill(CGRect(x: CGFloat(w) / 2, y: 0, width: CGFloat(w) / 2, height: CGFloat(h)))
    ctx.setFillColor(rgb(0xD8D6D0))
    ctx.fill(CGRect(x: CGFloat(w) / 2 - 1, y: 0, width: 2, height: CGFloat(h)))

    for panel in 0...1 {
        let x = CGFloat(panel) * CGFloat(w) / 2 + 40
        ctx.draw(render(pixelWidth: 512), in: CGRect(x: x + 24, y: 56, width: 512, height: 512))
        var rowX = x + 40
        for size in [128, 64, 32, 16] {
            ctx.draw(render(pixelWidth: size),
                     in: CGRect(x: rowX, y: 660, width: CGFloat(size), height: CGFloat(size)))
            rowX += CGFloat(size) + 60
        }
        var dockX = x + 40
        for _ in 0..<4 {
            ctx.draw(render(pixelWidth: 128), in: CGRect(x: dockX, y: 800, width: 128, height: 128))
            dockX += 152
        }
    }
    writePNG(ctx.makeImage()!, to: url)
}

let args = CommandLine.arguments
guard args.count >= 2 else {
    print("usage: make-icon.swift iconset <dir> | png <size> <file> | preview <file>")
    exit(2)
}

switch args[1] {
case "iconset", "iconsetflat":
    let dir = URL(fileURLWithPath: args[2])
    let flat = args[1] == "iconsetflat"
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let names: [(String, Int)] = [
        ("icon_16x16.png", 16), ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32), ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128), ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256), ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512), ("icon_512x512@2x.png", 1024),
    ]
    for (name, size) in names {
        writePNG(render(pixelWidth: size, flat: flat), to: dir.appendingPathComponent(name))
    }
    print("wrote \(names.count) PNGs to \(dir.path)")
case "png":
    let size = Int(args[2])!
    writePNG(render(pixelWidth: size), to: URL(fileURLWithPath: args[3]))
    print("wrote \(args[3]) at \(size)x\(size)")
case "pngflat":
    let size = Int(args[2])!
    writePNG(render(pixelWidth: size, flat: true), to: URL(fileURLWithPath: args[3]))
    print("wrote \(args[3]) at \(size)x\(size) (flat)")
case "preview":
    makePreview(to: URL(fileURLWithPath: args[2]))
    print("wrote \(args[2])")
default:
    print("unknown mode \(args[1])")
    exit(2)
}
