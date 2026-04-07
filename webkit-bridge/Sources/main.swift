// LadWebKitBridge — stdin/stdout JSON bridge for WKWebView.
//
// Protocol: newline-delimited JSON (NDJSON).
// Rust (lad) writes commands to stdin, reads responses + events from stdout.
// All WKWebView work runs on the main thread via DispatchQueue.main.

import Cocoa
import WebKit

// MARK: - Protocol Types

struct Command: Decodable {
    let id: UInt64
    let cmd: String
    var url: String?
    var script: String?
    var cookies: [CookieData]?
    var visible: Bool?
    var width: Int?
    var height: Int?
    var interval: Int?
}

struct CookieData: Codable {
    let name: String
    let value: String
    let domain: String
    let path: String
    var expires: Double?
    var secure: Bool?
    var httpOnly: Bool?
    var sameSite: String?
}

// MARK: - Thread-safe stdout writer

final class StdoutWriter {
    private let lock = NSLock()

    func writeLine(_ dict: [String: Any]) {
        lock.lock()
        defer { lock.unlock() }
        guard let data = try? JSONSerialization.data(withJSONObject: dict, options: []),
              let json = String(data: data, encoding: .utf8) else { return }
        FileHandle.standardOutput.write(Data((json + "\n").utf8))
    }

    func respond(_ id: UInt64, ok: Bool, extra: [String: Any] = [:]) {
        var dict: [String: Any] = ["id": id, "ok": ok]
        for (k, v) in extra { dict[k] = v }
        writeLine(dict)
    }

    func respondError(_ id: UInt64, _ message: String) {
        respond(id, ok: false, extra: ["error": message])
    }

    func event(_ type: String, extra: [String: Any] = [:]) {
        var dict: [String: Any] = ["event": type]
        for (k, v) in extra { dict[k] = v }
        writeLine(dict)
    }
}

// MARK: - Console capture handler

final class ConsoleHandler: NSObject, WKScriptMessageHandler {
    let writer: StdoutWriter

    init(writer: StdoutWriter) {
        self.writer = writer
    }

    func userContentController(
        _ controller: WKUserContentController,
        didReceive message: WKScriptMessage
    ) {
        guard let body = message.body as? [String: Any],
              let level = body["level"] as? String,
              let text = body["message"] as? String else { return }
        writer.event("console", extra: ["level": level, "message": text])
    }
}

// MARK: - Navigation delegate

final class NavDelegate: NSObject, WKNavigationDelegate {
    let writer: StdoutWriter
    /// Pending wait_for_navigation completions keyed by request id.
    var pendingWaits: Set<UInt64> = []
    let lock = NSLock()

    init(writer: StdoutWriter) {
        self.writer = writer
    }

    func addPendingWait(_ id: UInt64) {
        lock.lock()
        pendingWaits.insert(id)
        lock.unlock()
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        writer.event("load", extra: ["url": webView.url?.absoluteString ?? ""])
        // Resolve all pending wait_for_navigation requests.
        lock.lock()
        let pending = pendingWaits
        pendingWaits.removeAll()
        lock.unlock()
        for waitId in pending {
            writer.respond(waitId, ok: true)
        }
    }

    func webView(
        _ webView: WKWebView,
        didFail navigation: WKNavigation!,
        withError error: Error
    ) {
        lock.lock()
        let pending = pendingWaits
        pendingWaits.removeAll()
        lock.unlock()
        for waitId in pending {
            writer.respondError(waitId, error.localizedDescription)
        }
    }

    func webView(
        _ webView: WKWebView,
        didFailProvisionalNavigation navigation: WKNavigation!,
        withError error: Error
    ) {
        lock.lock()
        let pending = pendingWaits
        pendingWaits.removeAll()
        lock.unlock()
        for waitId in pending {
            writer.respondError(waitId, error.localizedDescription)
        }
    }
}

// MARK: - Bridge app

