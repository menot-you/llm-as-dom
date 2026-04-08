// LADRemote — WebSocket client that connects to lad-relay.
//
// The iPhone connects outbound to the desktop's lad-relay WebSocket server.
// Receives NDJSON commands, dispatches to WKWebView, returns responses.
//
// FIX-G1: Removed nonisolated(unsafe), use DispatchQueue for thread safety.
// FIX-G2: NDJSON framing — append \n to sends, split multi-line receives.
// FIX-G3: WebSocket keep-alive via periodic ping.

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
    private var pingTimer: Timer?

    @MainActor public weak var delegate: RelayConnectionDelegate?

    /// Initialize with a pairing URL (e.g., ws://192.168.1.42:9876?token=123456).
    public init(url: URL) {
        self.url = url
        super.init()
    }

    deinit {
        // FIX-G4: Prevent URLSession delegate retention leak.
        session?.invalidateAndCancel()
        pingTimer?.invalidate()
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

    /// Disconnect gracefully.
    public func disconnect() {
        queue.async { [self] in
            pingTimer?.invalidate()
            pingTimer = nil
            task?.cancel(with: .normalClosure, reason: nil)
            task = nil
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
                // FIX-G2: NDJSON requires trailing newline.
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
        // dispatchPrecondition(condition: .onQueue(queue)) — can't check, called from callback too.
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
                default:
                    self.receiveLoop()
                }
            }
        }
    }

    private func handleMessage(_ text: String) {
        // FIX-G2: A single WebSocket frame may contain multiple NDJSON lines.
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

    /// FIX-G3: Periodic ping to keep WebSocket alive through NAT.
    private func startPingTimer() {
        DispatchQueue.main.async { [weak self] in
            self?.pingTimer?.invalidate()
            self?.pingTimer = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in
                self?.queue.async {
                    self?.task?.sendPing { error in
                        if let error {
                            self?.logger.warning("ping failed: \(error.localizedDescription)")
                        }
                    }
                }
            }
        }
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
            updateState(.disconnected)
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
            }
        }
    }
}
