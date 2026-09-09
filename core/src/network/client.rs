use anyhow::{Context, Result};
use reqwest::{Client, Response};
use tracing::debug;

const DEFAULT_UA: &str = "curl/8.7.1";

/// Information gathered from an HTTP HEAD request
pub struct ServerInfo {
    pub content_length: u64,
    pub supports_ranges: bool,
    pub content_type: Option<String>,
    pub filename_hint: Option<String>,
    pub supports_http3: bool,
}

/// HTTP client wrapper — handles both HTTP/1.1 & HTTP/2 today, with HTTP/3 detection.
/// Quinn-based QUIC is used when the server advertises `alt-svc: h3` (via HTTP/3 upgrade).
pub struct HttpClient {
    pub inner: Client,
}

impl HttpClient {
    pub fn new() -> Self {
        let inner = Client::builder()
            .user_agent(DEFAULT_UA)
            .use_rustls_tls()
            .timeout(std::time::Duration::from_secs(30))
            .connection_verbose(false)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .build()
            .expect("Failed to build HTTP client");

        Self { inner }
    }

    pub async fn probe(
        &self,
        url: &str,
        user_agent: Option<&str>,
        cookies: Option<&str>,
    ) -> Result<ServerInfo> {
        let ua = user_agent.unwrap_or(DEFAULT_UA);

        let mut req = self.inner.head(url).header("User-Agent", ua);

        if let Some(c) = cookies {
            req = req.header("Cookie", c);
        }

        let resp = req.send().await.map_err(|e| {
            tracing::error!("Reqwest GET (probe) error: {:?}", e);
            anyhow::anyhow!("Probe request failed: {}", e)
        })?;

        let status = resp.status();
        if !status.is_success() && status.as_u16() != 206 {
            anyhow::bail!("Server returned HTTP {}", status);
        }

        let headers = resp.headers();

        // If we get 206, we can parse Content-Range for total length
        let mut content_length = 0;
        let mut supports_ranges = status.as_u16() == 206;

        if supports_ranges {
            if let Some(cr) = headers.get("content-range").and_then(|v| v.to_str().ok()) {
                if let Some(total_str) = cr.split('/').nth(1) {
                    if let Ok(t) = total_str.parse::<u64>() {
                        content_length = t;
                    }
                }
            }
        } 
        
        // Fallback to content-length header if Content-Range didn't give it or not 206
        if content_length == 0 {
            content_length = headers
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
        }

        // If it wasn't a 206, we can also check Accept-Ranges header
        if !supports_ranges {
            supports_ranges = headers
                .get("accept-ranges")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.contains("bytes"))
                .unwrap_or(false);
        }

        let content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Check for HTTP/3 upgrade advertisement (alt-svc: h3="...")
        let supports_http3 = headers
            .get("alt-svc")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("h3"))
            .unwrap_or(false);

        // Try to extract filename from Content-Disposition header
        let filename_hint = headers
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.split(';')
                    .find(|s| s.trim().starts_with("filename"))
                    .and_then(|s| s.split('=').nth(1))
                    .map(|s| s.trim().trim_matches('"').to_string())
            });

        debug!(
            "Probe: length={} ranges={} http3={} type={:?}",
            content_length, supports_ranges, supports_http3, content_type
        );

        Ok(ServerInfo {
            content_length,
            supports_ranges,
            content_type,
            filename_hint,
            supports_http3,
        })
    }

    /// Send an HTTP GET with a Range header, returning a streaming response
    pub async fn get_range(
        &self,
        url: &str,
        range: &str,
        user_agent: Option<&str>,
        cookies: Option<&str>,
    ) -> Result<Response> {
        let ua = user_agent.unwrap_or(DEFAULT_UA);

        let mut req = self
            .inner
            .get(url)
            .header("User-Agent", ua)
            .header("Range", range);

        if let Some(c) = cookies {
            req = req.header("Cookie", c);
        }

        let resp = req.send().await.context("GET range request failed")?;

        if !resp.status().is_success() && resp.status().as_u16() != 206 {
            anyhow::bail!("Range request returned HTTP {}", resp.status());
        }

        Ok(resp)
    }
}