final class BridgeApp: NSObject, NSApplicationDelegate {
    let writer = StdoutWriter()
    var webView: WKWebView!
    var window: NSWindow!
    var navDelegate: NavDelegate!
    var showWindow = false
    var windowWidth = 1280
    var windowHeight = 800
    var monitoringTimer: Timer?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Parse initial config from env or first stdin line will configure.
        showWindow = ProcessInfo.processInfo.environment["LAD_WEBKIT_VISIBLE"] == "1"
        if let w = ProcessInfo.processInfo.environment["LAD_WEBKIT_WIDTH"],
           let wInt = Int(w) { windowWidth = wInt }
        if let h = ProcessInfo.processInfo.environment["LAD_WEBKIT_HEIGHT"],
           let hInt = Int(h) { windowHeight = hInt }

        if let cls = NSClassFromString("WKBrowsingContextController") as? NSObject.Type {
            let sel = NSSelectorFromString("registerSchemeForCustomProtocol:")
            if cls.responds(to: sel) {
                cls.perform(sel, with: "http")
                cls.perform(sel, with: "https")
            }
        }
        URLProtocol.registerClass(NetworkMonitorProtocol.self)

        setupWebView()

        // Read stdin on background thread.
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.readLoop()
        }

        writer.event("ready", extra: ["version": "0.1.1"])
    }

    private func setupWebView() {
        let config = WKWebViewConfiguration()

        // Session isolation: use ephemeral or custom data store
        if let dataDir = ProcessInfo.processInfo.environment["LAD_WEBKIT_DATA_DIR"],
           !dataDir.isEmpty {
            // Custom persistent store for explicit session reuse
            if #available(macOS 14.0, *) {
                config.websiteDataStore = WKWebsiteDataStore(forIdentifier: UUID(uuidString: dataDir) ?? UUID())
            } else {
                // Fallback: non-persistent for older macOS
                config.websiteDataStore = .nonPersistent()
            }
        } else {
            // Default: non-persistent (ephemeral, no cookie leak between sessions)
            config.websiteDataStore = .nonPersistent()
        }

        config.preferences.setValue(true, forKey: "developerExtrasEnabled")

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
        config.userContentController.add(
            ConsoleHandler(writer: writer), name: "ladConsole"
        )

        // Create offscreen window (needed for screenshots even in headless).
        let frame = NSRect(x: 0, y: 0, width: windowWidth, height: windowHeight)
        window = NSWindow(
            contentRect: frame,
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "lad-webkit-bridge"

        webView = WKWebView(frame: frame, configuration: config)
        webView.autoresizingMask = [.width, .height]
        window.contentView?.addSubview(webView)

        navDelegate = NavDelegate(writer: writer)
        webView.navigationDelegate = navDelegate

        if showWindow {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    // MARK: - stdin reader

    private func readLoop() {
        while let line = readLine() {
            autoreleasepool {
                guard !line.isEmpty,
                      let data = line.data(using: .utf8) else { return }

                let cmd: Command
                do {
                    cmd = try JSONDecoder().decode(Command.self, from: data)
                } catch {
                    // Can't respond without an id.
                    writer.event("error", extra: ["message": "invalid JSON: \(error.localizedDescription)"])
                    return  // return from autoreleasepool closure, not readLoop
                }

                DispatchQueue.main.async { [weak self] in
                    self?.dispatch(cmd)
                }
            }
        }
        // stdin closed — exit gracefully.
        DispatchQueue.main.async {
            NSApp.terminate(nil)
        }
    }

    // MARK: - Command dispatch

    private func dispatch(_ cmd: Command) {
        switch cmd.cmd {

        case "navigate":
            guard let urlStr = cmd.url, let url = URL(string: urlStr) else {
                writer.respondError(cmd.id, "missing or invalid url")
                return
            }
            webView.load(URLRequest(url: url))
            // Respond immediately — use wait_for_navigation for completion.
            writer.respond(cmd.id, ok: true)

        case "eval_js":
            guard let script = cmd.script else {
                writer.respondError(cmd.id, "missing script")
                return
            }
            webView.evaluateJavaScript(script) { [weak self] result, error in
                guard let self = self else { return }
                if let error = error {
                    self.writer.respondError(cmd.id, error.localizedDescription)
                } else {
                    let value = self.serializeJSResult(result)
                    self.writer.respond(cmd.id, ok: true, extra: ["value": value])
                }
            }

        case "start_monitoring":
            guard let script = cmd.script else {
                writer.respondError(cmd.id, "missing script")
                return
            }
            let interval = cmd.interval ?? 1000
            monitoringTimer?.invalidate()
            monitoringTimer = Timer.scheduledTimer(withTimeInterval: Double(interval) / 1000.0, repeats: true) { [weak self] _ in
                guard let self = self else { return }
                self.webView.evaluateJavaScript(script) { result, _ in
                    let value = self.serializeJSResult(result)
                    self.writer.event("monitor", extra: ["result": value])
                }
            }
            writer.respond(cmd.id, ok: true)

        case "stop_monitoring":
            monitoringTimer?.invalidate()
            monitoringTimer = nil
            writer.respond(cmd.id, ok: true)

        case "wait_for_navigation":
            // If already loaded (no pending navigation), check loading state.
            if !webView.isLoading {
                writer.respond(cmd.id, ok: true)
            } else {
                navDelegate.addPendingWait(cmd.id)
                // Timeout after 30s.
                DispatchQueue.main.asyncAfter(deadline: .now() + 30) { [weak self] in
                    guard let self = self else { return }
                    self.navDelegate.lock.lock()
                    if self.navDelegate.pendingWaits.remove(cmd.id) != nil {
                        self.navDelegate.lock.unlock()
                        self.writer.respondError(cmd.id, "navigation timeout")
                    } else {
                        self.navDelegate.lock.unlock()
                    }
                }
            }

        case "url":
            let url = webView.url?.absoluteString ?? "about:blank"
            writer.respond(cmd.id, ok: true, extra: ["value": url])

        case "title":
            let title = webView.title ?? ""
            writer.respond(cmd.id, ok: true, extra: ["value": title])

        case "screenshot":
            let config = WKSnapshotConfiguration()
            webView.takeSnapshot(with: config) { [weak self] image, error in
                guard let self = self else { return }
                if let error = error {
                    self.writer.respondError(cmd.id, error.localizedDescription)
                    return
                }
                guard let image = image,
                      let tiff = image.tiffRepresentation,
                      let bitmap = NSBitmapImageRep(data: tiff),
                      let png = bitmap.representation(using: .png, properties: [:]) else {
                    self.writer.respondError(cmd.id, "screenshot conversion failed")
                    return
                }
                let b64 = png.base64EncodedString()
                self.writer.respond(cmd.id, ok: true, extra: ["png_b64": b64])
            }

        case "cookies":
            webView.configuration.websiteDataStore.httpCookieStore.getAllCookies { [weak self] cookies in
                guard let self = self else { return }
                let mapped: [[String: Any]] = cookies.map { c in
                    var dict: [String: Any] = [
                        "name": c.name,
                        "value": c.value,
                        "domain": c.domain,
                        "path": c.path,
                        "expires": c.expiresDate?.timeIntervalSince1970 ?? 0,
                        "secure": c.isSecure,
                        "httpOnly": c.isHTTPOnly,
                    ]
                    if let sameSite = c.sameSitePolicy {
                        switch sameSite {
                        case .sameSiteLax: dict["sameSite"] = "Lax"
                        case .sameSiteStrict: dict["sameSite"] = "Strict"
                        default: break
                        }
                    }
                    return dict
                }
                self.writer.respond(cmd.id, ok: true, extra: ["cookies": mapped])
            }

        case "set_cookies":
            guard let cookiesData = cmd.cookies else {
                writer.respondError(cmd.id, "missing cookies")
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
                self?.writer.respond(cmd.id, ok: true)
            }

        case "close":
            writer.respond(cmd.id, ok: true)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
                NSApp.terminate(nil)
            }

        default:
            writer.respondError(cmd.id, "unknown command: \(cmd.cmd)")
        }
    }

    // MARK: - JS result serialization

    /// Convert WKWebView's JS result (Any?) to a JSON-safe value.
    private func serializeJSResult(_ result: Any?) -> Any {
        switch result {
        case nil:
            return NSNull()
        case is NSNull:
            return NSNull()
        case let date as Date:
            // JS Date() → Unix timestamp (seconds)
            return date.timeIntervalSince1970
        case let data as Data:
            self.writer.event("console", extra: ["level": "warn", "message": "Ignored NSData in JS result (\(data.count) bytes)"])
            return NSNull()
        case let str as String:
            return str
        case let num as NSNumber:
            // Distinguish booleans from numbers (NSNumber wraps both).
            if CFBooleanGetTypeID() == CFGetTypeID(num) {
                return num.boolValue
            }
            return num
        case let arr as [Any]:
            return arr.map { serializeJSResult($0) }
        case let dict as [String: Any]:
            return dict.mapValues { serializeJSResult($0) }
        default:
            // Unknown type → null (safe fallback, no crash)
            return NSNull()
        }
    }
}

