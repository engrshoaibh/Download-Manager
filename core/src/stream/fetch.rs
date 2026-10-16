//! HTTP side of the stream downloader: manifests, segments (parallel, written in order),
//! AES-128 decryption, retries, and a small state file so Pause/Resume continues.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Context};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::{Key, Result, Seg, StreamError, Track};

/// Segments in flight at once.
const PARALLEL: usize = 8;
const RETRIES: u32 = 4;
const DEFAULT_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

pub struct Fetcher {
    client: reqwest::Client,
    user_agent: String,
    referer: Option<String>,
    cookies: Option<String>,
    /// Site of the manifest; a plain `Cookie` header is only sent back to it.
    site: String,
    keys: tokio::sync::Mutex<HashMap<String, [u8; 16]>>,
}

/// Last two host labels ("cdn.example.com" → "example.com"), good enough to keep
/// cookies on the site that set them.
fn site_of(host: &str) -> String {
    let labels: Vec<_> = host.trim_end_matches('.').rsplit('.').take(2).collect();
    labels.into_iter().rev().collect::<Vec<_>>().join(".")
}

fn is_cookie_jar(c: &str) -> bool {
    c.trim_start().starts_with("# Netscape") || c.lines().any(|l| l.split('\t').count() >= 7)
}

/// `Cookie` header for `url` from a header string or a Netscape cookie jar.
fn cookie_header(cookies: &str, site: &str, url: &url::Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    if is_cookie_jar(cookies) {
        let pairs: Vec<String> = cookies
            .lines()
            .filter(|l| !l.starts_with('#') || l.starts_with("#HttpOnly_"))
            .filter_map(|l| {
                let f: Vec<_> = l.trim_start_matches("#HttpOnly_").split('\t').collect();
                if f.len() < 7 {
                    return None;
                }
                let domain = f[0].trim_start_matches('.').to_ascii_lowercase();
                (host == domain || host.ends_with(&format!(".{domain}"))).then(|| format!("{}={}", f[5], f[6].trim()))
            })
            .collect();
        (!pairs.is_empty()).then(|| pairs.join("; "))
    } else {
        (site_of(&host) == site).then(|| cookies.trim().to_string()).filter(|c| !c.is_empty())
    }
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    items: usize,
    done: usize,
    bytes: u64,
}

