import AppKit
import CoreGraphics
import Foundation
import ImageIO

@MainActor
struct ClipboardBundle {
    static let maxPNGBytes = 8 * 1024 * 1024
    static let maxImagePixels = 16_777_216
    static let maxPayloadBytes = 16 + 3 * ClipboardService.maxClipboardBytes + maxPNGBytes
    private static let extendedTypes: Set<NSPasteboard.PasteboardType> = [.rtf, .html, .png, .tiff]

    let text: Data
    let rtf: Data
    let html: Data
    let png: Data

    var hasExtendedContent: Bool {
        !rtf.isEmpty || !html.isEmpty || !png.isEmpty
    }

    static func isAvailable(in types: [NSPasteboard.PasteboardType]) -> Bool {
        !types.contains(.fileURL) && !extendedTypes.isDisjoint(with: types)
    }

    func encode() -> Data {
        let members = [text, rtf, html, png]
        var result = Data(capacity: 16 + members.reduce(0) { $0 + $1.count })
        for member in members {
            var length = UInt32(member.count).bigEndian
            withUnsafeBytes(of: &length) { result.append(contentsOf: $0) }
        }
        for member in members {
            result.append(member)
        }
        return result
    }

    static func decode(_ payload: Data) throws -> ClipboardBundle {
        guard payload.count >= 16, payload.count <= maxPayloadBytes else {
            throw ClipboardError.invalidBundle
        }

        let textLimit = ClipboardService.maxClipboardBytes
        let limits = [textLimit, textLimit, textLimit, maxPNGBytes]
        var rest = payload.dropFirst(16)
        var members: [Data] = []
        for (index, limit) in limits.enumerated() {
            let count = payload.dropFirst(4 * index).prefix(4).reduce(0) {
                ($0 << 8) | Int($1)
            }
            guard count <= limit, count <= rest.count else {
                throw ClipboardError.invalidBundle
            }
            members.append(Data(rest.prefix(count)))
            rest = rest.dropFirst(count)
        }

        let bundle = ClipboardBundle(text: members[0], rtf: members[1], html: members[2], png: members[3])
        guard rest.isEmpty, bundle.hasExtendedContent,
              bundle.text.isEmpty || validText(bundle.text),
              bundle.html.isEmpty || validText(bundle.html),
              bundle.rtf.isEmpty || validRTF(bundle.rtf),
              bundle.png.isEmpty || isPNG(bundle.png)
        else { throw ClipboardError.invalidBundle }
        return bundle
    }

    static func read(expectedChangeCount: Int) throws -> ClipboardBundle {
        let pasteboard = NSPasteboard.general
        guard pasteboard.changeCount == expectedChangeCount else {
            throw ClipboardError.changedDuringRead
        }
        guard let item = pasteboard.pasteboardItems?.first,
              isAvailable(in: item.types)
        else {
            if pasteboard.changeCount != expectedChangeCount {
                throw ClipboardError.changedDuringRead
            }
            throw ClipboardError.noText
        }

        let text = item.string(forType: .string).map { Data($0.utf8) } ?? Data()
        let rtf = item.data(forType: .rtf) ?? Data()
        let html = item.data(forType: .html) ?? Data()
        let bundle = ClipboardBundle(
            text: validText(text) ? text : Data(),
            rtf: validRTF(rtf) ? rtf : Data(),
            html: validText(html) ? html : Data(),
            png: imagePNG(from: item) ?? Data()
        )
        guard pasteboard.changeCount == expectedChangeCount else {
            throw ClipboardError.changedDuringRead
        }
        guard bundle.hasExtendedContent else {
            throw ClipboardError.noText
        }
        return bundle
    }

    func write(expectedChangeCount: Int) throws -> Int? {
        let pasteboard = NSPasteboard.general
        guard pasteboard.changeCount == expectedChangeCount else { return nil }

        let item = NSPasteboardItem()
        var formats: [(NSPasteboard.PasteboardType, Data)] = [
            (.rtf, rtf), (.html, html), (.png, png)
        ]
        if !png.isEmpty {
            guard let image = Self.bitmapImage(from: png),
                  let tiff = NSBitmapImageRep(cgImage: image).tiffRepresentation
            else { throw ClipboardError.invalidBundle }
            formats.append((.tiff, tiff))
        }
        for (type, bytes) in formats where !bytes.isEmpty {
            guard item.setData(bytes, forType: type) else { throw ClipboardError.writeFailed }
        }
        if !text.isEmpty {
            guard let plain = String(data: text, encoding: .utf8),
                  item.setString(plain, forType: .string)
            else { throw ClipboardError.invalidBundle }
        }

        guard pasteboard.changeCount == expectedChangeCount else { return nil }
        let ownedCount = pasteboard.clearContents()
        guard pasteboard.changeCount == ownedCount else {
            throw ClipboardError.changedDuringWrite
        }
        guard pasteboard.writeObjects([item]) else { throw ClipboardError.writeFailed }
        if !text.isEmpty {
            guard let observed = pasteboard.string(forType: .string),
                  observed.utf8.elementsEqual(text)
            else { throw ClipboardError.changedDuringWrite }
        }
        for (type, bytes) in formats where !bytes.isEmpty {
            guard pasteboard.data(forType: type) == bytes else {
                throw ClipboardError.changedDuringWrite
            }
        }
        guard pasteboard.changeCount == ownedCount else {
            throw ClipboardError.changedDuringWrite
        }
        return ownedCount
    }

    private static func validText(_ bytes: Data) -> Bool {
        !bytes.isEmpty && bytes.count <= ClipboardService.maxClipboardBytes
            && !bytes.contains(0) && String(data: bytes, encoding: .utf8) != nil
    }

    private static func validRTF(_ bytes: Data) -> Bool {
        bytes.count <= ClipboardService.maxClipboardBytes
            && bytes.starts(with: Data("{\\rtf".utf8))
            && bytes.reversed().first(where: { ![9, 10, 11, 12, 13, 32].contains($0) }) == 125
    }

    private static func isPNG(_ data: Data) -> Bool {
        data.count <= maxPNGBytes
            && data.starts(with: [137, 80, 78, 71, 13, 10, 26, 10])
            && data.suffix(12).elementsEqual([0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130])
    }

    private static func bitmapImage(from data: Data) -> CGImage? {
        guard data.count <= maxImagePixels * 4 + 1_048_576,
              let source = CGImageSourceCreateWithData(data as CFData, nil),
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [String: Any],
              let width = properties[kCGImagePropertyPixelWidth as String] as? Int,
              let height = properties[kCGImagePropertyPixelHeight as String] as? Int,
              width > 0, height > 0, width <= maxImagePixels / height
        else { return nil }
        return CGImageSourceCreateImageAtIndex(source, 0, nil)
    }

    private static func imagePNG(from item: NSPasteboardItem) -> Data? {
        if let png = item.data(forType: .png), isPNG(png), bitmapImage(from: png) != nil {
            return png
        }
        guard let tiff = item.data(forType: .tiff),
              let image = bitmapImage(from: tiff),
              let png = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]),
              isPNG(png)
        else { return nil }
        return png
    }
}
