// BridgeEngine — dispatches NDJSON commands to WKWebView.
//
// Port of the macOS webkit-bridge BridgeApp.dispatch() to iOS.
// WKWebView must be in the active view hierarchy for screenshots to work.

import Foundation
import WebKit
import os

/// Drives a WKWebView based on commands received from the relay.
@MainActor
public final class BridgeEngine: NSObject {
    private let logger = Logger(subsystem: "im.nott.lad", category: "BridgeEngine")

    public let webView: WKWebView
    public weak var connection: RelayConnection?

    private var navDelegate: BridgeNavDelegate!
    private var uiDelegate: BridgeUIDelegate!
    private var monitoringTimer: Timer?

    deinit {
        // FIX-G4: Prevent timer leak if engine is destroyed without stop_monitoring.
        monitoringTimer?.invalidate()
    }

    public override init() {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.defaultWebpagePreferences.allowsContentJavaScript = true

        // Console capture injection.
        let consoleJS = """
        (function() {
            ['log','warn','error','info','debug'].forEach(function(level) {
                var orig = console[level];
                console[level] = function() {
                    var args = Array.prototype.slice.call(arguments);
                    orig.apply(console, args);
                    try {
                        window.webkit.messageHandlers.ladConsole.postMessage({
                            level: level,
                            message: args.map(function(a) {
                                if (typeof a === 'object') {
                                    try { return JSON.stringify(a); }
                                    catch(e) { return String(a); }
                                }
                                return String(a);
                            }).join(' ')
                        });
                    } catch(e) {}
                };
            });
        })();
        """
        let script = WKUserScript(
            source: consoleJS,
            injectionTime: .atDocumentStart,
            forMainFrameOnly: false
        )
        config.userContentController.addUserScript(script)

        webView = WKWebView(frame: .zero, configuration: config)
        webView.allowsBackForwardNavigationGestures = true

        super.init()

        // Console handler.
        config.userContentController.add(self, name: "ladConsole")

        // Navigation delegate.
        navDelegate = BridgeNavDelegate(engine: self)
        webView.navigationDelegate = navDelegate

        // FIX-G5: UI delegate handles alert/confirm/prompt and target="_blank".
        uiDelegate = BridgeUIDelegate(engine: self)
        webView.uiDelegate = uiDelegate
    }

    // MARK: - Command Dispatch

