//
//  ContentView.swift
//  ReimsBridge
//
//  Created by Tamas Nemeth on 2026. 11. 26..
//

import SwiftUI
import AppKit

struct ContentView: View {
    @ObservedObject private var ipc = LocalIPC.shared
    @State private var changeCount: Int?
    @State private var hasPlainText = false
    @State private var isExcluded = false
    @State private var validationStatus = "Not tested"
    @State private var selfTestStatus = "Not tested"
    
    var body: some View {
        Text("ReimsBridge v0.1.0")
            .font(.headline)
        
        Text(ipc.isRunning
            ? "Service available to Windows"
            : "Service unavailable")
        Text(ipc.watcherRecentlySeen
            ? "Windows watcher: recently active"
            : "Windows watcher: not detected")
        Text("Synchronization controlled by Windows")
        
        Divider()
        
        Text("IPC: \(ipc.status)")
        Text("Completed IPC requests: \(ipc.requestCount)")
        
        Button(
            ipc.isRunning
                ? "Stop IPC Service"
                : "Start IPC Service"
        ) {
            if ipc.isRunning {
                ipc.stop()
            } else {
                ipc.start()
            }
        }
        
        Divider()
        
        if let changeCount {
            Text("Clipboard change count: \(changeCount)")
            
            Text(
                hasPlainText
                    ? "Plain text available"
                : "No plain text available"
            )
        } else {
            Text("Clipboard: Not inspected")
        }
        
        if changeCount != nil {
            Text(
                hasPlainText
                    ? "Transfer eligibility: Candidate"
                    : "Transfer eligibility: No text"
            )
        }
        
        if changeCount != nil {
            Text(isExcluded
                ? "Privacy marker detected (transfer allowed)"
                : "No known privacy markers detected")
        }
        
        Button("Inspect Clipboard") {
            inspectClipboard()
        }
        
        Divider()
        
        Text("Validation: \(validationStatus)")
        
        Button("Validate Clipboard Text") {
            validateClipboard()
        }
        
        Divider()
        
        Text("Self-test: \(selfTestStatus)")
        
        Button("Run Clipboard Self-Test") {
            runSelfTest()
        }
        
        Divider()
        
        Button("Quit ReimsBridge") {
            ipc.stop()
            NSApplication.shared.terminate(nil)
        }
        .keyboardShortcut("q")
    }
    
    private func inspectClipboard() {
        let metadata = ClipboardService.inspect()
        
        changeCount = metadata.changeCount
        hasPlainText = metadata.hasPlainText
        isExcluded = metadata.isExcluded
        validationStatus = "Not tested"
    }
    
    private func validateClipboard() {
        inspectClipboard()
        
        do {
            let text = try ClipboardService.readText()
            
            validationStatus =
            "PASS (\(text.utf8.count ) bytes)"
        } catch {
            validationStatus = error.localizedDescription
        }
    }
    
    private func runSelfTest() {
        do {
            let bytes = try ClipboardService.selfTest()
            
            selfTestStatus = "PASS (\(bytes) bytes)"
        } catch {
            selfTestStatus =
                "FAIL: \(error.localizedDescription)"
        }
    }
}

#Preview {
    ContentView()
}
