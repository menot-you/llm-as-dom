//! Chromium browser engine adapter.
//!
//! Wraps `chromiumoxide::Browser` and `chromiumoxide::Page` behind the
//! `BrowserEngine` / `PageHandle` traits.

use async_trait::async_trait;
use std::sync::Arc;

use super::{BrowserEngine, EngineConfig, PageHandle};

/// Chromium-backed browser engine.
pub struct ChromiumEngine {
    browser: Arc<chromiumoxide::Browser>,
    _handler: tokio::task::JoinHandle<()>,
}

impl ChromiumEngine {
    /// Launch a Chromium browser with the given configuration.
    pub async fn launch(config: EngineConfig) -> Result<Self, crate::Error> {
        let mut builder = chromiumoxide::BrowserConfig::builder();

        if config.interactive {
            builder = builder
                .arg("--app=about:blank")
                .arg("--disable-extensions")
                .arg("--disable-default-apps")
                .arg("--disable-component-extensions-with-background-pages")
                .arg("--disable-translate")
                .arg("--no-first-run")
                .arg("--no-default-browser-check")
                .arg(format!(
                    "--window-size={},{}",
                    config.window_size.0, config.window_size.1
                ));
        } else if !config.visible {
            builder = builder.arg("--headless=new").arg(format!(
                "--window-size={},{}",
                config.window_size.0, config.window_size.1
            ));
        } else {
            builder = builder.arg(format!(
                "--window-size={},{}",
                config.window_size.0, config.window_size.1
            ));
        }

        builder = builder
            .arg("--disable-gpu")
            .arg("--disable-dev-shm-usage")
            .arg(format!(
                "--user-data-dir={}",
                config.user_data_dir.display()
            ));

        if std::env::var("LAD_CHROME_NO_SANDBOX").unwrap_or_default() == "true" {
            builder = builder.arg("--no-sandbox");
        }

        let browser_config = builder.build().map_err(crate::Error::Browser)?;

        let (browser, mut handler) = chromiumoxide::Browser::launch(browser_config)
            .await
            .map_err(|e| crate::Error::Browser(format!("{e}")))?;

        let handle = tokio::spawn(async move {
            use futures::StreamExt;
            while handler.next().await.is_some() {}
        });

        Ok(Self {
            browser: Arc::new(browser),
            _handler: handle,
        })
    }
}

#[async_trait]
impl BrowserEngine for ChromiumEngine {
    async fn new_page(&self, url: &str) -> Result<Box<dyn PageHandle>, crate::Error> {
        let page = self.browser.new_page(url).await.map_err(cdp_err)?;
        Ok(Box::new(ChromiumPage { page }))
    }

    fn name(&self) -> &str {
        "chromium"
    }

    async fn close(&self) -> Result<(), crate::Error> {
        // Dropping the browser triggers graceful shutdown.
        // The handler task will end when the event stream closes.
        Ok(())
    }
}

/// Chromium-backed page handle.
struct ChromiumPage {
    page: chromiumoxide::Page,
}

#[async_trait]
impl PageHandle for ChromiumPage {
    async fn eval_js(&self, script: &str) -> Result<serde_json::Value, crate::Error> {
        match self.page.evaluate(script).await {
            Ok(eval_result) => {
                // Try to extract a Value; void expressions fail here.
                match eval_result.into_value::<serde_json::Value>() {
                    Ok(v) => Ok(v),
                    Err(_) => Ok(serde_json::Value::Null),
                }
            }
            Err(e) => Err(cdp_err(e)),
        }
    }

    async fn navigate(&self, url: &str) -> Result<(), crate::Error> {
        self.page.goto(url).await.map_err(cdp_err)?;
        Ok(())
    }

    async fn wait_for_navigation(&self) -> Result<(), crate::Error> {
        self.page.wait_for_navigation().await.map_err(cdp_err)?;
        Ok(())
    }

