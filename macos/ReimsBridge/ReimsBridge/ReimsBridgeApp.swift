//
//  ReimsBridgeApp.swift
//  ReimsBridge
//
//  Created by Tamas Nemeth on 2026. 11. 26..
//

import SwiftUI
import AppKit

@main
struct ReimsBridgeApp: App {
    var body: some Scene {
        MenuBarExtra(
            "ReimsBridge",
            systemImage: "arrow.left.arrow.right"
        ) {
            ContentView()
        }
        .menuBarExtraStyle(.menu)
    }
}
