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
    static let shared = LocalIPC()
    
    @Published private(set) var isRunning = false
    @Published private(set) var status = "Stopped"
    @Published private(set) var requestCount = 0
    
    private var listener: Int32 = -1
    private var timer: Timer?
    private var activity: (any NSObjectProtocol)?

    private var clients: [Int32: IPCConnection] = [:]

    private let maxClients = 4
    
    private var directory: String {
        NSHomeDirectory() + "/.reimsbridge"
    }
    
    private var socketPath: String {
        directory + "/status.sock"
    }
    
    private init() {}
    
    func start() {
        guard !isRunning else {
            return
        }
        
        do {
            try startListener()
            
            requestCount = 0
            isRunning = true
            status = "Listening"

            activity = ProcessInfo.processInfo.beginActivity(
                options: .userInitiatedAllowingIdleSystemSleep,
                reason: "ReimsBridge local IPC service"
            )
            
            let pollTimer = Timer(
                timeInterval: 0.05,
                repeats: true
            ) { [weak self] _ in
                Task { @MainActor in
                    self?.acceptClients()
                }
            }
            
            timer = pollTimer
            
            RunLoop.main.add(
                pollTimer,
                forMode: .common
            )
        } catch {
            status = "Error: \(error.localizedDescription)"
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
        
        if listener >= 0 {
            Darwin.close(listener)
            listener = -1
            
            _ = Darwin.unlink(socketPath)
        }
        
        isRunning = false
        status = "Stopped"
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
    
    private func startListener() throws {
        try prepareDirectory()
        
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
        
        var bound = false
        var success = false
        
        defer {
            if !success {
                Darwin.close(fd)
                
                if bound {
                    _ = Darwin.unlink(socketPath)
                }
            }
        }
        
        guard Darwin.fcntl(fd, F_SETFL, O_NONBLOCK) >= 0 else {
            throw systemError("fcntl")
        }
        
        let result = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
                Darwin.bind(fd, socketAddress, socklen_t(
                    MemoryLayout<sockaddr_un>.size
                ))
            }
        }
        
        guard result == 0 else {
            throw systemError("bind")
        }
        
        bound = true
        
        guard Darwin.chmod(socketPath, 0o600) == 0 else {
            throw systemError("chmod")
        }
        
        guard Darwin.listen(fd, 8) == 0 else {
            throw systemError("listen")
        }
        
        listener = fd
        success = true
    }
    
    private func acceptClients() {
        guard isRunning, listener >= 0 else {
            return
        }

        for fd in Array(clients.keys) {
            guard let connection = clients[fd] else {
                continue
            }

            switch connection.advance() {
            case .pending:
                continue

            case .finished:
                requestCount += 1
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