impl Fetcher {
    pub fn new(req: &super::Request) -> Result<Self> {
        let url = url::Url::parse(&req.url).map_err(|e| super::unsupported(format!("bad URL: {e}")))?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(anyhow::Error::from)?;
        Ok(Self {
            client,
            user_agent: req.user_agent.clone().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| DEFAULT_UA.into()),
            referer: req.referer.clone().filter(|r| !r.trim().is_empty()),
            cookies: req.cookies.clone().filter(|c| !c.trim().is_empty()),
            site: site_of(url.host_str().unwrap_or("")),
            keys: Default::default(),
        })
    }

    fn request(&self, url: &str) -> anyhow::Result<reqwest::RequestBuilder> {
        let parsed = url::Url::parse(url)?;
        let mut rb = self.client.get(url).header("User-Agent", &self.user_agent);
        if let Some(r) = &self.referer {
            rb = rb.header("Referer", r);
        }
        if let Some(c) = self.cookies.as_deref().and_then(|c| cookie_header(c, &self.site, &parsed)) {
            rb = rb.header("Cookie", c);
        }
        Ok(rb)
    }

    async fn get(&self, url: &str, range: Option<(u64, u64)>) -> anyhow::Result<Vec<u8>> {
        let mut last = anyhow!("no attempt");
        for attempt in 0..=RETRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
            }
            let mut rb = self.request(url)?;
            if let Some((a, b)) = range {
                rb = rb.header("Range", format!("bytes={a}-{b}"));
            }
            match rb.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_client_error() {
                        // 403/404 won't fix themselves (expired token, missing segment).
                        return Err(anyhow!("HTTP {status} for {url}"));
                    }
                    if !status.is_success() {
                        last = anyhow!("HTTP {status} for {url}");
                        continue;
                    }
                    if range.is_some() && status.as_u16() != 206 {
                        return Err(anyhow!("server ignored the byte range for {url}"));
                    }
                    match resp.bytes().await {
                        Ok(b) => return Ok(b.to_vec()),
                        Err(e) => last = e.into(),
                    }
                }
                Err(e) => last = e.into(),
            }
        }
        Err(last.context(format!("giving up on {url}")))
    }

    pub async fn text(&self, url: &str) -> Result<String> {
        let body = self.get(url, None).await?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    async fn key(&self, url: &str) -> anyhow::Result<[u8; 16]> {
        if let Some(k) = self.keys.lock().await.get(url) {
            return Ok(*k);
        }
        let body = self.get(url, None).await?;
        let key: [u8; 16] = body.as_slice().try_into().map_err(|_| anyhow!("AES key is {} bytes, not 16", body.len()))?;
        self.keys.lock().await.insert(url.to_string(), key);
        Ok(key)
    }

    async fn segment(&self, seg: &Seg) -> anyhow::Result<Vec<u8>> {
        let data = self.get(&seg.url, seg.range).await?;
        match &seg.key {
            None => Ok(data),
            Some(Key { url, iv }) => {
                use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
                let key = self.key(url).await?;
                cbc::Decryptor::<aes::Aes128>::new(&key.into(), iv.into())
                    .decrypt_padded_vec_mut::<Pkcs7>(&data)
                    .map_err(|_| anyhow!("AES-128 decryption failed for {}", seg.url))
            }
        }
    }

    /// Download every piece of `track` (init first) into `out`, in order. Resumes from
    /// `state` when it matches the file on disk. `progress(items_done, bytes_done)`.
    /// Returns the final (items, bytes).
    pub async fn track_to_file(
        &self,
        track: &Track,
        out: &Path,
        state_path: &Path,
        stop: &AtomicBool,
        progress: &mut (dyn FnMut(usize, u64) + Send),
    ) -> Result<(usize, u64)> {
        let items = track.items();
        let saved: State = std::fs::read(state_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let on_disk = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
        let mut st = if saved.items == items.len() && saved.done <= items.len() && on_disk >= saved.bytes {
            saved
        } else {
            State { items: items.len(), ..Default::default() }
        };
        let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(out).context("open stream output")?;
        file.set_len(st.bytes).context("trim stream output")?;
        use std::io::Seek;
        file.seek(std::io::SeekFrom::End(0)).context("seek stream output")?;
        progress(st.done, st.bytes);

        let mut pending = futures_util::stream::iter(items[st.done..].iter().cloned())
            .map(|seg| async move { self.segment(&seg).await })
            .buffered(PARALLEL);
        while let Some(data) = pending.next().await {
            if stop.load(Ordering::Relaxed) {
                return Err(StreamError::Stopped);
            }
            let data = data?;
            file.write_all(&data).context("write stream output")?;
            st.done += 1;
            st.bytes += data.len() as u64;
            if let Err(e) = std::fs::write(state_path, serde_json::to_vec(&st).unwrap_or_default()) {
                warn!("Could not save stream progress: {e}");
            }
            progress(st.done, st.bytes);
        }
        file.sync_all().context("flush stream output")?;
        Ok((st.done, st.bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_stay_on_their_site() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        let site = site_of("www.example.com");
        assert_eq!(cookie_header("a=1; b=2", &site, &u("https://cdn.example.com/x.ts")).as_deref(), Some("a=1; b=2"));
        assert_eq!(cookie_header("a=1", &site, &u("https://evil.test/x.ts")), None);
        let jar = "# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tTRUE\t0\tsid\tabc\nother.org\tFALSE\t/\tFALSE\t0\tx\ty\n";
        assert_eq!(cookie_header(jar, &site, &u("https://media.example.com/a")).as_deref(), Some("sid=abc"));
        assert_eq!(cookie_header(jar, &site, &u("https://other.org/a")).as_deref(), Some("x=y"));
    }
}
