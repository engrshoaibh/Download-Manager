//! HLS playlists (RFC 8216): master playlists with variants and audio renditions, and
//! VOD media playlists with byte ranges, AES-128 keys and fMP4 init sections.

use super::{unsupported, Key, Kind, Result, Seg, Track};

#[derive(Debug)]
pub enum Playlist {
    Master(Master),
    Media(Track),
}

#[derive(Debug, Default)]
pub struct Master {
    pub variants: Vec<Variant>,
    pub audio: Vec<Rendition>,
}

#[derive(Debug)]
pub struct Variant {
    pub url: String,
    pub bandwidth: u64,
    pub height: Option<u32>,
    pub audio_group: Option<String>,
}

#[derive(Debug)]
pub struct Rendition {
    pub group: String,
    /// `None` when the audio is muxed into the variant streams.
    pub url: Option<String>,
    pub default: bool,
}

/// `KEY=value,KEY="quoted, value"` → pairs (quotes removed).
pub fn attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim().to_ascii_uppercase();
        rest = &rest[eq + 1..];
        let value;
        if let Some(stripped) = rest.strip_prefix('"') {
            let end = stripped.find('"').unwrap_or(stripped.len());
            value = stripped[..end].to_string();
            rest = stripped.get(end + 1..).unwrap_or("");
            rest = rest.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            value = rest[..end].trim().to_string();
            rest = rest.get(end + 1..).unwrap_or("");
        }
        out.push((key, value));
    }
    out
}

fn attr<'a>(a: &'a [(String, String)], key: &str) -> Option<&'a str> {
    a.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn resolve(base: &str, uri: &str) -> Result<String> {
    let base = url::Url::parse(base).map_err(|e| unsupported(format!("bad playlist URL: {e}")))?;
    base.join(uri.trim())
        .map(|u| u.to_string())
        .map_err(|e| unsupported(format!("bad segment URL {uri}: {e}")))
}

/// `len[@offset]` → (len, offset)
fn byterange(s: &str) -> Option<(u64, Option<u64>)> {
    let mut it = s.trim().splitn(2, '@');
    let len = it.next()?.trim().parse().ok()?;
    let off = it.next().and_then(|o| o.trim().parse().ok());
    Some((len, off))
}

fn hex_iv(s: &str) -> Option<[u8; 16]> {
    let h = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    let v = u128::from_str_radix(h, 16).ok()?;
    Some(v.to_be_bytes())
}

pub fn parse(text: &str, base: &str) -> Result<Playlist> {
    if !text.trim_start().starts_with("#EXTM3U") {
        return Err(unsupported("not an M3U8 playlist"));
    }
    if text.contains("#EXT-X-STREAM-INF") {
        parse_master(text, base).map(Playlist::Master)
    } else {
        parse_media(text, base).map(Playlist::Media)
    }
}

fn parse_master(text: &str, base: &str) -> Result<Master> {
    let mut m = Master::default();
    let mut pending: Option<Vec<(String, String)>> = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending = Some(attrs(rest));
        } else if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA:") {
            let a = attrs(rest);
            if attr(&a, "TYPE") == Some("AUDIO") {
                m.audio.push(Rendition {
                    group: attr(&a, "GROUP-ID").unwrap_or("").to_string(),
                    url: attr(&a, "URI").map(|u| resolve(base, u)).transpose()?,
                    default: attr(&a, "DEFAULT") == Some("YES"),
                });
            }
        } else if line.starts_with('#') {
            continue;
        } else if let Some(a) = pending.take() {
            let height = attr(&a, "RESOLUTION")
                .and_then(|r| r.split(['x', 'X']).nth(1))
                .and_then(|h| h.parse().ok());
            m.variants.push(Variant {
                url: resolve(base, line)?,
                bandwidth: attr(&a, "BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0),
                height,
                audio_group: attr(&a, "AUDIO").map(str::to_string),
            });
        }
    }
    if m.variants.is_empty() {
        return Err(unsupported("master playlist without variants"));
    }
    Ok(m)
}

fn kind_from_url(url: &str) -> Kind {
    let path = url::Url::parse(url).map(|u| u.path().to_ascii_lowercase()).unwrap_or_default();
    if path.ends_with(".m4s") || path.ends_with(".mp4") || path.ends_with(".m4a") || path.ends_with(".m4v") || path.ends_with(".cmfv") || path.ends_with(".cmfa") {
        Kind::Fmp4
    } else if path.ends_with(".aac") {
        Kind::Aac
    } else if path.ends_with(".webm") {
        Kind::WebM
    } else {
        Kind::Ts
    }
}

