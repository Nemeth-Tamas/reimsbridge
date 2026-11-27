//
//  IPCConnection.swift
//  ReimsBridge
//
//  Created by Tamas Nemeth on 2026. 11. 26..
//

import Foundation
import Darwin

private enum ClipboardFormat: String, CaseIterable {
    case utf8Text = "text-utf8"
    case bundle = "clipboard-bundle-v1"

    var maxPayloadBytes: Int {
        switch self {
        case .utf8Text:
            return ClipboardService.maxClipboardBytes
        case .bundle:
            return ClipboardBundle.maxPayloadBytes
        }
    }

    func isAvailable(in metadata: ClipboardMetadata) -> Bool {
        switch self {
        case .utf8Text:
            return metadata.hasPlainText
        case .bundle:
            return metadata.hasExtendedContent
        }
    }
}

@MainActor
final class IPCConnection {
    enum Progress {
        case pending
        case finished
        case failed
    }

    enum Transfer {
        case sent(bytes: Int)
        case received(bytes: Int)
    }

    private let fd: Int32

    private var deadline = ProcessInfo.processInfo.systemUptime + 10.0
    private var lastProgress = ProcessInfo.processInfo.systemUptime
    private var usesBundleDeadline = false

    private var request = Data()
    private var response: Data?
    private var sentBytes = 0
    private(set) var isWatchStatus = false
    private(set) var transfer: Transfer?

    private static let maxRequestBytes = 96
    private static let maxPayloadBytes = ClipboardFormat.allCases.map { $0.maxPayloadBytes }.max() ?? 0
    private static let readChunkBytes = 64 * 1024
    private static let writeChunkBytes = 64 * 1024
    private static let maxReadsPerTick = 4
    private static let maxWritesPerTick = 4

    init(fd: Int32) {
        self.fd = fd
    }

    func advance() -> Progress {
        let now = ProcessInfo.processInfo.systemUptime
        guard now < deadline, now - lastProgress < 10 else {
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
            Self.maxPayloadBytes
            + Self.maxRequestBytes
            + 1

        guard received <= maxFrameBytes - request.count else {
            return finishRequest(
                Data("ERR OVERSIZED\n".utf8)
            )
        }

        lastProgress = ProcessInfo.processInfo.systemUptime
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

        if fields.first == "read-format" {
            guard fields.count == 3,
                  request.count == payloadStart,
                  let counter = parseCounter(fields[2])
            else {
                return finishRequest(Data("ERR BAD_REQUEST\n".utf8))
            }

            guard let format = ClipboardFormat(rawValue: String(fields[1])) else {
                return finishRequest(Data("ERR UNSUPPORTED_FORMAT\n".utf8))
            }

            allowBundleDeadline(for: format)
            return finishRequest(readResponse(format: format, expectedChangeCount: counter))
        }

        let format: ClipboardFormat?
        let expectedChangeCount: Int?
        let sizeString: Substring

        if fields.count == 2, fields[0] == "write" {
            format = nil
            expectedChangeCount = nil
            sizeString = fields[1]
        } else if fields.count == 3, fields[0] == "write-if",
                  let counter = parseCounter(fields[1]) {
            format = nil
            expectedChangeCount = counter
            sizeString = fields[2]
        } else if fields.count == 4, fields[0] == "write-format-if",
                  let counter = parseCounter(fields[2]) {
            guard let requestedFormat = ClipboardFormat(rawValue: String(fields[1])) else {
                return finishRequest(Data("ERR UNSUPPORTED_FORMAT\n".utf8))
            }

            format = requestedFormat
            expectedChangeCount = counter
            sizeString = fields[3]
        } else {
            return finishRequest(Data("ERR BAD_REQUEST\n".utf8))
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

        guard payloadSize <= (format ?? .utf8Text).maxPayloadBytes else {
            return finishRequest(
                Data("ERR OVERSIZED\n".utf8)
            )
        }

        allowBundleDeadline(for: format)
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
                format: format,
                expectedChangeCount: expectedChangeCount
            )
        )
    }

    private func allowBundleDeadline(for format: ClipboardFormat?) {
        if format == .bundle && !usesBundleDeadline {
            // Large frames can take longer over SSH. The idle timeout still applies.
            deadline += 170
            usesBundleDeadline = true
        }
    }

    private func parseCounter(_ value: Substring) -> Int? {
        let digits = value.first == "-" ? value.dropFirst() : value
        guard !digits.isEmpty, digits.utf8.allSatisfy({ (48...57).contains($0) }) else {
            return nil
        }

        return Int(value)
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
        format: ClipboardFormat?,
        expectedChangeCount: Int?
    ) -> Data {
        do {
            let writeCount: Int?
            switch format ?? .utf8Text {
            case .utf8Text:
                let text = String(decoding: payload, as: UTF8.self)
                guard text.utf8.elementsEqual(payload) else {
                    return Data("ERR INVALID_TEXT\n".utf8)
                }
                writeCount = try ClipboardService.writeTextChecked(
                    text,
                    expectedChangeCount: expectedChangeCount
                )
            case .bundle:
                guard let expectedChangeCount else {
                    return Data("ERR BAD_REQUEST\n".utf8)
                }
                let bundle = try ClipboardBundle.decode(payload)
                writeCount = try bundle.write(expectedChangeCount: expectedChangeCount)
            }

            guard let writtenChangeCount = writeCount else {
                return Data("ERR STALE\n".utf8)
            }

            transfer = .received(bytes: payload.count)

            if expectedChangeCount != nil {
                let formatPrefix = format.map { "\($0.rawValue) " } ?? ""
                return Data("OK \(formatPrefix)\(writtenChangeCount)\n".utf8)
            }

            return Data("OK\n".utf8)
        } catch ClipboardError.invalidBundle {
            return Data("ERR INVALID_PAYLOAD\n".utf8)
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
        let supported = ClipboardFormat.allCases
        let available = supported.filter { $0.isAvailable(in: metadata) }

        let json =
            """
            {"protocol":2,"status":"ok","watchStatusSupported":true,"changeCount":\(metadata.changeCount),"hasPlainText":\(metadata.hasPlainText),"isExcluded":\(metadata.isExcluded),"formats":{"version":1,"supported":[\(formatList(supported))],"available":[\(formatList(available))]}}
            """ + "\n"

        return Data(json.utf8)
    }

    private func formatList(_ formats: [ClipboardFormat]) -> String {
        formats.map { "\"\($0.rawValue)\"" }.joined(separator: ",")
    }

    private func readResponse(
        format: ClipboardFormat? = nil,
        expectedChangeCount: Int? = nil
    ) -> Data {
        do {
            let payload: Data
            switch format ?? .utf8Text {
            case .utf8Text:
                let text = try ClipboardService.readText(expectedChangeCount: expectedChangeCount)
                payload = Data(text.utf8)
            case .bundle:
                guard let expectedChangeCount else {
                    return Data("ERR BAD_REQUEST\n".utf8)
                }
                payload = try ClipboardBundle.read(expectedChangeCount: expectedChangeCount).encode()
            }

            let formatPrefix = format.map { "\($0.rawValue) " } ?? ""
            var result = Data("OK \(formatPrefix)\(payload.count)\n".utf8)

            result.append(payload)
            transfer = .sent(bytes: payload.count)

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

        lastProgress = ProcessInfo.processInfo.systemUptime
        sentBytes += written

        if sentBytes == response.count {
            return .finished
        }

        return .pending
    }
}
