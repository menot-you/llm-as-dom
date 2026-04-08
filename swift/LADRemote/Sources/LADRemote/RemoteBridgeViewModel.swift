// RemoteBridgeViewModel — coordinates RelayConnection + BridgeEngine.

#if canImport(UIKit)
import SwiftUI
import Combine

@MainActor
public final class RemoteBridgeViewModel: ObservableObject {
    @Published public var connectionState: RelayConnectionState = .disconnected
    @Published public var currentURL: String = "Waiting for connection..."
    @Published public var lastCommand: String = ""
    @Published public var isPaused: Bool = false

    let engine: BridgeEngine
    private let connection: RelayConnection

    public init(pairingURL: URL) {
        self.connection = RelayConnection(url: pairingURL)
        self.engine = BridgeEngine()
        self.engine.connection = connection
        self.connection.delegate = self
    }

    public func connect() {
        connection.connect()
    }

    public func disconnect() {
        connection.disconnect()
    }

    public func togglePause() {
        isPaused.toggle()
        if isPaused {
            lastCommand = "Paused by user"
        }
    }
}

// MARK: - RelayConnectionDelegate

extension RemoteBridgeViewModel: RelayConnectionDelegate {
    public func connectionStateChanged(_ state: RelayConnectionState) {
        connectionState = state
        switch state {
        case .connected:
            currentURL = "Connected — waiting for commands"
        case .connecting:
            currentURL = "Connecting..."
        case .disconnected:
            currentURL = "Disconnected"
            lastCommand = ""
        case .error(let msg):
            currentURL = "Error: \(msg)"
        case .paused:
            currentURL = "Paused"
        }
    }

    public func didReceiveCommand(_ command: BridgeCommand) {
        guard !isPaused else { return }

        // Update UI.
        lastCommand = "\(command.cmd)"
        if let url = command.url {
            lastCommand += " → \(url)"
        }
        if command.cmd == "navigate", let url = command.url {
            currentURL = url
        }

        // Dispatch to engine.
        engine.dispatch(command)
    }
}

#endif
