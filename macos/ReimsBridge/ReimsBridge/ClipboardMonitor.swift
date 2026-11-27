import AppKit
import Combine
import Foundation

@MainActor
final class ClipboardMonitor: ObservableObject {
    static let shared = ClipboardMonitor()

    @Published private(set) var metadata: ClipboardMetadata?

    private var timer: Timer?

    private init() {}

    func start() {
        guard timer == nil else { return }

        refresh()

        let pollTimer = Timer(timeInterval: 0.5, repeats: true) { [weak self] _ in
            // This timer is scheduled only on the main run loop.
            MainActor.assumeIsolated {
                self?.refresh()
            }
        }
        pollTimer.tolerance = 0.1
        timer = pollTimer
        RunLoop.main.add(pollTimer, forMode: .common)
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    func refresh(force: Bool = false) {
        let pasteboard = NSPasteboard.general
        let changeCount = pasteboard.changeCount
        guard force || metadata?.changeCount != changeCount else { return }

        let snapshot = ClipboardService.inspect(pasteboard)
        guard snapshot.changeCount == changeCount,
              pasteboard.changeCount == changeCount
        else { return }

        metadata = snapshot
    }
}
