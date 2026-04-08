// NDJSON protocol types — matches the webkit-bridge protocol exactly.
//
// Commands flow: LAD → lad-relay → WebSocket → iPhone → BridgeCommand
// Responses flow: iPhone → BridgeResponse → WebSocket → lad-relay → LAD

import Foundation

/// A command received from LAD via the relay.
public struct BridgeCommand: Codable, Sendable {
    public let id: UInt64
    public let cmd: String
    public var url: String?
    public var script: String?
    public var cookies: [CookieData]?
    public var visible: Bool?
    public var width: Int?
    public var height: Int?
    public var interval: Int?
}

/// A response sent back to LAD via the relay.
public struct BridgeResponse: Codable, Sendable {
    public var id: UInt64?
    public var ok: Bool?
    public var error: String?
    public var value: AnyCodable?
    public var event: String?
    public var png_b64: String?
    public var cookies: [CookieData]?

    public init(id: UInt64, ok: Bool, extra: [String: AnyCodable] = [:]) {
        self.id = id
        self.ok = ok
        for (key, val) in extra {
            switch key {
            case "value": self.value = val
            case "png_b64": if case .string(let s) = val { self.png_b64 = s }
            default: break
            }
        }
    }

    public static func ok(_ id: UInt64) -> BridgeResponse {
        BridgeResponse(id: id, ok: true)
    }

    public static func error(_ id: UInt64, _ message: String) -> BridgeResponse {
        var r = BridgeResponse(id: id, ok: false)
        r.error = message
        return r
    }

    public static func event(_ type: String, extra: [String: AnyCodable] = [:]) -> BridgeResponse {
        var r = BridgeResponse(id: 0, ok: true)
        r.id = nil
        r.ok = nil
        r.event = type
        return r
    }
}

/// Cookie data matching the webkit-bridge protocol.
public struct CookieData: Codable, Sendable {
    public let name: String
    public let value: String
    public let domain: String
    public let path: String
    public var expires: Double?
    public var secure: Bool?
    public var httpOnly: Bool?
    public var sameSite: String?

    public init(
        name: String, value: String, domain: String, path: String,
        expires: Double? = nil, secure: Bool? = nil,
        httpOnly: Bool? = nil, sameSite: String? = nil
    ) {
        self.name = name
        self.value = value
        self.domain = domain
        self.path = path
        self.expires = expires
        self.secure = secure
        self.httpOnly = httpOnly
        self.sameSite = sameSite
    }
}

/// Type-erased Codable value for flexible JSON serialization.
public enum AnyCodable: Codable, Sendable {
    case string(String)
    case int(Int)
    case double(Double)
    case bool(Bool)
    case null
    case array([AnyCodable])
    case dictionary([String: AnyCodable])

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let v = try? container.decode(Bool.self) { self = .bool(v) }
        else if let v = try? container.decode(Int.self) { self = .int(v) }
        else if let v = try? container.decode(Double.self) { self = .double(v) }
        else if let v = try? container.decode(String.self) { self = .string(v) }
        else if let v = try? container.decode([AnyCodable].self) { self = .array(v) }
        else if let v = try? container.decode([String: AnyCodable].self) { self = .dictionary(v) }
        else if container.decodeNil() { self = .null }
        else { self = .null }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .string(let v): try container.encode(v)
        case .int(let v): try container.encode(v)
        case .double(let v): try container.encode(v)
        case .bool(let v): try container.encode(v)
        case .null: try container.encodeNil()
        case .array(let v): try container.encode(v)
        case .dictionary(let v): try container.encode(v)
        }
    }
}
