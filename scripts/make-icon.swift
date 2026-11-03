// Renders the app icon into an .iconset and builds AppIcon.icns with iconutil.
//   swift scripts/make-icon.swift crates/omp-desktop/assets/AppIcon.icns
// A blue rounded square (the design's accent, #123BB5 → #3B6CF0) with a
// white terminal prompt "›_". Re-run after changing the drawing; the .icns
// is committed so bundling needs no Swift.
import AppKit

let output = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "AppIcon.icns"
let iconset = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("AppIcon.iconset")
try? FileManager.default.removeItem(at: iconset)
try! FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

func color(_ hex: UInt32) -> CGColor {
	CGColor(red: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: 1)
}

func render(_ px: Int) -> Data {
	let s = CGFloat(px)
	let ctx = CGContext(data: nil, width: px, height: px, bitsPerComponent: 8, bytesPerRow: 0,
		space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
	// macOS icon grid: 824/1024 body, centred, corner radius ~185/1024.
	let inset = s * 100 / 1024
	let body = CGRect(x: inset, y: inset, width: s - 2 * inset, height: s - 2 * inset)
	let shape = CGPath(roundedRect: body, cornerWidth: s * 185 / 1024, cornerHeight: s * 185 / 1024, transform: nil)
	ctx.saveGState()
	ctx.setShadow(offset: CGSize(width: 0, height: -s * 10 / 1024), blur: s * 28 / 1024, color: CGColor(gray: 0, alpha: 0.3))
	ctx.addPath(shape)
	ctx.setFillColor(color(0x123BB5))
	ctx.fillPath()
	ctx.restoreGState()
	ctx.saveGState()
	ctx.addPath(shape)
	ctx.clip()
	let gradient = CGGradient(colorsSpace: nil, colors: [color(0x3B6CF0), color(0x123BB5)] as CFArray, locations: [0, 1])!
	ctx.drawLinearGradient(gradient, start: CGPoint(x: 0, y: body.maxY), end: CGPoint(x: 0, y: body.minY), options: [])
	ctx.restoreGState()
	// "›" chevron and "_" cursor.
	ctx.setStrokeColor(CGColor(gray: 1, alpha: 1))
	ctx.setLineWidth(s * 78 / 1024)
	ctx.setLineCap(.round)
	ctx.setLineJoin(.round)
	let u = s / 1024
	ctx.move(to: CGPoint(x: 300 * u, y: 680 * u))
	ctx.addLine(to: CGPoint(x: 480 * u, y: 512 * u))
	ctx.addLine(to: CGPoint(x: 300 * u, y: 344 * u))
	ctx.strokePath()
	ctx.move(to: CGPoint(x: 560 * u, y: 344 * u))
	ctx.addLine(to: CGPoint(x: 740 * u, y: 344 * u))
	ctx.strokePath()
	let rep = NSBitmapImageRep(cgImage: ctx.makeImage()!)
	return rep.representation(using: .png, properties: [:])!
}

for points in [16, 32, 128, 256, 512] {
	for scale in [1, 2] {
		let name = scale == 1 ? "icon_\(points)x\(points).png" : "icon_\(points)x\(points)@2x.png"
		try! render(points * scale).write(to: iconset.appendingPathComponent(name))
	}
}
let task = Process()
task.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
task.arguments = ["-c", "icns", iconset.path, "-o", output]
try! task.run()
task.waitUntilExit()
exit(task.terminationStatus)
