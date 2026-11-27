import AppKit
import Combine
import Foundation
import ServiceManagement

@MainActor
final class ReimsBridgeStatusMenu: NSObject, NSMenuDelegate {
    private let ipc = LocalIPC.shared
    private let clipboard = ClipboardMonitor.shared
    private let defaults = UserDefaults.standard
    private let statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    private let menu = NSMenu()
    private var stateObserver: AnyCancellable?
    private var isTracking = false

    private var validation: (changeCount: Int, message: String)?
    private var selfTestStatus = "Not tested"
    private var loginStartupHint: String?

    override init() {
        super.init()
        defaults.register(defaults: [ReimsBridgePreferences.showTransferStatistics: true])
        menu.autoenablesItems = false
        menu.delegate = self
        rebuildMenu()
        statusItem.menu = menu

        stateObserver = ipc.$state.sink { [weak self] state in
            // LocalIPC publishes on the main actor, including the initial value.
            MainActor.assumeIsolated {
                self?.updateIcon(for: state)
            }
        }
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        guard !isTracking else { return }
        rebuildMenu()
    }

    func menuWillOpen(_ menu: NSMenu) {
        isTracking = true
    }

    func menuDidClose(_ menu: NSMenu) {
        isTracking = false
    }

    private func rebuildMenu() {
        // Sample counters before tracking. Rebuilding a visible menu can
        // dismiss its submenu while the pointer is over it.
        clipboard.refresh(force: true)
        let counts = ipc.statistics.counts
        let loginStatus = SMAppService.mainApp.status

        menu.removeAllItems()
        menu.addItem(label("ReimsBridge v0.1.0"))
        menu.addItem(label("IPC: \(ipc.state.description)"))
        menu.addItem(label(ipc.watcherRecentlySeen
            ? "Windows watcher: recently active"
            : "Windows watcher: not detected"))
        menu.addItem(action(ipc.isRunning ? "Stop IPC Service" : "Start IPC Service", #selector(toggleIPC(_:))))
        menu.addItem(.separator())

        if let metadata = clipboard.metadata {
            menu.addItem(label("Clipboard change count: \(metadata.changeCount)"))
            menu.addItem(label(metadata.hasPlainText ? "Plain text available" : "No plain text available"))
            if metadata.isExcluded {
                menu.addItem(label("Privacy marker detected (transfer allowed)"))
            }
        } else {
            menu.addItem(label("Waiting for clipboard metadata"))
        }

        if defaults.bool(forKey: ReimsBridgePreferences.showTransferStatistics) {
            menu.addItem(.separator())
            menu.addItem(label("Completed IPC requests: \(counts.completedRequests)"))
            menu.addItem(label("IPC text sent: \(counts.sentTransfers) · \(counts.sentBytes) bytes"))
            menu.addItem(label("IPC text received: \(counts.receivedTransfers) · \(counts.receivedBytes) bytes"))
        }

        menu.addItem(.separator())

        let diagnostics = NSMenu(title: "Diagnostics")
        diagnostics.autoenablesItems = false
        diagnostics.addItem(action("Refresh Clipboard Metadata", #selector(refreshClipboard(_:))))
        diagnostics.addItem(label("Validation: \(validationStatus)"))
        diagnostics.addItem(action("Validate Clipboard Text", #selector(validateClipboard(_:))))
        diagnostics.addItem(label("Self-test: \(selfTestStatus)"))
        diagnostics.addItem(action("Run Clipboard Self-Test", #selector(runSelfTest(_:))))
        addSubmenu(diagnostics)

        let preferences = NSMenu(title: "Preferences")
        preferences.autoenablesItems = false
        let loginEnabled = loginStatus == .enabled || loginStatus == .requiresApproval
        preferences.addItem(action(
            loginEnabled ? "Disable Start at Login" : "Enable Start at Login",
            #selector(toggleLoginStartup(_:))
        ))
        preferences.addItem(action("Open Login Item Settings…", #selector(openLoginSettings(_:))))
        if let loginStartupHint {
            preferences.addItem(label(loginStartupHint))
        }

        let start = action("Start IPC on Launch", #selector(toggleIPCOnLaunch(_:)))
        start.state = defaults.bool(forKey: ReimsBridgePreferences.startIPCOnLaunch) ? .on : .off
        preferences.addItem(start)
        let statistics = action("Show Transfer Statistics", #selector(toggleStatistics(_:)))
        statistics.state = defaults.bool(forKey: ReimsBridgePreferences.showTransferStatistics) ? .on : .off
        preferences.addItem(statistics)
        addSubmenu(preferences)

        menu.addItem(.separator())
        let quit = action("Quit ReimsBridge", #selector(quit(_:)))
        quit.keyEquivalent = "q"
        menu.addItem(quit)
    }

    private func label(_ title: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        return item
    }

    private func action(_ title: String, _ selector: Selector) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: selector, keyEquivalent: "")
        item.target = self
        return item
    }

    private func addSubmenu(_ submenu: NSMenu) {
        let item = NSMenuItem(title: submenu.title, action: nil, keyEquivalent: "")
        item.submenu = submenu
        menu.addItem(item)
    }

    private func updateIcon(for state: LocalIPC.State) {
        let (title, symbol) = switch state {
        case .stopped(cleanupError: nil):
            ("ReimsBridge: IPC stopped", "pause.circle")
        case .stopped, .failed:
            ("ReimsBridge: IPC error", "exclamationmark.triangle")
        case .listening(watcherRecentlySeen: false):
            ("ReimsBridge: IPC listening", "arrow.left.arrow.right")
        case .listening(watcherRecentlySeen: true):
            ("ReimsBridge: Windows watcher recently active", "checkmark.circle.fill")
        }

        let image = NSImage(systemSymbolName: symbol, accessibilityDescription: title)
        image?.isTemplate = true
        statusItem.button?.image = image
        statusItem.button?.toolTip = title
    }

    private var validationStatus: String {
        guard let validation, validation.changeCount == clipboard.metadata?.changeCount else {
            return "Not tested"
        }
        return validation.message
    }

    @objc private func toggleIPC(_ sender: NSMenuItem) {
        if ipc.isRunning {
            ipc.stop()
        } else {
            ipc.start()
        }
    }

    @objc private func refreshClipboard(_ sender: NSMenuItem) {
        clipboard.refresh(force: true)
    }

    @objc private func validateClipboard(_ sender: NSMenuItem) {
        let changeCount = NSPasteboard.general.changeCount
        defer { clipboard.refresh(force: true) }

        do {
            let text = try ClipboardService.readText()
            validation = (changeCount, "PASS (\(text.utf8.count) bytes)")
        } catch {
            validation = (changeCount, error.localizedDescription)
        }
    }

    @objc private func runSelfTest(_ sender: NSMenuItem) {
        do {
            let bytes = try ClipboardService.selfTest()
            selfTestStatus = "PASS (\(bytes) bytes)"
        } catch {
            selfTestStatus = "FAIL: \(error.localizedDescription)"
        }
    }

    @objc private func toggleLoginStartup(_ sender: NSMenuItem) {
        let status = SMAppService.mainApp.status
        let loginEnabled = status == .enabled || status == .requiresApproval
        do {
            if loginEnabled {
                try SMAppService.mainApp.unregister()
            } else {
                try SMAppService.mainApp.register()
                defaults.set(true, forKey: ReimsBridgePreferences.startIPCOnLaunch)
            }
            loginStartupHint = nil
        } catch {
            loginStartupHint = loginEnabled
                ? "Remove ReimsBridge from Open at Login"
                : "Add ReimsBridge under Open at Login"
            SMAppService.openSystemSettingsLoginItems()
        }
    }

    @objc private func openLoginSettings(_ sender: NSMenuItem) {
        loginStartupHint = nil
        SMAppService.openSystemSettingsLoginItems()
    }

    @objc private func toggleIPCOnLaunch(_ sender: NSMenuItem) {
        let key = ReimsBridgePreferences.startIPCOnLaunch
        defaults.set(!defaults.bool(forKey: key), forKey: key)
    }

    @objc private func toggleStatistics(_ sender: NSMenuItem) {
        let key = ReimsBridgePreferences.showTransferStatistics
        defaults.set(!defaults.bool(forKey: key), forKey: key)
    }

    @objc private func quit(_ sender: NSMenuItem) {
        NSApplication.shared.terminate(nil)
    }
}