    public func dispatch(_ cmd: BridgeCommand) {
        switch cmd.cmd {
        case "navigate":
            guard let urlStr = cmd.url, let url = URL(string: urlStr) else {
                respond(.error(cmd.id, "missing or invalid url"))
                return
            }
            webView.load(URLRequest(url: url))
            respond(.ok(cmd.id))

        case "eval_js":
            guard let script = cmd.script else {
                respond(.error(cmd.id, "missing script"))
                return
            }
            webView.evaluateJavaScript(script) { [weak self] result, error in
                guard let self else { return }
                if let error {
                    self.respond(.error(cmd.id, error.localizedDescription))
                } else {
                    let value = self.serializeJSResult(result)
                    var resp = BridgeResponse.ok(cmd.id)
                    resp.value = value
                    self.respond(resp)
                }
            }

        case "start_monitoring":
            guard let script = cmd.script else {
                respond(.error(cmd.id, "missing script"))
                return
            }
            let interval = cmd.interval ?? 1000
            monitoringTimer?.invalidate()
            monitoringTimer = Timer.scheduledTimer(
                withTimeInterval: Double(interval) / 1000.0,
                repeats: true
            ) { [weak self] _ in
                guard let self else { return }
                Task { @MainActor in
                    self.webView.evaluateJavaScript(script) { result, _ in
                        let value = self.serializeJSResult(result)
                        self.sendEvent("monitor", extra: ["result": value])
                    }
                }
            }
            respond(.ok(cmd.id))

        case "stop_monitoring":
            monitoringTimer?.invalidate()
            monitoringTimer = nil
            respond(.ok(cmd.id))

        case "wait_for_navigation":
            if !webView.isLoading {
                respond(.ok(cmd.id))
            } else {
                navDelegate.addPendingWait(cmd.id)
                // FIX-G10: weak self prevents 30s artificial retain.
                Task { @MainActor [weak self] in
                    try? await Task.sleep(for: .seconds(30))
                    guard let self else { return }
                    if self.navDelegate.removePendingWait(cmd.id) {
                        self.respond(.error(cmd.id, "navigation timeout"))
                    }
                }
            }

        case "url":
            var resp = BridgeResponse.ok(cmd.id)
            resp.value = .string(webView.url?.absoluteString ?? "about:blank")
            respond(resp)

        case "title":
            var resp = BridgeResponse.ok(cmd.id)
            resp.value = .string(webView.title ?? "")
            respond(resp)

        case "screenshot":
            let config = WKSnapshotConfiguration()
            webView.takeSnapshot(with: config) { [weak self] image, error in
                guard let self else { return }
                if let error {
                    self.respond(.error(cmd.id, error.localizedDescription))
                    return
                }
                guard let image,
                      let data = self.pngData(from: image) else {
                    self.respond(.error(cmd.id, "screenshot conversion failed"))
                    return
                }
                var resp = BridgeResponse.ok(cmd.id)
                resp.png_b64 = data.base64EncodedString()
                self.respond(resp)
            }

        case "cookies":
            webView.configuration.websiteDataStore.httpCookieStore.getAllCookies { [weak self] cookies in
                guard let self else { return }
                let mapped = cookies.map { c in
                    CookieData(
                        name: c.name,
                        value: c.value,
                        domain: c.domain,
                        path: c.path,
                        expires: c.expiresDate?.timeIntervalSince1970,
                        secure: c.isSecure,
                        httpOnly: c.isHTTPOnly,
                        sameSite: {
                            switch c.sameSitePolicy {
                            case .sameSiteLax: return "Lax"
                            case .sameSiteStrict: return "Strict"
                            default: return nil
                            }
                        }()
                    )
                }
                var resp = BridgeResponse.ok(cmd.id)
                resp.cookies = mapped
                self.respond(resp)
            }

        case "set_cookies":
            guard let cookiesData = cmd.cookies else {
                respond(.error(cmd.id, "missing cookies"))
                return
            }
            let store = webView.configuration.websiteDataStore.httpCookieStore
            let group = DispatchGroup()
            for cd in cookiesData {
                var props: [HTTPCookiePropertyKey: Any] = [
                    .name: cd.name,
                    .value: cd.value,
                    .domain: cd.domain,
                    .path: cd.path,
                ]
                if let expires = cd.expires, expires > 0 {
                    props[.expires] = Date(timeIntervalSince1970: expires)
                }
                if cd.secure == true {
                    props[.secure] = "TRUE"
                }
                if cd.httpOnly == true {
                    props[HTTPCookiePropertyKey("HttpOnly")] = "TRUE"
                }
                if let sameSite = cd.sameSite {
                    props[.init("SameSite")] = sameSite
                }
                if let cookie = HTTPCookie(properties: props) {
                    group.enter()
                    store.setCookie(cookie) { group.leave() }
                }
            }
            group.notify(queue: .main) { [weak self] in
                self?.respond(.ok(cmd.id))
            }

        case "close":
            respond(.ok(cmd.id))
            // Don't actually close the app — just disconnect.
            connection?.disconnect()

        default:
            respond(.error(cmd.id, "unknown command: \(cmd.cmd)"))
        }
    }

    // MARK: - Response helpers

    func respond(_ response: BridgeResponse) {
        connection?.send(response)
    }

    func sendEvent(_ type: String, extra: [String: AnyCodable] = [:]) {
        let resp = BridgeResponse.event(type, extra: extra)
        connection?.send(resp)
    }

    func resolveNavigation(id: UInt64, ok: Bool, error: String? = nil) {
        if ok {
            respond(.ok(id))
        } else {
            respond(.error(id, error ?? "navigation failed"))
        }
    }

    // MARK: - Screenshot helpers

    #if canImport(UIKit)
    private func pngData(from image: UIImage) -> Data? {
        image.pngData()
    }
    #else
    private func pngData(from image: NSImage) -> Data? {
        guard let tiff = image.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff) else { return nil }
        return bitmap.representation(using: .png, properties: [:])
    }
    #endif

    // MARK: - JS Result Serialization

    private func serializeJSResult(_ result: Any?) -> AnyCodable {
        switch result {
        case nil:
            return .null
        case is NSNull:
            return .null
        case let str as String:
            return .string(str)
        case let num as NSNumber:
            if CFBooleanGetTypeID() == CFGetTypeID(num) {
                return .bool(num.boolValue)
            }
            if num.doubleValue == Double(num.intValue) {
                return .int(num.intValue)
            }
            return .double(num.doubleValue)
        case let arr as [Any]:
            return .array(arr.map { serializeJSResult($0) })
        case let dict as [String: Any]:
            return .dictionary(dict.mapValues { serializeJSResult($0) })
        default:
            return .null
        }
    }
}

