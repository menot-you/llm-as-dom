//! Wire-protocol types for the WebKit bridge NDJSON protocol.
//!
//! These types are serialized/deserialized as single JSON lines on
//! stdin (requests) and stdout (responses/events) of the
//! `lad-webkit-bridge` sidecar process.

use serde::{Deserialize, Serialize};

/// Request sent from Rust to the Swift bridge via stdin.
#[derive(Serialize)]
pub(super) struct Request {
    pub id: u64,
    pub cmd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookies: Option<Vec<CookieWire>>,
}

impl Request {
    /// Create a bare command request (id is set later by `BridgeConnection`).
    pub fn cmd(cmd: &str) -> Self {
        Self {
            id: 0,
            cmd: cmd.into(),
            url: None,
            script: None,
            cookies: None,
        }
    }
}

/// Cookie representation on the wire (matches Swift JSON encoding).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct CookieWire {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    #[serde(default)]
    pub expires: f64,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, rename = "httpOnly")]
    pub http_only: bool,
    #[serde(default, rename = "sameSite")]
    pub same_site: Option<String>,
}

/// Response or event received from the Swift bridge via stdout.
#[derive(Deserialize)]
pub(super) struct Response {
    /// Present on correlated responses, absent on push events.
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub png_b64: Option<String>,
    #[serde(default)]
    pub cookies: Option<Vec<CookieWire>>,
    #[serde(default)]
    pub error: Option<String>,
    /// Event type for push messages (e.g. "ready", "console", "load").
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

// ── Cookie conversion helpers ────────────────────────────────────

pub(super) fn cookie_from_wire(w: CookieWire) -> crate::session::CookieEntry {
    crate::session::CookieEntry {
        name: w.name,
        value: w.value,
        domain: w.domain,
        path: w.path,
        expires: w.expires,
        secure: w.secure,
        http_only: w.http_only,
        same_site: w.same_site,
    }
}

pub(super) fn cookie_to_wire(c: &crate::session::CookieEntry) -> CookieWire {
    CookieWire {
        name: c.name.clone(),
        value: c.value.clone(),
        domain: c.domain.clone(),
        path: c.path.clone(),
        expires: c.expires,
        secure: c.secure,
        http_only: c.http_only,
        same_site: c.same_site.clone(),
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_wire_roundtrip() {
        let wire = CookieWire {
            name: "sid".into(),
            value: "abc123".into(),
            domain: ".example.com".into(),
            path: "/".into(),
            expires: 1700000000.0,
            secure: true,
            http_only: true,
            same_site: Some("Lax".into()),
        };

        let json = serde_json::to_string(&wire).unwrap();
        let back: CookieWire = serde_json::from_str(&json).unwrap();
        assert_eq!(wire, back);
    }

    #[test]
    fn cookie_wire_defaults() {
        let json = r#"{"name":"x","value":"y","domain":".d","path":"/"}"#;
        let c: CookieWire = serde_json::from_str(json).unwrap();
        assert_eq!(c.expires, 0.0);
        assert!(!c.secure);
        assert!(!c.http_only);
        assert!(c.same_site.is_none());
    }

    #[test]
    fn request_serialize_navigate() {
        let req = Request {
            id: 42,
            cmd: "navigate".into(),
            url: Some("https://example.com".into()),
            script: None,
            cookies: None,
        };
        let json: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(json["id"], 42);
        assert_eq!(json["cmd"], "navigate");
        assert_eq!(json["url"], "https://example.com");
        assert!(json.get("script").is_none());
        assert!(json.get("cookies").is_none());
    }

    #[test]
    fn request_serialize_eval_js() {
        let req = Request {
            id: 7,
            cmd: "eval_js".into(),
            url: None,
            script: Some("document.title".into()),
            cookies: None,
        };
        let json: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(json["cmd"], "eval_js");
        assert_eq!(json["script"], "document.title");
        assert!(json.get("url").is_none());
    }

    #[test]
    fn response_deserialize_ok() {
        let json = r#"{"id":1,"ok":true,"value":"Example Domain"}"#;
        let resp: Response = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert_eq!(resp.ok, Some(true));
        assert_eq!(resp.value.unwrap().as_str().unwrap(), "Example Domain");
    }

    #[test]
    fn response_deserialize_error() {
        let json = r#"{"id":2,"ok":false,"error":"JS eval failed"}"#;
        let resp: Response = serde_json::from_str(json).unwrap();
        assert_eq!(resp.ok, Some(false));
        assert_eq!(resp.error.as_deref(), Some("JS eval failed"));
    }

    #[test]
    fn response_deserialize_event() {
        let json = r#"{"event":"console","level":"error","message":"TypeError"}"#;
        let resp: Response = serde_json::from_str(json).unwrap();
        assert!(resp.id.is_none());
        assert_eq!(resp.event.as_deref(), Some("console"));
        assert_eq!(resp.level.as_deref(), Some("error"));
        assert_eq!(resp.message.as_deref(), Some("TypeError"));
    }

    #[test]
    fn response_deserialize_screenshot() {
        let json = r#"{"id":3,"ok":true,"png_b64":"iVBOR"}"#;
        let resp: Response = serde_json::from_str(json).unwrap();
        assert_eq!(resp.png_b64.as_deref(), Some("iVBOR"));
    }

    #[test]
    fn response_deserialize_cookies() {
        let json = r#"{"id":6,"ok":true,"cookies":[{"name":"sid","value":"abc","domain":".example.com","path":"/"}]}"#;
        let resp: Response = serde_json::from_str(json).unwrap();
        let cookies = resp.cookies.unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "sid");
    }

    #[test]
    fn cookie_conversion_roundtrip() {
        let entry = crate::session::CookieEntry {
            name: "token".into(),
            value: "xyz".into(),
            domain: ".test.com".into(),
            path: "/app".into(),
            expires: 99999.0,
            secure: true,
            http_only: false,
            same_site: Some("Strict".into()),
        };
        let wire = cookie_to_wire(&entry);
        let back = cookie_from_wire(wire);
        assert_eq!(back.name, entry.name);
        assert_eq!(back.value, entry.value);
        assert_eq!(back.domain, entry.domain);
        assert_eq!(back.secure, entry.secure);
        assert_eq!(back.same_site, entry.same_site);
    }
}
