//
//  ContentView.swift
//  ReimsBridge
//
//  Created by Tamas Nemeth on 2026. 11. 26..
//

import SwiftUI
import AppKit
import ServiceManagement

struct ContentView: View {
    @ObservedObject private var ipc = LocalIPC.shared
    @ObservedObject private var clipboard = ClipboardMonitor.shared
    @ObservedObject private var statistics = LocalIPC.shared.statistics

    @AppStorage(ReimsBridgePreferences.startIPCOnLaunch) private var startIPCOnLaunch = false
    @AppStorage(ReimsBridgePreferences.showTransferStatistics) private var showTransferStatistics = true

    @State private var validation: (changeCount: Int, message: String)?
    @State private var selfTestStatus = "Not tested"
    @State private var loginStartupError: String?
    @State private var loginStartupStatus = SMAppService.mainApp.status

    var body: some View {
        Text("ReimsBridge v0.1.0")
            .font(.headline)

        Text("IPC: \(ipc.state.description)")
        Text(ipc.watcherRecentlySeen
            ? "Windows watcher: recently active"
            : "Windows watcher: not detected")

        Button(ipc.isRunning ? "Stop IPC Service" : "Start IPC Service") {
            if ipc.isRunning {
                ipc.stop()
            } else {
                ipc.start()
            }
        }

        Divider()

        if let metadata = clipboard.metadata {
            Text("Clipboard change count: \(metadata.changeCount)")
            Text(metadata.hasPlainText ? "Plain text available" : "No plain text available")

            if metadata.isExcluded {
                Text("Privacy marker detected (transfer allowed)")
            }
        } else {
            Text("Waiting for clipboard metadata")
        }

        if showTransferStatistics {
            Divider()
            Text("Completed IPC requests: \(statistics.counts.completedRequests)")
            Text("IPC text sent: \(statistics.counts.sentTransfers) · \(statistics.counts.sentBytes) bytes")
            Text("IPC text received: \(statistics.counts.receivedTransfers) · \(statistics.counts.receivedBytes) bytes")
        }

        Divider()

        Menu("Diagnostics") {
            Button("Refresh Clipboard Metadata") {
                clipboard.refresh(force: true)
            }

            Text("Validation: \(validationStatus)")
            Button("Validate Clipboard Text", action: validateClipboard)

            Text("Self-test: \(selfTestStatus)")
            Button("Run Clipboard Self-Test", action: runSelfTest)
        }

        Menu("Preferences") {
            Button(loginStartupEnabled ? "Disable Start at Login" : "Enable Start at Login") {
                toggleLoginStartup()
            }
            if loginStartupStatus == .requiresApproval {
                Button("Open Login Item Settings") {
                    SMAppService.openSystemSettingsLoginItems()
                }
            }
            if let loginStartupError {
                Text(loginStartupError)
            }
            Toggle("Start IPC on Launch", isOn: $startIPCOnLaunch)
            Toggle("Show Transfer Statistics", isOn: $showTransferStatistics)
        }

        Divider()

        Button("Quit ReimsBridge") {
            NSApplication.shared.terminate(nil)
        }
        .keyboardShortcut("q")
    }

    private var validationStatus: String {
        guard let validation,
              validation.changeCount == clipboard.metadata?.changeCount
        else { return "Not tested" }

        return validation.message
    }

    private var loginStartupEnabled: Bool {
        loginStartupStatus == .enabled || loginStartupStatus == .requiresApproval
    }

    private func toggleLoginStartup() {
        loginStartupStatus = SMAppService.mainApp.status
        defer { loginStartupStatus = SMAppService.mainApp.status }
        do {
            if loginStartupEnabled {
                try SMAppService.mainApp.unregister()
            } else {
                try SMAppService.mainApp.register()
                startIPCOnLaunch = true
            }
            loginStartupError = nil
        } catch {
            loginStartupError = error.localizedDescription
        }
    }

    private func validateClipboard() {
        let changeCount = NSPasteboard.general.changeCount
        defer { clipboard.refresh(force: true) }

        do {
            let text = try ClipboardService.readText()
            validation = (changeCount, "PASS (\(text.utf8.count) bytes)")
        } catch {
            validation = (changeCount, error.localizedDescription)
        }
    }

    private func runSelfTest() {
        do {
            let bytes = try ClipboardService.selfTest()
            selfTestStatus = "PASS (\(bytes) bytes)"
        } catch {
            selfTestStatus = "FAIL: \(error.localizedDescription)"
        }
    }
}

#Preview {
    ContentView()
}
