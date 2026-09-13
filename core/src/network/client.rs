use anyhow::{Context, Result};
use reqwest::{Client, Response, StatusCode};
use tracing::{debug, warn};
use url::Url;

const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Match `engine::manager::MAX_SEGMENT_COUNT` with headroom for steal churn / reconnects.
const POOL_MAX_IDLE_PER_HOST: usize = 64;
const MAX_REDIRECTS: usize = 10;

/// Information gathered from an HTTP HEAD / Range probe
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub content_length: u64,
    pub supports_ranges: bool,
    pub content_type: Option<String>,
    pub filename_hint: Option<String>,
    pub supports_http3: bool,
    /// Final URL after manual redirect following (Range-safe).
    pub final_url: Option<String>,
}

/// Whether a download error should be retried by segment workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Retryable,
    Fatal,
}

/// HTTP client wrapper. Parallel segments use HTTP/1.1 so each range gets its
/// own TCP connection. Redirects are followed manually so `Range` is preserved.
pub struct HttpClient {
    pub inner: Client,
}

impl HttpClient {
    pub fn new() -> Self {
        // HTTP/1.1 on purpose: segmented downloads need one TCP connection per
        // range. HTTP/2 multiplexes every stream onto a single connection, so
        // 16–32 workers starve on flow-control and one stall fails the job.
        //
        // Auto-redirect is disabled: CDNs often drop Range on 302→GET; we re-apply
        // Range on each hop and re-validate against the final URL.
        let inner = Client::builder()
            .user_agent(DEFAULT_UA)
            .use_rustls_tls()
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
            .tcp_nodelay(true)
            .connect_timeout(std::time::Duration::from_secs(15))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .build()
            .expect("Failed to build HTTP client");

        Self { inner }
    }

    fn apply_browser_headers(
        mut req: reqwest::RequestBuilder,
        user_agent: Option<&str>,
        cookies: Option<&str>,
        referer: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let ua = user_agent.unwrap_or(DEFAULT_UA);
        req = req
            .header("User-Agent", ua)
            .header("Accept", "*/*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Connection", "keep-alive");

        if let Some(c) = cookies {
            if !c.is_empty() {
                req = req.header("Cookie", c);
            }
        }
        if let Some(r) = referer {
            if !r.is_empty() {
                req = req.header("Referer", r);
                if let Ok(u) = Url::parse(r) {
                    if let Some(host) = u.host_str() {
                        let origin = format!("{}://{}", u.scheme(), host);
                        req = req.header("Origin", &origin);
                    }
                }
            }
        }
        req
    }

    /// Issue a GET/HEAD, following redirects manually while preserving Range.
    async fn send_with_redirects(
        &self,
        method: reqwest::Method,
        url: &str,
        range: Option<&str>,
        user_agent: Option<&str>,
        cookies: Option<&str>,
        referer: Option<&str>,
    ) -> Result<(Response, String)> {
        let mut current = url.to_string();
        let mut hops = 0usize;

        loop {
            let mut req = Self::apply_browser_headers(
                self.inner.request(method.clone(), &current),
                user_agent,
                cookies,
                referer,
            );
            if let Some(r) = range {
                if !r.is_empty() {
                    req = req.header("Range", r);
                }
            }

            let resp = req.send().await.map_err(|e| {
                let class = classify_reqwest_error(&e);
                anyhow::anyhow!("{e} [{class:?}]")
            })?;

            let status = resp.status();
            if status.is_redirection() {
                let loc = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                drop(resp);

                let Some(loc) = loc else {
                    anyhow::bail!("Redirect without Location (HTTP {status}) [{:?}]", ErrorClass::Fatal);
                };
                hops += 1;
                if hops > MAX_REDIRECTS {
                    anyhow::bail!(
                        "Too many redirects (>{MAX_REDIRECTS}) [{:?}]",
                        ErrorClass::Fatal
                    );
                }
                current = resolve_redirect(&current, &loc)?;
                debug!("Redirect hop {hops} → {}", url_host(&current));
                continue;
            }

            return Ok((resp, current));
        }
    }

