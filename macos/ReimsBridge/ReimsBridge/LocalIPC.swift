//
//  LocalIPC.swift
//  ReimsBridge
//
//  Created by Tamas Nemeth on 2026. 11. 26..
//

import Foundation
import Combine
import Darwin

struct IPCError: LocalizedError {
    let message: String
    
    var errorDescription: String? {
        message
    }
}

@MainActor
final class LocalIPC: ObservableObject {
    enum State {
        case stopped(cleanupError: String?)
        case listening(watcherRecentlySeen: Bool)
        case failed(String)

        var description: String {
            switch self {
            case .stopped(let cleanupError):
                return cleanupError.map { "Stopped: \($0)" } ?? "Stopped"
            case .listening:
                return "Listening"
            case .failed(let message):
                return "Error: \(message)"
            }
        }
    }

    static let shared = LocalIPC()

    @Published private(set) var state: State = .stopped(cleanupError: nil)
    let statistics = IPCStatistics()

    var isRunning: Bool {
        if case .listening = state { return true }
        return false
    }

    var watcherRecentlySeen: Bool {
        if case .listening(let recentlySeen) = state { return recentlySeen }
        return false
    }
    
    private var listener: Int32 = -1
    private var listenerLock: Int32 = -1
    private var boundSocketInfo: stat?
    private var timer: Timer?
    private var activity: (any NSObjectProtocol)?

    private var clients: [Int32: IPCConnection] = [:]

    private let maxClients = 4
    private let watcherTimeout: TimeInterval = 30
    private var lastWatcherActivity: TimeInterval?
    
    private var directory: String {
        NSHomeDirectory() + "/.reimsbridge"
    }
    
    private var socketPath: String {
        directory + "/status.sock"
    }
    
    private var lockPath: String {
        directory + "/listener.lock"
    }

    private init() {}
    
    func start() {
        guard !isRunning else {
            return
        }
        
        do {
            try startListener()
            
            statistics.reset()
            lastWatcherActivity = nil
            state = .listening(watcherRecentlySeen: false)

            activity = ProcessInfo.processInfo.beginActivity(
                options: .userInitiatedAllowingIdleSystemSleep,
                reason: "ReimsBridge local IPC service"
            )
            
            let pollTimer = Timer(
                timeInterval: 0.05,
                repeats: true
            ) { [weak self] _ in
                // This timer is scheduled only on the main run loop.
                MainActor.assumeIsolated {
                    self?.acceptClients()
                }
            }
            
            timer = pollTimer
            
            RunLoop.main.add(
                pollTimer,
                forMode: .common
            )
        } catch {
            state = .failed(error.localizedDescription)
        }
    }
    
    func stop() {
        timer?.invalidate()
        timer = nil

        if let activity {
            ProcessInfo.processInfo.endActivity(activity)
            self.activity = nil
        }

        for fd in clients.keys {
            Darwin.close(fd)
        }

        clients.removeAll()
        
        var cleanupError: String?

        if listener >= 0 {
            // Remove only the socket this listener actually bound.
            // Keep the listener lock held through all cleanup.
            if let boundSocketInfo {
                do {
                    try unlinkSocket(matching: boundSocketInfo)
                } catch {
                    cleanupError = error.localizedDescription
                }
            }

            Darwin.close(listener)
            listener = -1
            boundSocketInfo = nil
        }

        if listenerLock >= 0 {
            Darwin.close(listenerLock)
            listenerLock = -1
        }

        lastWatcherActivity = nil
        state = .stopped(cleanupError: cleanupError)
    }
    
    private func systemError(
        _ operation: String
    ) -> IPCError {
        let description = String(
            cString: strerror(errno)
        )
        
        return IPCError(
            message: "\(operation): \(description)"
        )
    }
    
