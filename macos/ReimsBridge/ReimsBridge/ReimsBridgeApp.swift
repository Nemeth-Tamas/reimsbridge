//
//  ReimsBridgeApp.swift
//  ReimsBridge
//
//  Created by Tamas Nemeth on 2026. 11. 26..
//

import SwiftUI
import AppKit
import Foundation

enum ReimsBridgePreferences {
    static let startIPCOnLaunch = "startIPCOnLaunch"
    static let showTransferStatistics = "showTransferStatistics"
}

@MainActor
final class ReimsBridgeAppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        ClipboardMonitor.shared.start()

        if UserDefaults.standard.bool(forKey: ReimsBridgePreferences.startIPCOnLaunch) {
            LocalIPC.shared.start()
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        LocalIPC.shared.stop()
        ClipboardMonitor.shared.stop()
    }
}

@main
struct ReimsBridgeApp: App {
    @NSApplicationDelegateAdaptor(ReimsBridgeAppDelegate.self) private var appDelegate
    @StateObject private var ipc = LocalIPC.shared

    var body: some Scene {
        let label = menuBarLabel
        MenuBarExtra(label.title, systemImage: label.symbol) {
            ContentView()
        }
        .menuBarExtraStyle(.menu)
    }

    private var menuBarLabel: (title: LocalizedStringKey, symbol: String) {
        switch ipc.state {
        case .stopped(cleanupError: nil):
            return ("ReimsBridge: IPC stopped", "pause.circle")
        case .stopped, .failed:
            return ("ReimsBridge: IPC error", "exclamationmark.triangle")
        case .listening(watcherRecentlySeen: false):
            return ("ReimsBridge: IPC listening", "arrow.left.arrow.right")
        case .listening(watcherRecentlySeen: true):
            return ("ReimsBridge: Windows watcher recently active", "checkmark.circle.fill")
        }
    }
}