    pub async fn probe(
        &self,
        url: &str,
        user_agent: Option<&str>,
        cookies: Option<&str>,
        referer: Option<&str>,
    ) -> Result<ServerInfo> {
        let clen_hint = content_length_from_url(url);

        // Prefer a 1-byte Range GET. Many CDNs reject or lie on HEAD, and a
        // successful 206 is the only reliable signal that segmented download works.
        let range_result = self.probe_range(url, user_agent, cookies, referer).await;
        match range_result {
            Ok(mut info) if info.content_length > 0 || info.supports_ranges => {
                if info.content_length == 0 {
                    info.content_length = clen_hint;
                }
                if info.filename_hint.is_none() {
                    info.filename_hint = filename_hint_from_url(url);
                }
                self.confirm_ranges_or_fallback(&mut info, user_agent, cookies, referer)
                    .await;
                return Ok(info);
            }
            Ok(mut info) => {
                if info.content_length == 0 {
                    info.content_length = clen_hint;
                }
                if info.content_length > 0 {
                    if info.filename_hint.is_none() {
                        info.filename_hint = filename_hint_from_url(url);
                    }
                    self.confirm_ranges_or_fallback(&mut info, user_agent, cookies, referer)
                        .await;
                    return Ok(info);
                }
            }
            Err(e) => {
                warn!("Range probe failed for {}: {:#}", url_host(url), e);
            }
        }

        match self.probe_head(url, user_agent, cookies, referer).await {
            Ok(mut info) => {
                if info.content_length == 0 {
                    info.content_length = clen_hint;
                }
                if info.filename_hint.is_none() {
                    info.filename_hint = filename_hint_from_url(url);
                }
                if info.content_length > 0 || info.supports_ranges || is_googlevideo(url) {
                    self.confirm_ranges_or_fallback(&mut info, user_agent, cookies, referer)
                        .await;
                    return Ok(info);
                }
            }
            Err(e) => {
                warn!("HEAD probe failed for {}: {:#}", url_host(url), e);
            }
        }

        // YouTube / googlevideo: clen in the query string is authoritative when
        // the CDN rejects probes (403). Proceed so the download can still start.
        if clen_hint > 0 {
            warn!(
                "Using clen={} from URL after probe failure ({})",
                clen_hint,
                url_host(url)
            );
            return Ok(ServerInfo {
                content_length: clen_hint,
                supports_ranges: is_googlevideo(url),
                content_type: mime_from_url(url),
                filename_hint: filename_hint_from_url(url),
                supports_http3: false,
                final_url: None,
            });
        }

        if is_googlevideo(url) {
            warn!("googlevideo probe failed with no clen — trying sequential download");
            return Ok(ServerInfo {
                content_length: 0,
                supports_ranges: false,
                content_type: mime_from_url(url),
                filename_hint: filename_hint_from_url(url),
                supports_http3: false,
                final_url: None,
            });
        }

        Err(anyhow::anyhow!(
            "Failed to probe download URL (host {}) [{:?}]",
            url_host(url),
            ErrorClass::Fatal
        ))
    }

    /// After a 206 probe (or Accept-Ranges claim), fetch a second real chunk and
    /// require a matching `Content-Range`. Otherwise force single-connection mode.
    async fn confirm_ranges_or_fallback(
        &self,
        info: &mut ServerInfo,
        user_agent: Option<&str>,
        cookies: Option<&str>,
        referer: Option<&str>,
    ) {
        if !info.supports_ranges {
            return;
        }
        let Some(url) = info
            .final_url
            .clone()
            .filter(|s| !s.is_empty())
        else {
            return;
        };

        // Need at least 2 bytes to verify a non-trivial partial range.
        if info.content_length > 0 && info.content_length < 2 {
            return;
        }

        let (start, end) = if info.content_length >= 2 {
            (1u64, 1u64)
        } else {
            (0u64, 0u64)
        };
        let range = format!("bytes={start}-{end}");

        match self
            .send_with_redirects(
                reqwest::Method::GET,
                &url,
                Some(&range),
                user_agent,
                cookies,
                referer,
            )
            .await
        {
            Ok((resp, final_url)) => {
                info.final_url = Some(final_url);
                let status = resp.status();
                let cr = resp
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                drop(resp);

                let ok = status == StatusCode::PARTIAL_CONTENT
                    && cr
                        .as_deref()
                        .and_then(parse_content_range)
                        .map(|(s, e, _)| s == start && e == end)
                        .unwrap_or(false);

                if !ok {
                    warn!(
                        "Range verification failed on {} (HTTP {}, Content-Range={:?}) — single connection",
                        url_host(&url),
                        status,
                        cr
                    );
                    info.supports_ranges = false;
                } else {
                    debug!("Range verification OK on {}", url_host(&url));
                }
            }
            Err(e) => {
                warn!(
                    "Range verification request failed on {}: {:#} — single connection",
                    url_host(&url),
                    e
                );
                info.supports_ranges = false;
            }
        }
    }

