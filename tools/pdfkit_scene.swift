import AppKit
import Foundation
import PDFKit

struct LineRecord {
    let x: CGFloat
    let y: CGFloat
    let width: CGFloat
    let height: CGFloat
    let fontName: String
    let fontSize: Double
    let text: String
}

struct PageRecord {
    let number: Int
    let bounds: CGRect
    let lines: [LineRecord]
}

func escapeField(_ text: String) -> String {
    var escaped = ""
    escaped.reserveCapacity(text.count)

    for scalar in text.unicodeScalars {
        switch scalar {
        case "\\":
            escaped.append("\\\\")
        case "\t":
            escaped.append("\\t")
        case "\n":
            escaped.append("\\n")
        case "\r":
            escaped.append("\\r")
        default:
            escaped.unicodeScalars.append(scalar)
        }
    }

    return escaped
}

func renderPage(_ page: PDFPage, to outputURL: URL, scale: CGFloat) throws {
    let bounds = page.bounds(for: .mediaBox)
    let width = Int((bounds.width * scale).rounded(.up))
    let height = Int((bounds.height * scale).rounded(.up))
    let image = NSImage(size: NSSize(width: width, height: height))

    image.lockFocusFlipped(false)
    NSColor.white.set()
    NSRect(x: 0, y: 0, width: width, height: height).fill()
    let context = NSGraphicsContext.current!.cgContext
    context.scaleBy(x: scale, y: scale)
    page.draw(with: .mediaBox, to: context)
    image.unlockFocus()

    guard let tiff = image.tiffRepresentation,
          let bitmap = NSBitmapImageRep(data: tiff),
          let png = bitmap.representation(using: .png, properties: [:])
    else {
        throw NSError(domain: "pdfkit_scene", code: 1, userInfo: [NSLocalizedDescriptionKey: "failed to encode PNG"])
    }

    try png.write(to: outputURL)
}

let arguments = CommandLine.arguments
guard arguments.count >= 3 else {
    fputs("usage: pdfkit_scene.swift <INPUT_PDF> <OUTPUT_DIR> [SCALE]\n", stderr)
    exit(2)
}

let inputURL = URL(fileURLWithPath: arguments[1])
let outputDir = URL(fileURLWithPath: arguments[2], isDirectory: true)
let scale = arguments.count >= 4 ? CGFloat(Double(arguments[3]) ?? 2.0) : 2.0
let renderPages: Set<Int> = {
    guard arguments.count >= 5 else { return [] }
    return Set(
        arguments[4]
            .split(separator: ",")
            .compactMap { Int($0) }
    )
}()

guard let document = PDFDocument(url: inputURL) else {
    fputs("failed to open PDF\n", stderr)
    exit(1)
}

try FileManager.default.createDirectory(at: outputDir, withIntermediateDirectories: true)

var pageRecords: [PageRecord] = []

for pageIndex in 0..<document.pageCount {
    guard let page = document.page(at: pageIndex) else {
        continue
    }

    let pageNumber = pageIndex + 1
    let bounds = page.bounds(for: .mediaBox)
    guard let attributed = page.attributedString, let selection = page.selection(for: bounds) else {
        pageRecords.append(PageRecord(number: pageNumber, bounds: bounds, lines: []))
        continue
    }

    let pageText = attributed.string as NSString
    var cursor = 0
    var lines: [LineRecord] = []

    for lineSelection in selection.selectionsByLine() {
        let raw = (lineSelection.string ?? "").replacingOccurrences(of: "\n", with: " ")
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty {
            continue
        }

        let searchStart = min(cursor, pageText.length)
        let searchRange = NSRange(location: searchStart, length: pageText.length - searchStart)
        var range = pageText.range(of: raw, options: [], range: searchRange)
        if range.location == NSNotFound {
            range = pageText.range(of: text, options: [], range: searchRange)
        }
        if range.location == NSNotFound {
            range = pageText.range(of: text)
        }
        if range.location == NSNotFound {
            continue
        }

        cursor = range.location + range.length

        let attributes = attributed.attributes(at: range.location, effectiveRange: nil)
        let font = attributes[.font] as? NSFont
        let fontName = font?.fontName ?? "Times-Roman"
        let fontSize = Double(font?.pointSize ?? lineSelection.bounds(for: page).height)
        let lineBounds = lineSelection.bounds(for: page)
        lines.append(
            LineRecord(
                x: lineBounds.origin.x,
                y: lineBounds.origin.y,
                width: lineBounds.size.width,
                height: lineBounds.size.height,
                fontName: fontName,
                fontSize: fontSize,
                text: text
            )
        )
    }

    pageRecords.append(PageRecord(number: pageNumber, bounds: bounds, lines: lines))
}

if !pageRecords.contains(where: { !$0.lines.isEmpty }) {
    exit(0)
}

for record in pageRecords {
    guard let page = document.page(at: record.number - 1) else {
        continue
    }

    let renderURL = outputDir.appendingPathComponent(String(format: "page-%04d.png", record.number))
    if renderPages.contains(record.number) {
        try renderPage(page, to: renderURL, scale: scale)
    }
    let renderPath = renderPages.contains(record.number) ? renderURL.path : ""
    print("PAGE\t\(record.number)\t\(record.bounds.width)\t\(record.bounds.height)\t\(escapeField(renderPath))")

    for line in record.lines {
        print(
            "LINE\t\(record.number)\t\(line.x)\t\(line.y)\t\(line.width)\t\(line.height)\t\(line.fontSize)\t\(escapeField(line.fontName))\t\(escapeField(line.text))"
        )
    }
}