fn parse_media(text: &str, base: &str) -> Result<Track> {
    let mut segments = Vec::new();
    let mut init: Option<Seg> = None;
    let mut duration = 0.0;
    let mut seq: u64 = 0;
    let mut key: Option<(String, Option<[u8; 16]>)> = None;
    let mut pending_dur = 0.0;
    let mut pending_range: Option<(u64, Option<u64>)> = None;
    let mut last_end: Option<(String, u64)> = None;
    let mut ended = false;

    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(v) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            seq = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("#EXTINF:") {
            pending_dur = v.split(',').next().and_then(|d| d.trim().parse().ok()).unwrap_or(0.0);
        } else if let Some(v) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            pending_range = byterange(v);
        } else if line == "#EXT-X-ENDLIST" {
            ended = true;
        } else if let Some(v) = line.strip_prefix("#EXT-X-KEY:") {
            let a = attrs(v);
            match attr(&a, "METHOD").unwrap_or("NONE") {
                "NONE" => key = None,
                "AES-128" => {
                    if attr(&a, "KEYFORMAT").map(|f| f != "identity").unwrap_or(false) {
                        return Err(unsupported("DRM key format"));
                    }
                    let uri = attr(&a, "URI").ok_or_else(|| unsupported("AES-128 key without URI"))?;
                    key = Some((resolve(base, uri)?, attr(&a, "IV").and_then(hex_iv)));
                }
                other => return Err(unsupported(format!("encryption method {other}"))),
            }
        } else if let Some(v) = line.strip_prefix("#EXT-X-MAP:") {
            let a = attrs(v);
            let url = resolve(base, attr(&a, "URI").ok_or_else(|| unsupported("EXT-X-MAP without URI"))?)?;
            let range = attr(&a, "BYTERANGE").and_then(byterange).map(|(len, off)| {
                let start = off.unwrap_or(0);
                (start, start + len - 1)
            });
            let map = Seg { url, range, key: None };
            if init.as_ref().map(|i| *i != map).unwrap_or(false) {
                return Err(unsupported("init section changes mid-playlist"));
            }
            init = Some(map);
        } else if line.starts_with('#') {
            continue;
        } else {
            let url = resolve(base, line)?;
            let range = pending_range.take().map(|(len, off)| {
                let start = off.unwrap_or_else(|| match &last_end {
                    Some((u, end)) if *u == url => *end,
                    _ => 0,
                });
                (start, start + len - 1)
            });
            if let Some((_, end)) = range {
                last_end = Some((url.clone(), end + 1));
            }
            let seg_key = key.as_ref().map(|(k, iv)| Key {
                url: k.clone(),
                // No IV attribute: the media sequence number, big-endian (RFC 8216 5.2).
                iv: iv.unwrap_or_else(|| (seq as u128).to_be_bytes()),
            });
            segments.push(Seg { url, range, key: seg_key });
            duration += pending_dur;
            pending_dur = 0.0;
            seq += 1;
        }
    }
    if !ended {
        return Err(unsupported("live playlist (no EXT-X-ENDLIST)"));
    }
    if segments.is_empty() {
        return Err(unsupported("playlist without segments"));
    }
    let kind = if init.is_some() { Kind::Fmp4 } else { kind_from_url(&segments[0].url) };
    Ok(Track { init, segments, kind, duration })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://cdn.example.com/v/master.m3u8?token=1";

    #[test]
    fn master_with_audio_group() {
        let text = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English, main\",DEFAULT=YES,URI=\"a/en.m3u8\"\n\
            #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,CODECS=\"avc1.4d401e,mp4a.40.2\",AUDIO=\"aud\"\n360/index.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080,AUDIO=\"aud\"\nhttps://other.example.com/1080.m3u8\n";
        let Playlist::Master(m) = parse(text, BASE).unwrap() else { panic!("expected master") };
        assert_eq!(m.variants.len(), 2);
        assert_eq!(m.variants[0].url, "https://cdn.example.com/v/360/index.m3u8");
        assert_eq!(m.variants[0].height, Some(360));
        assert_eq!(m.variants[1].url, "https://other.example.com/1080.m3u8");
        assert_eq!(m.audio[0].url.as_deref(), Some("https://cdn.example.com/v/a/en.m3u8"));
        assert!(m.audio[0].default);
        assert_eq!(m.variants[1].audio_group.as_deref(), Some("aud"));
    }

    #[test]
    fn media_with_keys_and_byteranges() {
        let text = "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-KEY:METHOD=AES-128,URI=\"k.bin\"\n\
            #EXTINF:4.0,\n#EXT-X-BYTERANGE:1000@0\nall.ts\n#EXTINF:4.0,\n#EXT-X-BYTERANGE:500\nall.ts\n\
            #EXT-X-KEY:METHOD=NONE\n#EXTINF:2.5,\nlast.ts\n#EXT-X-ENDLIST\n";
        let Playlist::Media(t) = parse(text, BASE).unwrap() else { panic!("expected media") };
        assert_eq!(t.kind, Kind::Ts);
        assert_eq!(t.segments.len(), 3);
        assert_eq!(t.segments[0].range, Some((0, 999)));
        assert_eq!(t.segments[1].range, Some((1000, 1499)));
        assert_eq!(t.segments[0].key.as_ref().unwrap().iv, 7u128.to_be_bytes());
        assert_eq!(t.segments[1].key.as_ref().unwrap().iv, 8u128.to_be_bytes());
        assert!(t.segments[2].key.is_none());
        assert!((t.duration - 10.5).abs() < 1e-9);
    }

    #[test]
    fn refuses_live_and_drm() {
        let live = "#EXTM3U\n#EXTINF:4,\na.ts\n";
        assert!(matches!(parse(live, BASE), Err(super::super::StreamError::Unsupported(_))));
        let drm = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://x\"\n#EXTINF:4,\na.ts\n#EXT-X-ENDLIST\n";
        assert!(matches!(parse(drm, BASE), Err(super::super::StreamError::Unsupported(_))));
    }

    #[test]
    fn fmp4_init_section() {
        let text = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"800@0\"\n#EXTINF:2,\nseg1.m4s\n#EXT-X-ENDLIST\n";
        let Playlist::Media(t) = parse(text, BASE).unwrap() else { panic!() };
        assert_eq!(t.kind, Kind::Fmp4);
        assert_eq!(t.init.unwrap().range, Some((0, 799)));
    }
}