// MARK: - WKScriptMessageHandler (Console capture)

extension BridgeEngine: WKScriptMessageHandler {
    public func userContentController(
        _ controller: WKUserContentController,
        didReceive message: WKScriptMessage
    ) {
        guard let body = message.body as? [String: Any],
              let level = body["level"] as? String,
              let text = body["message"] as? String else { return }
        sendEvent("console", extra: [
            "level": .string(level),
            "message": .string(text),
        ])
    }
}

// MARK: - Navigation Delegate

@MainActor
final class BridgeNavDelegate: NSObject, WKNavigationDelegate {
    private weak var engine: BridgeEngine?
    private var pendingWaits: Set<UInt64> = []

    init(engine: BridgeEngine) {
        self.engine = engine
    }

    func addPendingWait(_ id: UInt64) {
        pendingWaits.insert(id)
    }

    /// Returns true if the id was pending (and is now removed).
    func removePendingWait(_ id: UInt64) -> Bool {
        pendingWaits.remove(id) != nil
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        engine?.sendEvent("load", extra: [
            "url": .string(webView.url?.absoluteString ?? ""),
        ])
        for waitId in pendingWaits {
            engine?.resolveNavigation(id: waitId, ok: true)
        }
        pendingWaits.removeAll()
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        for waitId in pendingWaits {
            engine?.resolveNavigation(id: waitId, ok: false, error: error.localizedDescription)
        }
        pendingWaits.removeAll()
    }

    func webView(
        _ webView: WKWebView,
        didFailProvisionalNavigation navigation: WKNavigation!,
        withError error: Error
    ) {
        for waitId in pendingWaits {
            engine?.resolveNavigation(id: waitId, ok: false, error: error.localizedDescription)
        }
        pendingWaits.removeAll()
    }
}

// MARK: - UI Delegate (FIX-G5)

/// Handles JS dialogs (alert/confirm/prompt) and target="_blank" links.
@MainActor
final class BridgeUIDelegate: NSObject, WKUIDelegate {
    private weak var engine: BridgeEngine?

    init(engine: BridgeEngine) {
        self.engine = engine
    }

    // Handle target="_blank" by loading in the same webView.
    func webView(
        _ webView: WKWebView,
        createWebViewWith configuration: WKWebViewConfiguration,
        for navigationAction: WKNavigationAction,
        windowFeatures: WKWindowFeatures
    ) -> WKWebView? {
        if navigationAction.targetFrame == nil || navigationAction.targetFrame?.isMainFrame == false {
            webView.load(navigationAction.request)
        }
        return nil
    }

    // Handle alert().
    func webView(
        _ webView: WKWebView,
        runJavaScriptAlertPanelWithMessage message: String,
        initiatedByFrame frame: WKFrameInfo,
        completionHandler: @escaping () -> Void
    ) {
        engine?.sendEvent("dialog", extra: [
            "type": .string("alert"),
            "message": .string(message),
        ])
        completionHandler()
    }

    // Handle confirm().
    func webView(
        _ webView: WKWebView,
        runJavaScriptConfirmPanelWithMessage message: String,
        initiatedByFrame frame: WKFrameInfo,
        completionHandler: @escaping (Bool) -> Void
    ) {
        engine?.sendEvent("dialog", extra: [
            "type": .string("confirm"),
            "message": .string(message),
        ])
        completionHandler(true) // Auto-accept.
    }

    // Handle prompt().
    func webView(
        _ webView: WKWebView,
        runJavaScriptTextInputPanelWithPrompt prompt: String,
        defaultText: String?,
        initiatedByFrame frame: WKFrameInfo,
        completionHandler: @escaping (String?) -> Void
    ) {
        engine?.sendEvent("dialog", extra: [
            "type": .string("prompt"),
            "message": .string(prompt),
        ])
        completionHandler(defaultText)
    }
}