    async fn probe_head(
        &self,
        url: &str,
        user_agent: Option<&str>,
        cookies: Option<&str>,
        referer: Option<&str>,
    ) -> Result<ServerInfo> {
        let (resp, final_url) = self
            .send_with_redirects(
                reqwest::Method::HEAD,
                url,
                None,
                user_agent,
                cookies,
                referer,
            )
            .await
            .map_err(|e| {
                tracing::error!("HEAD probe error: {:?}", e);
                e
            })?;

        let status = resp.status();
        let mut info = Self::info_from_response(&resp, false).map_err(|e| {
            anyhow::anyhow!(
                "{e} (HEAD HTTP {status}) [{:?}]",
                classify_http_status(status)
            )
        })?;
        info.final_url = Some(final_url);
        Ok(info)
    }

    async fn probe_range(
        &self,
        url: &str,
        user_agent: Option<&str>,
        cookies: Option<&str>,
        referer: Option<&str>,
    ) -> Result<ServerInfo> {
        let (resp, final_url) = self
            .send_with_redirects(
                reqwest::Method::GET,
                url,
                Some("bytes=0-0"),
                user_agent,
                cookies,
                referer,
            )
            .await
            .context("Range probe failed")?;

        let status = resp.status();
        // Drop the body without reading it so a 200 (Range ignored) does not
        // pull the entire file during probe.
        let mut info = Self::info_from_response(&resp, true).map_err(|e| {
            anyhow::anyhow!(
                "{e} (Range HTTP {status}) [{:?}]",
                classify_http_status(status)
            )
        })?;
        info.final_url = Some(final_url);
        drop(resp);
        Ok(info)
    }

    fn info_from_response(resp: &Response, range_probe: bool) -> Result<ServerInfo> {
        let status = resp.status();
        if !status.is_success() && status != StatusCode::PARTIAL_CONTENT {
            anyhow::bail!("Server returned HTTP {}", status);
        }

        let headers = resp.headers();
        let is_206 = status == StatusCode::PARTIAL_CONTENT;

        let mut content_length = 0u64;
        if is_206 {
            if let Some(cr) = headers.get("content-range").and_then(|v| v.to_str().ok()) {
                if let Some((_, _, Some(total))) = parse_content_range(cr) {
                    content_length = total;
                } else if let Some(total_str) = cr.split('/').nth(1) {
                    if let Ok(t) = total_str.parse::<u64>() {
                        content_length = t;
                    }
                }
            }
        }

        if content_length == 0 {
            content_length = headers
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);

            // A Range probe that returned 200 + Content-Length is the full file.
            // bytes=0-0 would otherwise report length 1.
            if range_probe && !is_206 && content_length <= 1 {
                content_length = 0;
            }
        }

        let mut supports_ranges = is_206;
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

        let supports_http3 = headers
            .get("alt-svc")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("h3"))
            .unwrap_or(false);

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
            final_url: None,
        })
    }

    /// Send an HTTP GET with a Range header, returning a streaming response.
    /// Redirects are followed manually; `Content-Range` is checked on the first
    /// real partial response before the body is streamed.
    pub async fn get_range(
        &self,
        url: &str,
        range: &str,
        user_agent: Option<&str>,
        cookies: Option<&str>,
        referer: Option<&str>,
    ) -> Result<Response> {
        let range_opt = if range.is_empty() {
            None
        } else {
            Some(range)
        };

        let (resp, _final_url) = self
            .send_with_redirects(
                reqwest::Method::GET,
                url,
                range_opt,
                user_agent,
                cookies,
                referer,
            )
            .await
            .context("GET range request failed")?;

        let status = resp.status();

        if !range.is_empty() {
            if status == StatusCode::PARTIAL_CONTENT {
                let expected_start = parse_range_start(range);
                let cr_ok = resp
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_content_range)
                    .map(|(s, _, _)| expected_start.map(|e| e == s).unwrap_or(true))
                    .unwrap_or(false);

                if !cr_ok {
                    anyhow::bail!(
                        "Partial response missing/invalid Content-Range for {range} [{:?}]",
                        ErrorClass::Fatal
                    );
                }
                return Ok(resp);
            }

            if status.is_success() {
                let from_start = range.starts_with("bytes=0-");
                if from_start {
                    // Some origins ignore Range on the first segment but still
                    // deliver the body from offset 0 — acceptable for seg 0 only.
                    return Ok(resp);
                }
                anyhow::bail!(
                    "Server ignored Range header (HTTP {status}) [{:?}]",
                    ErrorClass::Fatal
                );
            }

            anyhow::bail!(
                "Range request returned HTTP {status} [{:?}]",
                classify_http_status(status)
            );
        }

        if status == StatusCode::PARTIAL_CONTENT || status.is_success() {
            return Ok(resp);
        }

        anyhow::bail!(
            "GET returned HTTP {status} [{:?}]",
            classify_http_status(status)
        );
    }
}

