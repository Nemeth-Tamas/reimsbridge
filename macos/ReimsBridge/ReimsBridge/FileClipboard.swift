import AppKit
import Darwin
import Foundation

@MainActor
enum FileClipboard {
    static let maxControlBytes = 1_048_576

    private static var stagingRoot: URL {
        URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true)
            .appendingPathComponent(".reimsbridge", isDirectory: true)
            .appendingPathComponent("transfers", isDirectory: true)
    }

    static func stageResponse() -> Data {
        let root = stagingRoot
        if Darwin.mkdir(root.path, 0o700) != 0 && errno != EEXIST {
            return Data("ERR FILE_STAGE\n".utf8)
        }
        guard isPrivateDirectory(root) else { return Data("ERR FILE_STAGE\n".utf8) }
        return encodePaths([root.path])
    }

    static func offer(expected: Int) -> Data {
        let pasteboard = NSPasteboard.general
        guard pasteboard.changeCount == expected else {
            return Data("ERR CLIPBOARD_CHANGED\n".utf8)
        }
        let urls = pasteboard.readObjects(
            forClasses: [NSURL.self],
            options: [.urlReadingFileURLsOnly: true]
        ) as? [URL] ?? []
        guard pasteboard.changeCount == expected else {
            return Data("ERR CLIPBOARD_CHANGED\n".utf8)
        }
        let paths = urls.filter { $0.isFileURL }.map { $0.path }
        guard !paths.isEmpty else { return Data("ERR NO_FILES\n".utf8) }
        return encodePaths(paths)
    }

    static func publish(expected: Int, batch: Substring) -> Data {
        let pasteboard = NSPasteboard.general
        guard pasteboard.changeCount == expected else { return Data("ERR STALE\n".utf8) }
        guard batch.hasPrefix("batch-"), batch.count <= 64,
              batch.utf8.allSatisfy({ (97...122).contains($0) || (48...57).contains($0) || $0 == 45 })
        else { return Data("ERR BAD_REQUEST\n".utf8) }

        let root = stagingRoot
        let directory = root.appendingPathComponent(String(batch), isDirectory: true)
        guard isPrivateDirectory(root), isPrivateDirectory(directory),
              let urls = try? FileManager.default.contentsOfDirectory(
                at: directory, includingPropertiesForKeys: nil, options: []
              ), !urls.isEmpty, urls.allSatisfy({ isRegularItem($0) })
        else { return Data("ERR FILE_STAGE\n".utf8) }

        guard pasteboard.changeCount == expected else { return Data("ERR STALE\n".utf8) }
        let written = pasteboard.clearContents()
        guard pasteboard.changeCount == written,
              pasteboard.writeObjects(urls.map { $0 as NSURL }),
              pasteboard.changeCount == written
        else { return Data("ERR WRITE_FAILED\n".utf8) }

        let received = pasteboard.readObjects(
            forClasses: [NSURL.self],
            options: [.urlReadingFileURLsOnly: true]
        ) as? [URL] ?? []
        guard Set(received.map { $0.path }) == Set(urls.map { $0.path }),
              pasteboard.changeCount == written
        else { return Data("ERR WRITE_FAILED\n".utf8) }

        return Data("OK \(written)\n".utf8)
    }

    private static func encodePaths(_ paths: [String]) -> Data {
        guard let data = try? JSONEncoder().encode(paths), data.count <= maxControlBytes else {
            return Data("ERR OVERSIZED\n".utf8)
        }
        return data
    }

    private static func isPrivateDirectory(_ url: URL) -> Bool {
        var info = stat()
        return Darwin.lstat(url.path, &info) == 0
            && (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFDIR)
            && info.st_uid == geteuid()
            && (info.st_mode & 0o077) == 0
    }

    private static func isRegularItem(_ url: URL) -> Bool {
        var info = stat()
        guard Darwin.lstat(url.path, &info) == 0 else { return false }
        let type = info.st_mode & mode_t(S_IFMT)
        return type == mode_t(S_IFREG) || type == mode_t(S_IFDIR)
    }
}