    private func prepareDirectory() throws {
        if Darwin.mkdir(directory, 0o700) != 0 {
            guard errno == EEXIST else {
                throw systemError("mkdir")
            }
        }
        
        var info = stat()
        
        guard Darwin.lstat(directory, &info) == 0 else {
            throw systemError("lstat")
        }
        
        guard
            (info.st_mode & mode_t(S_IFMT)) ==
                mode_t(S_IFDIR),
            info.st_uid == getuid(),
            (info.st_mode & 0o7777) == 0o700
        else {
            throw IPCError(message: "IPC directory is not private")
        }
    }
    
    private func acquireListenerLock() throws -> Int32 {
        // Never unlink the lock file: all instances must lock the same inode.
        let fd = Darwin.open(
            lockPath,
            O_CREAT | O_RDWR | O_NOFOLLOW | O_CLOEXEC | O_EXLOCK | O_NONBLOCK,
            mode_t(0o600)
        )

        guard fd >= 0 else {
            if errno == EWOULDBLOCK {
                throw IPCError(message: "Another ReimsBridge IPC listener is running")
            }

            throw systemError("open listener lock")
        }

        var success = false
        defer {
            if !success {
                Darwin.close(fd)
            }
        }

        var info = stat()
        guard Darwin.fstat(fd, &info) == 0 else {
            throw systemError("fstat listener lock")
        }

        guard (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
              info.st_uid == getuid(),
              (info.st_mode & 0o7777) == 0o600,
              info.st_nlink == 1
        else {
            throw IPCError(message: "IPC listener lock is not private")
        }

        success = true
        return fd
    }

    private func socketInfo() throws -> stat? {
        var info = stat()

        if Darwin.lstat(socketPath, &info) != 0 {
            if errno == ENOENT {
                return nil
            }

            throw systemError("lstat socket")
        }

        guard (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFSOCK),
              info.st_uid == getuid()
        else {
            throw IPCError(message: "IPC socket path is not an owned socket")
        }

        return info
    }

    private func unlinkSocket(matching expected: stat) throws {
        guard let current = try socketInfo() else {
            return
        }

        guard current.st_dev == expected.st_dev,
              current.st_ino == expected.st_ino
        else {
            throw IPCError(message: "IPC socket changed during cleanup")
        }

        guard Darwin.unlink(socketPath) == 0 else {
            throw systemError("unlink socket")
        }
    }

    private func recoverStaleSocket(address: inout sockaddr_un) throws {
        guard let existing = try socketInfo() else {
            return
        }

        // Older versions do not take the listener lock. Probe their socket
        // before removing anything, and treat ambiguous results as live.
        let probe = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard probe >= 0 else {
            throw systemError("socket probe")
        }

        defer {
            Darwin.close(probe)
        }

        guard Darwin.fcntl(probe, F_SETFL, O_NONBLOCK) >= 0 else {
            throw systemError("fcntl socket probe")
        }

        let result = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
                Darwin.connect(
                    probe,
                    socketAddress,
                    socklen_t(MemoryLayout<sockaddr_un>.size)
                )
            }
        }

        if result == 0 {
            throw IPCError(message: "Another IPC listener is already accepting connections")
        }

        let connectError = errno

        guard connectError == ECONNREFUSED else {
            throw IPCError(message: "Existing IPC socket could not be safely recovered (errno \(connectError))")
        }