/// Classify HTTP status codes for worker retry policy.
pub fn classify_http_status(status: StatusCode) -> ErrorClass {
    match status.as_u16() {
        // Transient / overload
        408 | 425 | 429 | 500 | 502 | 503 | 504 => ErrorClass::Retryable,
        // Permanent client failures
        400 | 401 | 403 | 404 | 405 | 410 | 416 | 451 => ErrorClass::Fatal,
        code if (500..600).contains(&code) => ErrorClass::Retryable,
        code if (400..500).contains(&code) => ErrorClass::Fatal,
        _ => ErrorClass::Fatal,
    }
}

fn classify_reqwest_error(err: &reqwest::Error) -> ErrorClass {
    if err.is_timeout() || err.is_connect() {
        return ErrorClass::Retryable;
    }
    if let Some(status) = err.status() {
        return classify_http_status(status);
    }
    // Body / decode / connection reset mid-stream → retry the segment.
    if err.is_body() || err.is_request() {
        return ErrorClass::Retryable;
    }
    ErrorClass::Retryable
}

/// Walk an anyhow error chain and classify for segment worker retries.
pub fn classify_download_error(err: &anyhow::Error) -> ErrorClass {
    for cause in err.chain() {
        if let Some(re) = cause.downcast_ref::<reqwest::Error>() {
            return classify_reqwest_error(re);
        }
        let msg = cause.to_string();
        if let Some(class) = class_from_tagged_message(&msg) {
            return class;
        }
        if msg.contains("stalled") || msg.contains("connection") || msg.contains("timed out") {
            return ErrorClass::Retryable;
        }
        if msg.contains("ignored Range") || msg.contains("invalid Content-Range") {
            return ErrorClass::Fatal;
        }
        if let Some(code) = extract_http_status_from_message(&msg) {
            return classify_http_status(
                StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
        }
    }
    ErrorClass::Retryable
}

fn class_from_tagged_message(msg: &str) -> Option<ErrorClass> {
    // Tags are appended as `[{:?}]` → `[Fatal]` / `[Retryable]`.
    if msg.contains("[Fatal]") {
        return Some(ErrorClass::Fatal);
    }
    if msg.contains("[Retryable]") {
        return Some(ErrorClass::Retryable);
    }
    None
}

fn extract_http_status_from_message(msg: &str) -> Option<u16> {
    for token in ["HTTP ", "status ", "returned "] {
        if let Some(idx) = msg.find(token) {
            let rest = &msg[idx + token.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(code) = digits.parse::<u16>() {
                if (100..600).contains(&code) {
                    return Some(code);
                }
            }
        }
    }
    None
}

/// Parse `Content-Range: bytes start-end/total` (total may be `*`).
pub fn parse_content_range(header: &str) -> Option<(u64, u64, Option<u64>)> {
    let s = header.trim();
    let s = s.strip_prefix("bytes")?;
    let s = s.trim_start();
    let (range_part, total_part) = s.split_once('/')?;
    let (start_s, end_s) = range_part.split_once('-')?;
    let start = start_s.trim().parse().ok()?;
    let end = end_s.trim().parse().ok()?;
    let total = if total_part.trim() == "*" {
        None
    } else {
        Some(total_part.trim().parse().ok()?)
    };
    Some((start, end, total))
}

fn parse_range_start(range: &str) -> Option<u64> {
    // bytes=START-END or bytes=START-
    let s = range.strip_prefix("bytes=")?;
    let start = s.split('-').next()?;
    start.parse().ok()
}

fn resolve_redirect(current: &str, location: &str) -> Result<String> {
    if location.starts_with("http://") || location.starts_with("https://") {
        return Ok(location.to_string());
    }
    let base = Url::parse(current).context("invalid current URL during redirect")?;
    let joined = base
        .join(location)
        .context("invalid redirect Location")?;
    Ok(joined.to_string())
}

pub fn is_googlevideo_media_url(url: &str) -> bool {
    let Ok(u) = Url::parse(url) else {
        return false;
    };
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    (host.contains("googlevideo.com") || host.contains("youtube.com"))
        && u.path().to_ascii_lowercase().contains("videoplayback")
}

fn is_googlevideo(url: &str) -> bool {
    is_googlevideo_media_url(url)
        || Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.contains("googlevideo.com")))
            .unwrap_or(false)
}