    async fn url(&self) -> Result<String, crate::Error> {
        Ok(self
            .page
            .url()
            .await
            .map_err(cdp_err)?
            .unwrap_or_else(|| "unknown".into()))
    }

    async fn title(&self) -> Result<String, crate::Error> {
        Ok(self
            .page
            .get_title()
            .await
            .map_err(cdp_err)?
            .unwrap_or_default())
    }

    async fn screenshot_png(&self) -> Result<Vec<u8>, crate::Error> {
        let params = chromiumoxide::page::ScreenshotParams::builder()
            .full_page(true)
            .build();
        self.page.screenshot(params).await.map_err(cdp_err)
    }

    async fn cookies(&self) -> Result<Vec<crate::session::CookieEntry>, crate::Error> {
        let js = r#"
            (() => {
                const url = window.location.href;
                const hostname = window.location.hostname;
                const pathname = window.location.pathname;
                return JSON.stringify({
                    url: url,
                    hostname: hostname,
                    pathname: pathname,
                    cookies: document.cookie.split(';').map(c => {
                        const [name, ...rest] = c.trim().split('=');
                        return { name: name || '', value: rest.join('=') || '' };
                    }).filter(c => c.name.length > 0)
                });
            })()
        "#;

        let result: String = self
            .page
            .evaluate(js)
            .await
            .map_err(cdp_err)?
            .into_value()
            .map_err(|e| crate::Error::ActionFailed(e.to_string()))?;

        let parsed: serde_json::Value =
            serde_json::from_str(&result).map_err(|e| crate::Error::ActionFailed(e.to_string()))?;

        let hostname = parsed["hostname"].as_str().unwrap_or_default();
        let pathname = parsed["pathname"].as_str().unwrap_or("/");

        let cookies: Vec<crate::session::CookieEntry> = parsed["cookies"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        let name = c["name"].as_str()?.to_string();
                        let value = c["value"].as_str().unwrap_or_default().to_string();
                        Some(crate::session::CookieEntry {
                            name,
                            value,
                            domain: hostname.to_string(),
                            path: pathname.to_string(),
                            expires: 0.0,
                            secure: false,
                            http_only: false,
                            same_site: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        tracing::debug!(count = cookies.len(), "extracted cookies via JS");
        Ok(cookies)
    }

    async fn set_cookies(
        &self,
        cookies: &[crate::session::CookieEntry],
    ) -> Result<(), crate::Error> {
        for cookie in cookies {
            let mut parts = vec![format!(
                "{}={}",
                crate::pilot::js_escape(&cookie.name),
                crate::pilot::js_escape(&cookie.value)
            )];

            if !cookie.domain.is_empty() {
                parts.push(format!("domain={}", cookie.domain));
            }
            if !cookie.path.is_empty() {
                parts.push(format!("path={}", cookie.path));
            }
            if cookie.expires > 0.0 {
                parts.push(format!("expires={}", cookie.expires));
            }
            if cookie.secure {
                parts.push("secure".to_string());
            }
            if let Some(ref ss) = cookie.same_site {
                parts.push(format!("samesite={ss}"));
            }

            let cookie_str = parts.join("; ");
            let js = format!(
                "document.cookie = '{}'",
                crate::pilot::js_escape(&cookie_str)
            );
            let _ = self.page.evaluate(js).await.map_err(cdp_err)?;
        }

        tracing::debug!(count = cookies.len(), "injected cookies via JS");
        Ok(())
    }

    async fn enable_network_monitoring(&self) -> Result<bool, crate::Error> {
        use chromiumoxide::cdp::browser_protocol::network::EnableParams;
        self.page
            .execute(EnableParams::default())
            .await
            .map_err(|e| {
                crate::Error::ActionFailed(format!("failed to enable network tracking: {e}"))
            })?;
        tracing::debug!("network tracking enabled");
        Ok(true)
    }
}

/// Convert a CDP error to our unified error type.
fn cdp_err(e: chromiumoxide::error::CdpError) -> crate::Error {
    crate::Error::Browser(e.to_string())
}
