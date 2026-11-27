import Combine

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
