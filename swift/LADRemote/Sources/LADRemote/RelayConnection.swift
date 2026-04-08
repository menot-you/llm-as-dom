// LADRemote — WebSocket client that connects to lad-relay.
//
// The iPhone connects outbound to the desktop's lad-relay WebSocket server.
// Receives NDJSON commands, dispatches to WKWebView, returns responses.
//
// Round 1 fixes: G1 (queue), G2 (NDJSON framing), G3 (ping), G4 (deinit leaks)
// Round 2 fixes: G7 (retain cycle), G8 (DispatchSourceTimer), G9 (receive teardown)

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
///
/// Thread safety: all mutable state is protected by `queue` (serial DispatchQueue).
/// Delegate calls are dispatched to MainActor.
public final class RelayConnection: NSObject, @unchecked Sendable {
    private let url: URL
    private let logger = Logger(subsystem: "im.nott.lad", category: "RelayConnection")
    private let queue = DispatchQueue(label: "im.nott.lad.relay", qos: .userInitiated)

    // Protected by `queue`.
    private var task: URLSessionWebSocketTask?
    private var session: URLSession?
    private var state: RelayConnectionState = .disconnected
    // FIX-G8: DispatchSourceTimer instead of Timer to avoid main-thread data race.
    private var pingTimer: DispatchSourceTimer?

    @MainActor public weak var delegate: RelayConnectionDelegate?

    /// Initialize with a pairing URL (e.g., ws://192.168.1.42:9876?token=123456).
    public init(url: URL) {
        self.url = url
        super.init()
    }

    deinit {
        session?.invalidateAndCancel()
        pingTimer?.cancel()
    }

    /// Connect to the lad-relay server.
    public func connect() {
        queue.async { [self] in
            let config = URLSessionConfiguration.default
            config.waitsForConnectivity = true
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
            startPingTimer()
        }
    }

    /// Disconnect gracefully. Breaks URLSession retain cycle.
    public func disconnect() {
        queue.async { [self] in
            guard !isDisconnected else { return } // Prevent double-disconnect.
            pingTimer?.cancel()
            pingTimer = nil
            task?.cancel(with: .normalClosure, reason: nil)
            task = nil
            // FIX-G7: invalidateAndCancel breaks URLSession → delegate retain cycle.
            session?.invalidateAndCancel()
            session = nil
            updateState(.disconnected)
        }
    }

    /// Send a JSON response back to lad-relay (NDJSON: appends \n).
    public func send(_ response: BridgeResponse) {
        queue.async { [self] in
            guard let task else {
                logger.warning("send called but no active connection")
                return
            }

            do {
                let data = try JSONEncoder().encode(response)
                guard var text = String(data: data, encoding: .utf8) else { return }
                if !text.hasSuffix("\n") { text.append("\n") }
                task.send(.string(text)) { [weak self] error in
                    if let error {
                        self?.logger.error("send failed: \(error.localizedDescription)")
                    }
                }
            } catch {
                logger.error("encode failed: \(error.localizedDescription)")
            }
        }
    }

    /// Send a raw JSON string (for events). Appends \n for NDJSON compliance.
    public func sendRaw(_ json: String) {
        queue.async { [self] in
            var line = json
            if !line.hasSuffix("\n") { line.append("\n") }
            task?.send(.string(line)) { [weak self] error in
                if let error {
                    self?.logger.error("sendRaw failed: \(error.localizedDescription)")
                }
            }
        }
    }

    // MARK: - Private

    private func receiveLoop() {
        task?.receive { [weak self] result in
            guard let self else { return }
            self.queue.async {
                switch result {
                case .success(.string(let text)):
                    self.handleMessage(text)
                    self.receiveLoop()
                case .success(.data(let data)):
                    if let text = String(data: data, encoding: .utf8) {
                        self.handleMessage(text)
                    }
                    self.receiveLoop()
                case .failure(let error):
                    self.logger.error("receive error: \(error.localizedDescription)")
                    self.updateState(.error(error.localizedDescription))
                    // FIX-G9: Tear down on fatal receive error.
                    self.disconnect()
                default:
                    self.receiveLoop()
                }
            }
        }
    }

    private func handleMessage(_ text: String) {
        let lines = text.components(separatedBy: "\n").filter { !$0.isEmpty }
        for line in lines {
            guard let data = line.data(using: .utf8) else { continue }
            do {
                let command = try JSONDecoder().decode(BridgeCommand.self, from: data)
                Task { @MainActor [weak self] in
                    self?.delegate?.didReceiveCommand(command)
                }
            } catch {
                logger.warning("failed to parse command: \(error.localizedDescription)")
            }
        }
    }

    /// FIX-G8: DispatchSourceTimer on `queue` — no main-thread data race.
    private func startPingTimer() {
        pingTimer?.cancel()
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 30, repeating: 30)
        timer.setEventHandler { [weak self] in
            self?.task?.sendPing { error in
                if let error {
                    self?.logger.warning("ping failed: \(error.localizedDescription)")
                }
            }
        }
        timer.resume()
        pingTimer = timer
    }

    private var isDisconnected: Bool {
        if case .disconnected = state { return true }
        return false
    }

    private func updateState(_ newState: RelayConnectionState) {
        state = newState
        Task { @MainActor [weak self] in
            self?.delegate?.connectionStateChanged(newState)
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
        queue.async { [self] in
            logger.info("WebSocket connected to \(self.url.absoluteString)")
            updateState(.connected)
            sendRaw(#"{"event":"ready","version":"0.1.0","engine":"ios-webkit"}"#)
        }
    }

    public func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didCloseWith closeCode: URLSessionWebSocketTask.CloseCode,
        reason: Data?
    ) {
        queue.async { [self] in
            logger.info("WebSocket closed: \(closeCode.rawValue)")
            // FIX-G7: Break URLSession retain cycle on server-initiated close.
            disconnect()
        }
    }

    public func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: (any Error)?
    ) {
        if let error {
            queue.async { [self] in
                logger.error("session error: \(error.localizedDescription)")
                updateState(.error(error.localizedDescription))
                // FIX-G7: Break URLSession retain cycle on error.
                disconnect()
            }
        }
    }
}