        try unlinkSocket(matching: existing)
    }

    private func startListener() throws {
        try prepareDirectory()

        let lock = try acquireListenerLock()
        var retainLock = false
        defer {
            if !retainLock {
                Darwin.close(lock)
            }
        }
        
        var address = sockaddr_un()
        let pathBytes = Array(socketPath.utf8) + [0]
        
        guard pathBytes.count <=
                MemoryLayout.size(ofValue: address.sun_path)
                
        else {
            throw IPCError(message: "Socket path is too long")
        }
        
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(
            MemoryLayout<sockaddr_un>.size
        )
        
        withUnsafeMutableBytes(of: &address.sun_path) { destination in
            destination.copyBytes(from: pathBytes)
        }
        
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        
        guard fd >= 0 else {
            throw systemError("socket")
        }
        
        var ownedSocket: stat?
        var success = false

        defer {
            if !success {
                if let ownedSocket {
                    try? unlinkSocket(matching: ownedSocket)
                }

                Darwin.close(fd)
            }
        }
        
        guard Darwin.fcntl(fd, F_SETFL, O_NONBLOCK) >= 0 else {
            throw systemError("fcntl")
        }
        
        func bindSocket() -> Int32 {
            withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
                    Darwin.bind(fd, socketAddress, socklen_t(
                        MemoryLayout<sockaddr_un>.size
                    ))
                }
            }
        }

        if bindSocket() != 0 {
            guard errno == EADDRINUSE else {
                throw systemError("bind")
            }

            try recoverStaleSocket(address: &address)

            guard bindSocket() == 0 else {
                throw systemError("bind after stale socket recovery")
            }
        }

        guard let identity = try socketInfo() else {
            throw IPCError(message: "Bound IPC socket disappeared")
        }

        ownedSocket = identity
        
        guard Darwin.chmod(socketPath, 0o600) == 0 else {
            throw systemError("chmod")
        }
        
        guard Darwin.listen(fd, 8) == 0 else {
            throw systemError("listen")
        }
        
        listener = fd
        boundSocketInfo = ownedSocket
        listenerLock = lock
        retainLock = true
        success = true
    }
    
    private func acceptClients() {
        guard isRunning, listener >= 0 else {
            return
        }

        if let lastWatcherActivity,
           ProcessInfo.processInfo.systemUptime - lastWatcherActivity >= watcherTimeout {
            self.lastWatcherActivity = nil
            state = .listening(watcherRecentlySeen: false)
        }

        for fd in Array(clients.keys) {
            guard let connection = clients[fd] else {
                continue
            }

            switch connection.advance() {
            case .pending:
                continue

            case .finished:
                if connection.isWatchStatus {
                    lastWatcherActivity = ProcessInfo.processInfo.systemUptime
                    if !watcherRecentlySeen {
                        state = .listening(watcherRecentlySeen: true)
                    }
                }
                statistics.recordCompletedRequest(connection.transfer)
                closeClient(fd)

            case .failed:
                closeClient(fd)
            }
        }

        for _ in 0..<maxClients {
            guard clients.count < maxClients else {
                break
            }

            let client = Darwin.accept(
                listener,
                nil,
                nil
            )

            if client < 0 {
                break
            }

            handleClient(client)
        }
    }

    private func closeClient(_ fd: Int32) {
        clients.removeValue(forKey: fd)
        Darwin.close(fd)
    }

    private func handleClient(_ client: Int32) {
        var accepted = false

        defer {
            if !accepted {
                Darwin.close(client)
            }
        }

        var uid: uid_t = 0
        var gid: gid_t = 0

        guard getpeereid(
            client,
            &uid,
            &gid
        ) == 0, uid == getuid() else {
            return
        }

        guard Darwin.fcntl(
            client,
            F_SETFL,
            O_NONBLOCK
        ) >= 0 else {
            return
        }

        var noSigPipe: Int32 = 1

        guard Darwin.setsockopt(
            client,
            SOL_SOCKET,
            SO_NOSIGPIPE,
            &noSigPipe,
            socklen_t(MemoryLayout<Int32>.size)
        ) == 0 else {
            return
        }

        clients[client] = IPCConnection(fd: client)

        accepted = true
    }
}

@MainActor
final class IPCStatistics: ObservableObject {
    struct Counts {
        var completedRequests = 0
        var sentTransfers = 0
        var sentBytes = 0
        var receivedTransfers = 0
        var receivedBytes = 0
    }

    @Published private(set) var counts = Counts()

    func reset() {
        counts = Counts()
    }

    func recordCompletedRequest(_ transfer: IPCConnection.Transfer?) {
        var updated = counts
        updated.completedRequests += 1

        switch transfer {
        case .sent(let bytes):
            updated.sentTransfers += 1
            updated.sentBytes += bytes
        case .received(let bytes):
            updated.receivedTransfers += 1
            updated.receivedBytes += bytes
        case nil:
            break
        }

        counts = updated
    }
}
