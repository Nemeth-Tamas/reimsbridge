//
//  IPCConnection.swift
//  ReimsBridge
//
//  Created by Tamas Nemeth on 2026. 11. 26..
//

import Foundation
import Darwin

@MainActor
final class IPCConnection {
    enum Progress {
        case pending
        case finished
        case failed
    }

    private let fd: Int32

    private let deadline =
        ProcessInfo.processInfo.systemUptime + 4.0

    private var request = Data()
    private var response: Data?
    private var sentBytes = 0

    private static let maxRequestBytes = 16
    private static let writeChunkBytes = 64 * 1024

    init(fd: Int32) {
        self.fd = fd
    }

    func advance() -> Progress {
        guard ProcessInfo.processInfo.systemUptime
                < deadline else {
            return .failed
        }

        if response == nil {
            switch receiveRequest() {
            case .pending:
                return .pending
            case .failed:
                return .failed
            case .finished:
                break
            }
        }

        return sendResponse()
    }

    private func receiveRequest() -> Progress {
        var buffer = [UInt8](
            repeating: 0,
            count: Self.maxRequestBytes
        )

        let received = buffer.withUnsafeMutableBytes {
            Darwin.recv(
                fd,
                $0.baseAddress,
                $0.count,
                0
            )
        }

        if received < 0 {
            if errno == EAGAIN ||
                errno == EWOULDBLOCK ||
                errno == EINTR {
                return .pending
            }

            return .failed
        }

        guard received > 0 else {
            return .failed
        }

        request.append(
            contentsOf: buffer.prefix(received)
        )

        if request.count > Self.maxRequestBytes {
            response = Data("ERR BAD_REQUEST\n".utf8)
            return .finished
        }

        guard let newline = request.firstIndex(of: 10) else {
            return .pending
        }

        guard newline == request.count - 1 else {
            response = Data("ERR BAD_REQUEST\n".utf8)
            return .finished
        }

        if request == Data("status\n".utf8) {
            response = statusResponse()
        } else if request == Data("read\n".utf8) {
            response = readResponse()
        } else {
            response = Data("ERR BAD_REQUEST\n".utf8)
        }

        request.removeAll()

        return .finished
    }

    private func statusResponse() -> Data {
        let metadata = ClipboardService.inspect()

        let json =
            """
            {"protocol":2,"status":"ok","changeCount":\(metadata.changeCount),"hasPlainText":\(metadata.hasPlainText),"isExcluded":\(metadata.isExcluded)}
            """ + "\n"

        return Data(json.utf8)
    }

    private func readResponse() -> Data {
        do {
            let text = try ClipboardService.readText()

            let payload = Data(text.utf8)

            var result = Data(
                "OK \(payload.count)\n".utf8
            )

            result.append(payload)

            return result
        } catch ClipboardError.noText {
            return Data("ERR NO_TEXT\n".utf8)
        } catch ClipboardError.excluded {
            return Data("ERR EXCLUDED\n".utf8)
        } catch ClipboardError.oversized {
            return Data("ERR OVERSIZED\n".utf8)
        } catch ClipboardError.embeddedNul {
            return Data("ERR INVALID_TEXT\n".utf8)
        } catch ClipboardError.changedDuringRead {
            return Data("ERR CLIPBOARD_CHANGED\n".utf8)
        } catch {
            return Data("ERR READ_FAILED\n".utf8)
        }
    }

    private func sendResponse() -> Progress {
        guard let response else {
            return .failed
        }

        let remaining = response.count - sentBytes

        guard remaining > 0 else {
            return .finished
        }

        let chunk = min(
            remaining,
            Self.writeChunkBytes
        )

        let written = response.withUnsafeBytes {
            buffer in

            Darwin.send(
                fd,
                buffer.baseAddress?.advanced(
                    by: sentBytes
                ),
                chunk,
                0
            )
        }

        if written < 0 {
            if errno == EAGAIN ||
                errno == EWOULDBLOCK ||
                errno == EINTR {
                return .pending
            }

            return .failed
        }

        guard written > 0 else {
            return .failed
        }

        sentBytes += written

        if sentBytes == response.count {
            return .finished
        }

        return .pending
    }
}
