// Rasterize the SF Symbols native menu rows use (src/components/ui.tsx,
// RowMenuItem.symbol) into template PNGs. Run once, commit the output;
// rerun only when a menu gains a symbol the set lacks:
//
//   swift scripts/menu-symbols.swift            # the list below
//   swift scripts/menu-symbols.swift trash link # just these
//
// Black glyph on clear at 2×, drawn with the menu's own symbol
// configuration (13 pt, regular, medium scale), centered on a 20×16 pt
// canvas so every row's label starts on the same column. The app marks each image
// as a template (src-tauri/src/menuicons.rs), so AppKit tints it for the
// appearance and inverts it under the highlight — one file per symbol.
import AppKit

let defaults = [
  "pencil", "square.and.arrow.down", "square.and.arrow.up", "externaldrive",
  "externaldrive.badge.xmark", "folder", "archivebox", "trash",
  "arrow.clockwise", "photo", "arrow.up.right.square", "link", "tag",
  "note.text", "doc.on.doc", "macwindow", "slider.horizontal.3", "doc.text",
  "play", "books.vertical", "text.bubble",
]
let names = CommandLine.arguments.count > 1 ? Array(CommandLine.arguments.dropFirst()) : defaults
let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let out = root.appendingPathComponent("src/assets/menu-symbols")
try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
let config = NSImage.SymbolConfiguration(pointSize: 13, weight: .regular, scale: .medium)
var missing: [String] = []
var done = 0
for name in names {
  guard let base = NSImage(systemSymbolName: name, accessibilityDescription: nil),
        let image = base.withSymbolConfiguration(config)
  else { missing.append(name); continue }
  // Every glyph on one canvas size, centered: NSMenu sets the text column
  // by each row's image width, so natural sizes would jog the labels.
  let glyph = image.size
  let canvas = NSSize(width: 20, height: 16)
  let scale: CGFloat = 2
  let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(canvas.width * scale), pixelsHigh: Int(canvas.height * scale),
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
  rep.size = canvas
  NSGraphicsContext.saveGraphicsState()
  NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
  NSColor.black.set()
  let fit = min(1, min(canvas.width / glyph.width, canvas.height / glyph.height))
  let drawn = NSSize(width: glyph.width * fit, height: glyph.height * fit)
  let origin = NSPoint(x: (canvas.width - drawn.width) / 2, y: (canvas.height - drawn.height) / 2)
  image.draw(in: NSRect(origin: origin, size: drawn), from: .zero, operation: .sourceOver, fraction: 1)
  NSGraphicsContext.restoreGraphicsState()
  let png = rep.representation(using: .png, properties: [:])!
  try! png.write(to: out.appendingPathComponent("\(name).png"))
  done += 1
}
print("\(done) symbols → \(out.path)")
if !missing.isEmpty { print("not an SF Symbol on this macOS: \(missing.joined(separator: ", "))") }
