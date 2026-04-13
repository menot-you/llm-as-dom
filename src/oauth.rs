//! OAuth flow detection, provider identification, and flow state tracking.

use serde::{Deserialize, Serialize};

/// Known OAuth providers with URL patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OAuthProvider {
    Google,
    GitHub,
    Facebook,
    Microsoft,
    Apple,
    Twitter,
    /// Any provider not in the known list.
    Generic,
}

/// OAuth flow state machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OAuthFlowState {
    /// No OAuth flow detected.
    #[default]
    Idle,
    /// Redirect to OAuth provider detected. Contains the origin URL.
    Redirecting { origin_url: String },
    /// On the provider's login page.
    AtProvider {
        provider: OAuthProvider,
        origin_url: String,
    },
    /// On the consent/scope approval page.
    Consent {
        provider: OAuthProvider,
        origin_url: String,
    },
    /// Redirected back to the origin with auth code/token.
    Callback {
        provider: OAuthProvider,
        origin_url: String,
    },
    /// OAuth flow completed successfully.
    Completed { provider: OAuthProvider },
    /// OAuth flow failed.
    Failed { reason: String },
}

/// OAuth provider URL patterns for detection.
const OAUTH_PATTERNS: &[(&str, OAuthProvider)] = &[
    ("accounts.google.com", OAuthProvider::Google),
    ("accounts.youtube.com", OAuthProvider::Google),
    ("github.com/login/oauth", OAuthProvider::GitHub),
    ("github.com/login", OAuthProvider::GitHub),
    ("facebook.com/v", OAuthProvider::Facebook),
    ("facebook.com/dialog/oauth", OAuthProvider::Facebook),
    ("login.microsoftonline.com", OAuthProvider::Microsoft),
    ("login.live.com", OAuthProvider::Microsoft),
    ("appleid.apple.com", OAuthProvider::Apple),
    ("api.twitter.com/oauth", OAuthProvider::Twitter),
    ("twitter.com/i/oauth2", OAuthProvider::Twitter),
];

/// Callback URL parameter patterns that indicate OAuth completion.
const CALLBACK_PARAMS: &[&str] = &[
    "code",
    "access_token",
    "token",
    "id_token",
    "state",
    "oauth_token",
    "oauth_verifier",
];

/// Consent page keywords in visible text.
pub const CONSENT_KEYWORDS: &[&str] = &[
    "authorize",
    "allow access",
    "grant access",
    "permission",
    "consent",
    "approve",
    "continue as",
    "wants to access",
    "is requesting access",
    "sign in to continue",
];

/// Detect if a URL belongs to a known OAuth provider.
pub fn detect_provider(url: &str) -> Option<OAuthProvider> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    let path = parsed.path().to_lowercase();

    OAUTH_PATTERNS
        .iter()
        .find(|(pattern, _)| {
            let mut parts = pattern.splitn(2, '/');
            let pat_host = parts.next().unwrap();
            let pat_path = parts.next().unwrap_or("");

            let host_match = host == pat_host || host.ends_with(&format!(".{}", pat_host));
            let path_match = if pat_path.is_empty() {
                true
            } else {
                path.strip_prefix('/').unwrap_or(&path).starts_with(pat_path)
            };

            host_match && path_match
        })
        .map(|(_, provider)| *provider)
}

/// Check if a URL contains OAuth callback parameters.
pub fn is_callback_url(url: &str) -> bool {
    let parsed = match url::Url::parse(url) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let has_query_param = parsed
        .query_pairs()
        .any(|(k, _)| CALLBACK_PARAMS.contains(&k.as_ref()));

    let has_fragment_param = if let Some(fragment) = parsed.fragment() {
        CALLBACK_PARAMS.iter().any(|param| {
            fragment.contains(&format!("{}=", param))
        })
    } else {
        false
    };

    has_query_param || has_fragment_param
}

