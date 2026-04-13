// RemoteBridgeView — SwiftUI view that shows the live WKWebView
// being piloted by LAD, with a status overlay.

#if canImport(UIKit)
import SwiftUI
import WebKit

/// Main view for the LAD Remote Control feature.
/// Shows the WKWebView being piloted + a status overlay.
public struct RemoteBridgeView: View {
    @StateObject private var viewModel: RemoteBridgeViewModel

    public init(pairingURL: URL) {
        _viewModel = StateObject(wrappedValue: RemoteBridgeViewModel(pairingURL: pairingURL))
    }

    public var body: some View {
        ZStack {
            // Full-screen WKWebView.
            WebViewWrapper(webView: viewModel.engine.webView)
                .ignoresSafeArea()

            // Status overlay at top.
            VStack {
                StatusOverlay(
                    state: viewModel.connectionState,
                    currentURL: viewModel.currentURL,
                    lastCommand: viewModel.lastCommand,
                    isPaused: viewModel.isPaused
                )
                .padding(.horizontal, 16)
                .padding(.top, 8)

                Spacer()

                // Bottom controls.
                HStack(spacing: 24) {
                    Button(action: { viewModel.togglePause() }) {
                        Image(systemName: viewModel.isPaused ? "play.fill" : "pause.fill")
                            .font(.title2)
                            .foregroundStyle(.white)
                            .frame(width: 44, height: 44)
                            .background(.ultraThinMaterial, in: Circle())
                    }

                    Button(action: { viewModel.disconnect() }) {
                        Image(systemName: "xmark.circle.fill")
                            .font(.title2)
                            .foregroundStyle(.red)
                            .frame(width: 44, height: 44)
                            .background(.ultraThinMaterial, in: Circle())
                    }
                }
                .padding(.bottom, 24)
            }

            // Active piloting border indicator.
            if viewModel.connectionState == .connected && !viewModel.isPaused {
                RoundedRectangle(cornerRadius: 0)
                    .stroke(Color.green.opacity(0.6), lineWidth: 3)
                    .ignoresSafeArea()
                    .allowsHitTesting(false)
            }
        }
        .onAppear { viewModel.connect() }
        .onDisappear { viewModel.disconnect() }
    }
}

// MARK: - Status Overlay

struct StatusOverlay: View {
    let state: RelayConnectionState
    let currentURL: String
    let lastCommand: String
    let isPaused: Bool

    var body: some View {
        HStack(spacing: 8) {
            // Connection indicator.
            Circle()
                .fill(indicatorColor)
                .frame(width: 8, height: 8)

            VStack(alignment: .leading, spacing: 2) {
                Text(currentURL)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)

                if !lastCommand.isEmpty {
                    Text(lastCommand)
                        .font(.caption2.bold())
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                }
            }

            Spacer()

            if isPaused {
                Text("PAUSED")
                    .font(.caption2.bold())
                    .foregroundStyle(.orange)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12))
    }

    private var indicatorColor: Color {
        switch state {
        case .connected: return .green
        case .connecting: return .yellow
        case .paused: return .orange
        case .disconnected: return .gray
        case .error: return .red
        }
    }
}

// MARK: - WKWebView UIViewRepresentable

struct WebViewWrapper: UIViewRepresentable {
    let webView: WKWebView

    func makeUIView(context: Context) -> WKWebView {
        webView.translatesAutoresizingMaskIntoConstraints = false
        return webView
    }

    func updateUIView(_ uiView: WKWebView, context: Context) {}
}

// MARK: - Extension for Equatable conformance on state

extension RelayConnectionState: Equatable {
    public static func == (lhs: RelayConnectionState, rhs: RelayConnectionState) -> Bool {
        switch (lhs, rhs) {
        case (.disconnected, .disconnected),
             (.connecting, .connecting),
             (.connected, .connected),
             (.paused, .paused):
            return true
        case (.error(let a), .error(let b)):
            return a == b
        default:
            return false
        }
    }
}

#endif