// MARK: - Entry point

let app = NSApplication.shared
let delegate = BridgeApp()
app.delegate = delegate

// Headless: don't activate as foreground app unless visible.
if ProcessInfo.processInfo.environment["LAD_WEBKIT_VISIBLE"] != "1" {
    app.setActivationPolicy(.accessory)
}

// MARK: - Network Monitor Protocol

final class NetworkMonitorProtocol: URLProtocol {
    private var dataTask: URLSessionDataTask?

    override class func canInit(with request: URLRequest) -> Bool {
        guard let scheme = request.url?.scheme, (scheme == "http" || scheme == "https") else { return false }
        if URLProtocol.property(forKey: "NetworkMonitorHandled", in: request) != nil {
            return false
        }
        return true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        return request
    }

    override func startLoading() {
        guard let req = (request as NSURLRequest).mutableCopy() as? NSMutableURLRequest else { return }
        URLProtocol.setProperty(true, forKey: "NetworkMonitorHandled", in: req)

        let method = request.httpMethod ?? "GET"
        let urlStr = request.url?.absoluteString ?? ""

        // Classify request
        var type = "Asset"
        if urlStr.contains("api") || urlStr.contains("graphql") || urlStr.contains(".json") {
            type = "API"
        } else if urlStr.contains("login") || urlStr.contains("oauth") || urlStr.contains("auth") {
            type = "Auth"
        } else if method != "GET" {
            type = "Action"
        } else if !urlStr.contains(".js") && !urlStr.contains(".css") && !urlStr.contains(".png") && !urlStr.contains(".svg") {
            type = "Navigation"
        }

        DispatchQueue.main.async {
            if let delegate = NSApp.delegate as? BridgeApp {
                delegate.writer.event("network", extra: [
                    "type": type,
                    "method": method,
                    "url": urlStr
                ])
            }
        }

        let session = URLSession(configuration: .default, delegate: self, delegateQueue: nil)
        dataTask = session.dataTask(with: req as URLRequest)
        dataTask?.resume()
    }

    override func stopLoading() {
        dataTask?.cancel()
        dataTask = nil
    }
}

extension NetworkMonitorProtocol: URLSessionDataDelegate {
    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse, completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        client?.urlProtocol(self, didLoad: data)
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        if let error = error {
            client?.urlProtocol(self, didFailWithError: error)
        } else {
            client?.urlProtocolDidFinishLoading(self)
        }
    }
}

app.run()
