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
        ProcessInfo.processInfo.systemUptime + 10.0

    private var request = Data()
    private var response: Data?
    private var sentBytes = 0
    private(set) var isWatchStatus = false

    private static let maxRequestBytes = 64
    private static let readChunkBytes = 64 * 1024
    private static let writeChunkBytes = 64 * 1024
    private static let maxReadsPerTick = 4
    private static let maxWritesPerTick = 4

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

        for _ in 0..<Self.maxWritesPerTick {
            let previousSentBytes = sentBytes

            switch sendResponse() {
            case .finished:
                return .finished

            case .failed:
                return .failed

            case .pending:
                if sentBytes == previousSentBytes {
                    return .pending
                }
            }
        }

        return .pending
    }

    private func receiveRequest() -> Progress {
        for _ in 0..<Self.maxReadsPerTick {
            let previousRequestBytes = request.count

            switch receiveChunk() {
            case .finished:
                return .finished

            case .failed:
                return .failed

            case .pending:
                if request.count == previousRequestBytes {
                    return .pending
                }
            }
        }

        return .pending
    }

    private func receiveChunk() -> Progress {
        var buffer = [UInt8](
            repeating: 0,
            count: Self.readChunkBytes
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

        let maxFrameBytes =
            ClipboardService.maxClipboardBytes
            + Self.maxRequestBytes
            + 1

        guard received <= maxFrameBytes - request.count else {
            return finishRequest(
                Data("ERR OVERSIZED\n".utf8)
            )
        }

        request.append(
            contentsOf: buffer.prefix(received)
        )

        return parseRequest()
    }

    private func parseRequest() -> Progress {
        guard let newline = request
            .prefix(Self.maxRequestBytes)
            .firstIndex(of: 10)
        else {
            if request.count >= Self.maxRequestBytes {
                return finishRequest(
                    Data("ERR BAD_REQUEST\n".utf8)
                )
            }

            return .pending
        }

        let header = String(
            decoding: request[..<newline],
            as: UTF8.self
        )

        let payloadStart = newline + 1

        if header == "status" || header == "watch-status" || header == "read" {
            guard request.count == payloadStart else {
                return finishRequest(
                    Data("ERR BAD_REQUEST\n".utf8)
                )
            }

            isWatchStatus = header == "watch-status"

            return finishRequest(
                header == "read"
                    ? readResponse()
                    : statusResponse()
            )
        }

        let fields = header.split(
            separator: " ",
            omittingEmptySubsequences: false
        )

        let expectedChangeCount: Int?
        let sizeString: Substring

        if fields.count == 2,
           fields[0] == "write" {
            expectedChangeCount = nil
            sizeString = fields[1]
        } else if fields.count == 3,
                  fields[0] == "write-if" {
            let counterString = fields[1]

            let digits = counterString.first == "-"
                ? counterString.dropFirst()
                : counterString

            guard !digits.isEmpty,
                  digits.utf8.allSatisfy({
                      (48...57).contains($0)
                  }),
                  let counter = Int(counterString)
            else {
                return finishRequest(
                    Data("ERR BAD_REQUEST\n".utf8)
                )
            }

            expectedChangeCount = counter
            sizeString = fields[2]
        } else {
            return finishRequest(
                Data("ERR BAD_REQUEST\n".utf8)
            )
        }

        guard !sizeString.isEmpty,
              sizeString.utf8.allSatisfy({
                  (48...57).contains($0)
              }),
              let payloadSize = Int(sizeString)
        else {
            return finishRequest(
                Data("ERR BAD_REQUEST\n".utf8)
            )
        }

        guard payloadSize > 0 else {
            return finishRequest(
                Data("ERR NO_TEXT\n".utf8)
            )
        }

        guard payloadSize <= ClipboardService.maxClipboardBytes else {
            return finishRequest(
                Data("ERR OVERSIZED\n".utf8)
            )
        }

        let payloadEnd = payloadStart + payloadSize
        let expectedFrameBytes = payloadEnd + 1

        if request.count < expectedFrameBytes {
            return .pending
        }

        guard request.count == expectedFrameBytes,
              request.last == 0
        else {
            return finishRequest(
                Data("ERR BAD_REQUEST\n".utf8)
            )
        }

        let payload = Data(
            request[payloadStart..<payloadEnd]
        )

        return finishRequest(
            writeResponse(
                payload,
                expectedChangeCount: expectedChangeCount
            )
        )
    }

    private func finishRequest(
        _ data: Data
    ) -> Progress {
        response = data
        request.removeAll()

        return .finished
    }

    private func writeResponse(
        _ payload: Data,
        expectedChangeCount: Int?
    ) -> Data {
        let text = String(
            decoding: payload,
            as: UTF8.self
        )

        guard text.utf8.elementsEqual(payload) else {
            return Data("ERR INVALID_TEXT\n".utf8)
        }

        do {
            guard let writtenChangeCount =
                try ClipboardService.writeTextChecked(
                    text,
                    expectedChangeCount: expectedChangeCount
                )
            else {
                return Data("ERR STALE\n".utf8)
            }

            if expectedChangeCount != nil {
                return Data(
                    "OK \(writtenChangeCount)\n".utf8
                )
            }

            return Data("OK\n".utf8)
        } catch ClipboardError.noText {
            return Data("ERR NO_TEXT\n".utf8)
        } catch ClipboardError.oversized {
            return Data("ERR OVERSIZED\n".utf8)
        } catch ClipboardError.embeddedNul {
            return Data("ERR INVALID_TEXT\n".utf8)
        } catch {
            return Data("ERR WRITE_FAILED\n".utf8)
        }
    }

    private func statusResponse() -> Data {
        let metadata = ClipboardService.inspect()

        let json =
            """
            {"protocol":2,"status":"ok","watchStatusSupported":true,"changeCount":\(metadata.changeCount),"hasPlainText":\(metadata.hasPlainText),"isExcluded":\(metadata.isExcluded)}
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
