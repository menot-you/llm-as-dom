// LADRemote — WebSocket client that connects to lad-relay.
//
// The iPhone connects outbound to the desktop's lad-relay WebSocket server.
// Receives NDJSON commands, dispatches to WKWebView, returns responses.

import Foundation
import os

/// Connection state for the relay link.
public enum RelayConnectionState: Sendable {
    case disconnected
    case connecting
    case connected
    case paused
    case error(String)
}

/// Delegate protocol for relay connection events.
@MainActor
public protocol RelayConnectionDelegate: AnyObject {
    func connectionStateChanged(_ state: RelayConnectionState)
    func didReceiveCommand(_ command: BridgeCommand)
}

/// Manages the WebSocket connection to the desktop lad-relay server.
public final class RelayConnection: NSObject, Sendable {
    private let url: URL
    private let logger = Logger(subsystem: "im.nott.lad", category: "RelayConnection")

    // nonisolated(unsafe) because URLSessionWebSocketTask is not Sendable
    // but we only access it from the internal serial queue.
    nonisolated(unsafe) private var task: URLSessionWebSocketTask?
    nonisolated(unsafe) private var session: URLSession?
    nonisolated(unsafe) private var _state: RelayConnectionState = .disconnected

    @MainActor public weak var delegate: RelayConnectionDelegate?

    /// Initialize with a pairing URL (e.g., ws://192.168.1.42:9876?token=123456).
    public init(url: URL) {
        self.url = url
        super.init()
    }

    /// Connect to the lad-relay server.
    public func connect() {
        let config = URLSessionConfiguration.default
        config.waitsForConnectivity = true
        // Keep connection alive.
        config.timeoutIntervalForRequest = 300
        config.timeoutIntervalForResource = 3600

        let session = URLSession(configuration: config, delegate: self, delegateQueue: nil)
        self.session = session

        let task = session.webSocketTask(with: url)
        task.maximumMessageSize = 16 * 1024 * 1024 // 16 MB for screenshots
        self.task = task

        updateState(.connecting)
        task.resume()
        receiveLoop()
    }

    /// Disconnect gracefully.
    public func disconnect() {
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
        session?.invalidateAndCancel()
        session = nil
        updateState(.disconnected)
    }

    /// Send a JSON response back to lad-relay → LAD.
    public func send(_ response: BridgeResponse) {
        guard let task else {
            logger.warning("send called but no active connection")
            return
        }

        do {
            let data = try JSONEncoder().encode(response)
            guard let text = String(data: data, encoding: .utf8) else { return }
            task.send(.string(text)) { [weak self] error in
                if let error {
                    self?.logger.error("send failed: \(error.localizedDescription)")
                }
            }
        } catch {
            logger.error("encode failed: \(error.localizedDescription)")
        }
    }

    /// Send a raw JSON string (for events).
    public func sendRaw(_ json: String) {
        task?.send(.string(json)) { [weak self] error in
            if let error {
                self?.logger.error("sendRaw failed: \(error.localizedDescription)")
            }
        }
    }

    // MARK: - Private

    private func receiveLoop() {
        task?.receive { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(.string(let text)):
                self.handleMessage(text)
                self.receiveLoop() // Continue listening.
            case .success(.data(let data)):
                if let text = String(data: data, encoding: .utf8) {
                    self.handleMessage(text)
                }
                self.receiveLoop()
            case .failure(let error):
                self.logger.error("receive error: \(error.localizedDescription)")
                self.updateState(.error(error.localizedDescription))
            default:
                self.receiveLoop()
            }
        }
    }

    private func handleMessage(_ text: String) {
        // Parse NDJSON line into BridgeCommand.
        guard let data = text.data(using: .utf8) else { return }
        do {
            let command = try JSONDecoder().decode(BridgeCommand.self, from: data)
            Task { @MainActor in
                self.delegate?.didReceiveCommand(command)
            }
        } catch {
            logger.warning("failed to parse command: \(error.localizedDescription)")
        }
    }

    private func updateState(_ state: RelayConnectionState) {
        _state = state
        Task { @MainActor in
            self.delegate?.connectionStateChanged(state)
        }
    }
}

// MARK: - URLSessionWebSocketDelegate

extension RelayConnection: URLSessionWebSocketDelegate {
    public func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didOpenWithProtocol protocol: String?
    ) {
        logger.info("WebSocket connected to \(self.url.absoluteString)")
        updateState(.connected)

        // Send ready event.
        sendRaw(#"{"event":"ready","version":"0.1.0","engine":"ios-webkit"}"#)
    }

    public func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didCloseWith closeCode: URLSessionWebSocketTask.CloseCode,
        reason: Data?
    ) {
        logger.info("WebSocket closed: \(closeCode.rawValue)")
        updateState(.disconnected)
    }

    public func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: (any Error)?
    ) {
        if let error {
            logger.error("session error: \(error.localizedDescription)")
            updateState(.error(error.localizedDescription))
        }
    }
}
