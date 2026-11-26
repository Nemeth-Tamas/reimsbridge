//
//  ContentView.swift
//  ReimsBridge
//
//  Created by Tamas Nemeth on 2026. 11. 26..
//

import SwiftUI
import AppKit

struct ContentView: View {
    @State private var changeCount: Int?
    @State private var hasPlainText = false
    
    var body: some View {
        Text("ReimsBridge v0.1.0")
            .font(.headline)
        
        Text("Connection: Not connected")
        Text("Synchronization: Disabled")
        
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
        
        Button("Inspect Clipboard") {
            inspectClipboard()
        }
        
        Divider()
        
        Button("Quit ReimsBridge") {
            NSApplication.shared.terminate(nil)
        }
        .keyboardShortcut("q")
    }
    
    private func inspectClipboard() {
        let pasteboard = NSPasteboard.general
        
        changeCount = pasteboard.changeCount
        
        hasPlainText = pasteboard.types?
            .contains(.string) ?? false
    }
}

#Preview {
    ContentView()
}
