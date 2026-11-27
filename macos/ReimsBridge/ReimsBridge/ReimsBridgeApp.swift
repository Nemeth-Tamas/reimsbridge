//
//  ReimsBridgeApp.swift
//  ReimsBridge
//
//  Created by Tamas Nemeth on 2026. 11. 26..
//

import AppKit
import Foundation

enum ReimsBridgePreferences {
    static let startIPCOnLaunch = "startIPCOnLaunch"
    static let showTransferStatistics = "showTransferStatistics"
}

@MainActor
final class ReimsBridgeAppDelegate: NSObject, NSApplicationDelegate {
    private var statusMenu: ReimsBridgeStatusMenu?

    func applicationDidFinishLaunching(_ notification: Notification) {
        ClipboardMonitor.shared.start()
        statusMenu = ReimsBridgeStatusMenu()

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
@MainActor
enum ReimsBridgeApp {
    static func main() {
        let application = NSApplication.shared
        let delegate = ReimsBridgeAppDelegate()
        application.delegate = delegate
        application.setActivationPolicy(.accessory)
        withExtendedLifetime(delegate) {
            application.run()
        }
    }
}
