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
    private var pingTimer: DispatchSourceTimer?
    // FIX-CX1: Connection generation to detect stale callbacks.
    private var connectionGeneration: UInt64 = 0
    // Exponential backoff reconnect state — protected by `queue`.
    private var reconnectAttempt: Int = 0
    private let maxReconnectAttempts: Int = 10
    private var shouldReconnect: Bool = true
    private var reconnectWorkItem: DispatchWorkItem?
    // FIX-CX2: Track if connect was user-initiated vs auto-reconnect.
    private var isAutoReconnect: Bool = false

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

    /// Connect to the lad-relay server. Idempotent — tears down existing connection first.
    public func connect() {
        queue.async { [self] in
            // FIX-CX2: Only reset backoff on explicit user-initiated connect, not auto-reconnect.
            if !isAutoReconnect {
                shouldReconnect = true
                reconnectAttempt = 0
            }
            isAutoReconnect = false
            reconnectWorkItem?.cancel()
            reconnectWorkItem = nil

            // Tear down existing connection before creating a new one.
            if task != nil || session != nil {
                pingTimer?.cancel()
                pingTimer = nil
                task?.cancel(with: .normalClosure, reason: nil)
                task = nil
                session?.invalidateAndCancel()
                session = nil
            }

            // FIX-CX1: Bump generation so stale callbacks from old sessions are ignored.
            connectionGeneration &+= 1
            let myGeneration = connectionGeneration

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
            receiveLoop(generation: myGeneration)
            startPingTimer()
        }
    }

    /// Disconnect gracefully. Breaks URLSession retain cycle and cancels any pending reconnect.
    public func disconnect() {
        queue.async { [self] in
            teardown(stopReconnect: true)
        }
    }

    /// Internal teardown — must be called on `queue`.
    /// - Parameter stopReconnect: if true, clears shouldReconnect and cancels pending work item.
    private func teardown(stopReconnect: Bool) {
        if stopReconnect {
            shouldReconnect = false
            reconnectWorkItem?.cancel()
            reconnectWorkItem = nil
        }
        guard !isDisconnected else { return } // Prevent double-disconnect.
        pingTimer?.cancel()
        pingTimer = nil
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
        // FIX-G7: invalidateAndCancel breaks URLSession → delegate retain cycle.
        session?.invalidateAndCancel()
        session = nil
        // FIX-G13: Preserve .error state so UI shows the reason, don't overwrite with .disconnected.
        if case .error = state {
            // Keep the error message visible.
        } else {
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

    /// FIX-CX1: receiveLoop takes a generation to ignore stale callbacks.
    private func receiveLoop(generation: UInt64) {
        task?.receive { [weak self] result in
            guard let self else { return }
            self.queue.async {
                // FIX-CX1: Ignore callbacks from old connections.
                guard generation == self.connectionGeneration else { return }

                switch result {
                case .success(.string(let text)):
                    self.handleMessage(text)
                    self.receiveLoop(generation: generation)
                case .success(.data(let data)):
                    if let text = String(data: data, encoding: .utf8) {
                        self.handleMessage(text)
                    }
                    self.receiveLoop(generation: generation)
                case .failure(let error):
                    self.logger.error("receive error: \(error.localizedDescription)")
                    self.updateState(.error(error.localizedDescription))
                    let willReconnect = self.shouldReconnect
                    self.teardown(stopReconnect: false)
                    if willReconnect { self.scheduleReconnect() }
                default:
                    self.receiveLoop(generation: generation)
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

    /// Schedule a reconnect attempt with exponential backoff (2^attempt, capped at 60s).
    /// Must be called from within `queue`.
    private func scheduleReconnect() {
        guard shouldReconnect, reconnectAttempt < maxReconnectAttempts else {
            logger.warning("reconnect exhausted after \(self.reconnectAttempt) attempts — giving up")
            return
        }
        let delay = min(pow(2.0, Double(reconnectAttempt)), 60.0)
        reconnectAttempt += 1
        logger.info("scheduling reconnect #\(self.reconnectAttempt) in \(delay)s")

        let item = DispatchWorkItem { [weak self] in
            guard let self, self.shouldReconnect else { return }
            // FIX-CX2: Mark as auto-reconnect so connect() doesn't reset the attempt counter.
            self.isAutoReconnect = true
            self.connect()
        }
        reconnectWorkItem = item
        queue.asyncAfter(deadline: .now() + delay, execute: item)
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
            // FIX-C4: Redact token from logs.
            let safeURL = self.url.absoluteString.replacingOccurrences(
                of: #"token=[^&]*"#, with: "token=***", options: .regularExpression
            )
            logger.info("WebSocket connected to \(safeURL)")
            // Reset backoff counter — successful open means we have a good connection.
            reconnectAttempt = 0
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
            // FIX-CX1: Ignore stale callback from old session.
            guard webSocketTask === self.task else { return }
            logger.info("WebSocket closed: \(closeCode.rawValue)")
            let willReconnect = shouldReconnect
            teardown(stopReconnect: false)
            if willReconnect { scheduleReconnect() }
        }
    }

    public func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: (any Error)?
    ) {
        if let error {
            queue.async { [self] in
                // FIX-CX1: Ignore stale callback from old session.
                guard session === self.session else { return }
                logger.error("session error: \(error.localizedDescription)")
                updateState(.error(error.localizedDescription))
                let willReconnect = shouldReconnect
                teardown(stopReconnect: false)
                if willReconnect { scheduleReconnect() }
            }
        }
    }
}