fn url_host(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| "unknown".into())
}

pub fn content_length_from_url(url: &str) -> u64 {
    let Ok(u) = Url::parse(url) else {
        return 0;
    };
    for (k, v) in u.query_pairs() {
        if k == "clen" {
            if let Ok(n) = v.parse::<u64>() {
                return n;
            }
        }
    }
    0
}

fn mime_from_url(url: &str) -> Option<String> {
    let Ok(u) = Url::parse(url) else {
        return None;
    };
    for (k, v) in u.query_pairs() {
        if k == "mime" {
            return Some(v.replace("%2F", "/"));
        }
    }
    None
}

pub fn filename_hint_from_url(url: &str) -> Option<String> {
    let Ok(u) = Url::parse(url) else {
        return None;
    };
    let host = u.host_str().unwrap_or("");
    if !host.contains("googlevideo.com") && !u.path().contains("videoplayback") {
        return None;
    }

    let mut itag = String::new();
    let mut mime = String::new();
    for (k, v) in u.query_pairs() {
        match k.as_ref() {
            "itag" => itag = v.to_string(),
            "mime" => mime = v.replace("%2F", "/"),
            _ => {}
        }
    }

    let ext = if mime.contains("webm") {
        "webm"
    } else if mime.contains("audio") && mime.contains("mp4") {
        "m4a"
    } else if mime.contains("mp4") {
        "mp4"
    } else if !itag.is_empty() {
        "mp4"
    } else {
        return Some("youtube_video.mp4".into());
    };

    let quality = match itag.as_str() {
        "18" => "360p",
        "22" => "720p",
        "37" => "1080p",
        "136" | "247" | "298" | "302" | "398" => "720p",
        "137" | "248" | "299" | "303" | "399" => "1080p",
        "135" | "244" | "397" => "480p",
        "134" | "243" | "396" => "360p",
        "133" | "242" | "395" => "240p",
        "271" | "308" | "400" => "1440p",
        "313" | "315" | "401" => "2160p",
        "140" | "141" | "251" | "249" | "250" => "audio",
        other if !other.is_empty() => other,
        _ => "video",
    };

    Some(format!("youtube_{quality}.{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_parse() {
        let (s, e, t) = parse_content_range("bytes 0-0/950116729").unwrap();
        assert_eq!((s, e, t), (0, 0, Some(950_116_729)));
        let (s, e, t) = parse_content_range("bytes 100-199/*").unwrap();
        assert_eq!((s, e, t), (100, 199, None));
    }

    #[test]
    fn status_classifier() {
        assert_eq!(
            classify_http_status(StatusCode::TOO_MANY_REQUESTS),
            ErrorClass::Retryable
        );
        assert_eq!(
            classify_http_status(StatusCode::NOT_FOUND),
            ErrorClass::Fatal
        );
        assert_eq!(
            classify_http_status(StatusCode::SERVICE_UNAVAILABLE),
            ErrorClass::Retryable
        );
        assert_eq!(
            classify_http_status(StatusCode::FORBIDDEN),
            ErrorClass::Fatal
        );
    }

    #[test]
    fn download_error_tags() {
        let fatal = anyhow::anyhow!("Server ignored Range header (HTTP 200) [Fatal]");
        assert_eq!(classify_download_error(&fatal), ErrorClass::Fatal);
        let retry = anyhow::anyhow!("Stream stalled for 60s");
        assert_eq!(classify_download_error(&retry), ErrorClass::Retryable);
    }
}