/// Check if visible text suggests a consent/authorization page.
pub fn is_consent_page(visible_text: &str) -> bool {
    let lower = visible_text.to_lowercase();
    CONSENT_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

/// Determine the next OAuth flow state based on current state and
/// page observation.
pub fn advance_flow(
    current: &OAuthFlowState,
    url: &str,
    visible_text: &str,
    origin_url: Option<&str>,
) -> OAuthFlowState {
    match current {
        OAuthFlowState::Idle => {
            if let Some(provider) = detect_provider(url) {
                OAuthFlowState::AtProvider {
                    provider,
                    origin_url: origin_url.unwrap_or("").to_string(),
                }
            } else {
                OAuthFlowState::Idle
            }
        }
        OAuthFlowState::Redirecting { origin_url } => {
            if let Some(provider) = detect_provider(url) {
                OAuthFlowState::AtProvider {
                    provider,
                    origin_url: origin_url.clone(),
                }
            } else {
                OAuthFlowState::Idle
            }
        }
        OAuthFlowState::AtProvider {
            provider,
            origin_url,
        } => {
            if is_consent_page(visible_text) {
                OAuthFlowState::Consent {
                    provider: *provider,
                    origin_url: origin_url.clone(),
                }
            } else if is_callback_url(url) {
                OAuthFlowState::Callback {
                    provider: *provider,
                    origin_url: origin_url.clone(),
                }
            } else if detect_provider(url).is_none() {
                // Left the provider — check if it's a callback
                if is_callback_url(url) || url.contains(origin_url.as_str()) {
                    OAuthFlowState::Callback {
                        provider: *provider,
                        origin_url: origin_url.clone(),
                    }
                } else {
                    OAuthFlowState::Failed {
                        reason: "navigated away from provider without callback".into(),
                    }
                }
            } else {
                current.clone()
            }
        }
        OAuthFlowState::Consent {
            provider,
            origin_url,
        } => {
            if detect_provider(url).is_none() || is_callback_url(url) {
                OAuthFlowState::Callback {
                    provider: *provider,
                    origin_url: origin_url.clone(),
                }
            } else {
                current.clone()
            }
        }
        OAuthFlowState::Callback { provider, .. } => OAuthFlowState::Completed {
            provider: *provider,
        },
        _ => current.clone(),
    }
}

/// Get a human-readable description of the OAuth flow for LLM context.
pub fn flow_context(state: &OAuthFlowState) -> Option<String> {
    match state {
        OAuthFlowState::Idle => None,
        OAuthFlowState::Redirecting { origin_url } => Some(format!(
            "OAuth: redirecting from {origin_url} to auth provider"
        )),
        OAuthFlowState::AtProvider {
            provider,
            origin_url,
        } => Some(format!(
            "OAuth: at {provider:?} login page (started from {origin_url}). \
             Fill credentials to continue.",
        )),
        OAuthFlowState::Consent { provider, .. } => Some(format!(
            "OAuth: {provider:?} asking for permission. \
             Click 'Allow' or 'Authorize' to proceed.",
        )),
        OAuthFlowState::Callback { provider, .. } => Some(format!(
            "OAuth: {provider:?} redirected back. Auth flow completing.",
        )),
        OAuthFlowState::Completed { provider } => {
            Some(format!("OAuth: {provider:?} flow completed successfully.",))
        }
        OAuthFlowState::Failed { reason } => Some(format!("OAuth: flow failed — {reason}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Provider detection ---

    #[test]
    fn detect_google_oauth() {
        assert_eq!(
            detect_provider("https://accounts.google.com/o/oauth2/v2/auth"),
            Some(OAuthProvider::Google),
        );
    }

    #[test]
    fn detect_github_oauth() {
        assert_eq!(
            detect_provider("https://github.com/login/oauth/authorize"),
            Some(OAuthProvider::GitHub),
        );
    }

    #[test]
    fn detect_facebook_oauth() {
        assert_eq!(
            detect_provider("https://facebook.com/dialog/oauth"),
            Some(OAuthProvider::Facebook),
        );
    }

    #[test]
    fn detect_microsoft_oauth() {
        assert_eq!(
            detect_provider("https://login.microsoftonline.com/common/oauth2"),
            Some(OAuthProvider::Microsoft),
        );
    }

    #[test]
    fn detect_no_provider() {
        assert_eq!(detect_provider("https://example.com/login"), None);
    }

    // --- Callback detection ---

    #[test]
    fn callback_with_code() {
        assert!(is_callback_url(
            "https://example.com/callback?code=abc123&state=xyz"
        ));
    }

    #[test]
    fn callback_with_token() {
        assert!(is_callback_url("https://example.com/auth#access_token=xyz"));
    }

    #[test]
    fn no_callback_params() {
        assert!(!is_callback_url("https://example.com/dashboard"));
    }

    // --- Consent page detection ---

    #[test]
    fn consent_page_detected() {
        assert!(is_consent_page(
            "MyApp wants to access your account. Click Allow to continue."
        ));
    }

    #[test]
    fn not_consent_page() {
        assert!(!is_consent_page("Enter your email and password"));
    }

    // --- Flow state machine ---

    #[test]
    fn flow_idle_to_provider() {
        let state = advance_flow(
            &OAuthFlowState::Idle,
            "https://accounts.google.com/o/oauth2/auth",
            "",
            Some("https://myapp.com"),
        );
        assert!(matches!(
            state,
            OAuthFlowState::AtProvider {
                provider: OAuthProvider::Google,
                ..
            }
        ));
    }

    #[test]
    fn flow_provider_to_consent() {
        let state = advance_flow(
            &OAuthFlowState::AtProvider {
                provider: OAuthProvider::Google,
                origin_url: "https://myapp.com".into(),
            },
            "https://accounts.google.com/o/oauth2/auth",
            "MyApp wants to access your account",
            None,
        );
        assert!(matches!(state, OAuthFlowState::Consent { .. }));
    }

    #[test]
    fn flow_consent_to_callback() {
        let state = advance_flow(
            &OAuthFlowState::Consent {
                provider: OAuthProvider::Google,
                origin_url: "https://myapp.com".into(),
            },
            "https://myapp.com/callback?code=abc",
            "",
            None,
        );
        assert!(matches!(state, OAuthFlowState::Callback { .. }));
    }

    #[test]
    fn flow_callback_to_completed() {
        let state = advance_flow(
            &OAuthFlowState::Callback {
                provider: OAuthProvider::Google,
                origin_url: "https://myapp.com".into(),
            },
            "https://myapp.com/dashboard",
            "",
            None,
        );
        assert!(matches!(state, OAuthFlowState::Completed { .. }));
    }

    #[test]
    fn flow_context_gives_instructions() {
        let state = OAuthFlowState::AtProvider {
            provider: OAuthProvider::Google,
            origin_url: "https://myapp.com".into(),
        };
        let ctx = flow_context(&state).unwrap();
        assert!(ctx.contains("Google"));
        assert!(ctx.contains("credentials"));
    }

    #[test]
    fn flow_context_idle_is_none() {
        assert!(flow_context(&OAuthFlowState::Idle).is_none());
    }
}
