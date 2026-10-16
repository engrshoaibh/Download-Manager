//! MPEG-DASH manifests (static, single period): `SegmentTemplate` (with `$Number$` or a
//! `SegmentTimeline`), `SegmentList`, and single-file `SegmentBase` representations.

use super::{unsupported, Kind, Result, Seg, Track};

/// Minimal XML element tree; names are local (namespace prefixes dropped).
#[derive(Debug, Default, Clone)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    text: String,
}

impl Node {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

fn parse_xml(text: &str) -> Result<Node> {
    use quick_xml::events::{BytesStart, Event};
    use quick_xml::Reader;

    fn node(e: &BytesStart) -> Result<Node> {
        let mut n = Node { name: String::from_utf8_lossy(e.local_name().as_ref()).into_owned(), ..Default::default() };
        for a in e.attributes() {
            let a = a.map_err(|e| unsupported(format!("bad MPD attribute: {e}")))?;
            let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
            let value = a.unescape_value().map_err(|e| unsupported(format!("bad MPD attribute: {e}")))?.into_owned();
            n.attrs.push((key, value));
        }
        Ok(n)
    }

    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Node> = vec![Node::default()];
    loop {
        match reader.read_event().map_err(|e| unsupported(format!("bad MPD XML: {e}")))? {
            Event::Start(e) => stack.push(node(&e)?),
            Event::Empty(e) => {
                let n = node(&e)?;
                stack.last_mut().unwrap().children.push(n);
            }
            Event::End(_) => {
                let n = stack.pop().unwrap();
                stack.last_mut().ok_or_else(|| unsupported("bad MPD nesting"))?.children.push(n);
            }
            Event::Text(t) => {
                let s = t.unescape().map_err(|e| unsupported(format!("bad MPD text: {e}")))?;
                stack.last_mut().unwrap().text.push_str(s.trim());
            }
            Event::CData(t) => stack.last_mut().unwrap().text.push_str(String::from_utf8_lossy(&t).trim()),
            Event::Eof => break,
            _ => {}
        }
    }
    let root = stack.pop().filter(|_| stack.is_empty()).ok_or_else(|| unsupported("unclosed MPD element"))?;
    root.children.into_iter().find(|c| c.name == "MPD").ok_or_else(|| unsupported("no MPD element"))
}

/// ISO 8601 duration (`PT1H2M3.5S`, `P1DT2H`) in seconds.
pub fn iso_duration(s: &str) -> Option<f64> {
    let s = s.trim().strip_prefix('P')?;
    let (date, time) = s.split_once('T').unwrap_or((s, ""));
    let mut secs = 0.0;
    let mut num = String::new();
    for (part, units) in [(date, &[('Y', 31_536_000.0), ('M', 2_592_000.0), ('W', 604_800.0), ('D', 86_400.0)][..]), (time, &[('H', 3600.0), ('M', 60.0), ('S', 1.0)][..])] {
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' {
                num.push(c);
            } else {
                let mult = units.iter().find(|(u, _)| *u == c)?.1;
                secs += num.parse::<f64>().ok()? * mult;
                num.clear();
            }
        }
    }
    Some(secs)
}

/// Fill `$RepresentationID$`, `$Number%05d$`, `$Time$`, `$Bandwidth$`, `$$`.
fn fill(template: &str, id: &str, bandwidth: u64, number: u64, time: u64) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('$') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &after[..end];
        let (name, fmt) = token.split_once('%').unwrap_or((token, ""));
        let width: usize = fmt.trim_start_matches('0').trim_end_matches('d').parse().unwrap_or(0);
        let num = |v: u64| format!("{v:0width$}");
        match name {
            "" => out.push('$'),
            "RepresentationID" => out.push_str(id),
            "Number" => out.push_str(&num(number)),
            "Time" => out.push_str(&num(time)),
            "Bandwidth" => out.push_str(&num(bandwidth)),
            _ => {
                out.push('$');
                out.push_str(token);
                out.push('$');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn join(base: &url::Url, uri: &str) -> Result<url::Url> {
    base.join(uri.trim()).map_err(|e| unsupported(format!("bad URL {uri}: {e}")))
}

fn with_base(base: &url::Url, node: &Node) -> Result<url::Url> {
    match node.child("BaseURL").map(|b| b.text.as_str()).filter(|t| !t.is_empty()) {
        Some(b) => join(base, b),
        None => Ok(base.clone()),
    }
}

fn range(s: Option<&str>) -> Option<(u64, u64)> {
    let (a, b) = s?.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    Video,
    Audio,
    Other,
}

#[derive(Debug, Clone)]
pub struct Representation {
    pub id: String,
    pub content: Content,
    pub bandwidth: u64,
    pub height: Option<u32>,
    pub track: Track,
}

/// Merge SegmentTemplate attributes from the AdaptationSet and the Representation
/// (the Representation wins), keeping the nearest SegmentTimeline.
fn template(set: &Node, rep: &Node) -> Option<Node> {
    let outer = set.child("SegmentTemplate");
    let inner = rep.child("SegmentTemplate");
    let mut t = outer.or(inner)?.clone();
    if let (Some(_), Some(inner)) = (outer, inner) {
        for (k, v) in &inner.attrs {
            t.attrs.retain(|(kk, _)| kk != k);
            t.attrs.push((k.clone(), v.clone()));
        }
        if inner.child("SegmentTimeline").is_some() {
            t.children = inner.children.clone();
        }
    }
    Some(t)
}

pub fn parse(text: &str, manifest_url: &str) -> Result<Vec<Representation>> {
    let mpd = parse_xml(text)?;
    if mpd.attr("type") == Some("dynamic") {
        return Err(unsupported("live DASH (type=dynamic)"));
    }
    let periods: Vec<_> = mpd.all("Period").collect();
    if periods.len() != 1 {
        return Err(unsupported(format!("{} periods", periods.len())));
    }
    let period = periods[0];
    let duration = period
        .attr("duration")
        .or(mpd.attr("mediaPresentationDuration"))
        .and_then(iso_duration)
        .unwrap_or(0.0);
    let base = url::Url::parse(manifest_url).map_err(|e| unsupported(format!("bad manifest URL: {e}")))?;
    let base = with_base(&with_base(&base, &mpd)?, period)?;

    let mut out = Vec::new();
    for set in period.all("AdaptationSet") {
        if set.child("ContentProtection").is_some() || set.all("Representation").any(|r| r.child("ContentProtection").is_some()) {
            return Err(unsupported("DRM-protected (ContentProtection)"));
        }
        let set_base = with_base(&base, set)?;
        for rep in set.all("Representation") {
            let mime = rep.attr("mimeType").or(set.attr("mimeType")).unwrap_or("");
            let content = match set.attr("contentType").or_else(|| mime.split('/').next()) {
                Some("video") => Content::Video,
                Some("audio") => Content::Audio,
                _ => Content::Other,
            };
            if content == Content::Other {
                continue;
            }
            let kind = if mime.contains("webm") {
                Kind::WebM
            } else if mime.contains("mp2t") {
                Kind::Ts
            } else {
                Kind::Fmp4
            };
            let id = rep.attr("id").unwrap_or("").to_string();
            let bandwidth: u64 = rep.attr("bandwidth").and_then(|b| b.parse().ok()).unwrap_or(0);
            let rep_base = with_base(&set_base, rep)?;
            let (init, segments) = segments(set, rep, &rep_base, &id, bandwidth, duration)?;
            out.push(Representation {
                id,
                content,
                bandwidth,
                height: rep.attr("height").or(set.attr("height")).and_then(|h| h.parse().ok()),
                track: Track { init, segments, kind, duration },
            });
        }
    }
    if out.is_empty() {
        return Err(unsupported("no audio or video representations"));
    }
    Ok(out)
}

fn seg(url: url::Url, range: Option<(u64, u64)>) -> Seg {
    Seg { url: url.to_string(), range, key: None }
}

fn segments(set: &Node, rep: &Node, base: &url::Url, id: &str, bw: u64, duration: f64) -> Result<(Option<Seg>, Vec<Seg>)> {
    if let Some(t) = template(set, rep) {
        let media = t.attr("media").ok_or_else(|| unsupported("SegmentTemplate without media"))?;
        let timescale: u64 = t.attr("timescale").and_then(|v| v.parse().ok()).filter(|&v| v > 0).unwrap_or(1);
        let start: u64 = t.attr("startNumber").and_then(|v| v.parse().ok()).unwrap_or(1);
        let init = t.attr("initialization").map(|i| join(base, &fill(i, id, bw, 0, 0))).transpose()?.map(|u| seg(u, None));
        let mut list = Vec::new();
        if let Some(timeline) = t.child("SegmentTimeline") {
            let end_time = (duration * timescale as f64).round() as u64;
            let (mut time, mut number) = (0u64, start);
            for s in timeline.all("S") {
                if let Some(t0) = s.attr("t").and_then(|v| v.parse().ok()) {
                    time = t0;
                }
                let d: u64 = s.attr("d").and_then(|v| v.parse().ok()).filter(|&d| d > 0).ok_or_else(|| unsupported("S without d"))?;
                let r: i64 = s.attr("r").and_then(|v| v.parse().ok()).unwrap_or(0);
                let count = if r < 0 { end_time.saturating_sub(time).div_ceil(d) } else { r as u64 + 1 };
                for _ in 0..count {
                    list.push(seg(join(base, &fill(media, id, bw, number, time))?, None));
                    time += d;
                    number += 1;
                }
            }
        } else {
            let seg_dur: u64 = t.attr("duration").and_then(|v| v.parse().ok()).filter(|&d| d > 0).ok_or_else(|| unsupported("SegmentTemplate without duration or timeline"))?;
            if duration <= 0.0 {
                return Err(unsupported("unknown presentation duration"));
            }
            let count = ((duration * timescale as f64) / seg_dur as f64).ceil() as u64;
            for i in 0..count {
                list.push(seg(join(base, &fill(media, id, bw, start + i, i * seg_dur))?, None));
            }
        }
        return Ok((init, list));
    }
    if let Some(sl) = rep.child("SegmentList").or(set.child("SegmentList")) {
        let init = match sl.child("Initialization") {
            Some(i) => Some(seg(join(base, i.attr("sourceURL").unwrap_or(""))?, range(i.attr("range")))),
            None => None,
        };
        let mut list = Vec::new();
        for s in sl.all("SegmentURL") {
            list.push(seg(join(base, s.attr("media").unwrap_or(""))?, range(s.attr("mediaRange"))));
        }
        return Ok((init, list));
    }
    // SegmentBase or nothing: the BaseURL is the whole (self-initialising) file.
    Ok((None, vec![seg(base.clone(), None)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_templates() {
        assert_eq!(iso_duration("PT4.0S"), Some(4.0));
        assert_eq!(iso_duration("PT1H2M3.5S"), Some(3723.5));
        assert_eq!(iso_duration("P1DT1S"), Some(86_401.0));
        assert_eq!(fill("c-$RepresentationID$-$Number%05d$.m4s", "v1", 0, 42, 0), "c-v1-00042.m4s");
        assert_eq!(fill("$Time$_$Bandwidth$$$", "x", 800, 0, 9000), "9000_800$");
    }

    #[test]
    fn template_number_and_list() {
        let mpd = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT9S">
 <BaseURL>https://cdn.example.com/media/</BaseURL>
 <Period>
  <AdaptationSet mimeType="video/mp4">
   <SegmentTemplate timescale="1000" duration="4000" startNumber="0" media="$RepresentationID$/$Number$.m4s" initialization="$RepresentationID$/init.mp4"/>
   <Representation id="720" bandwidth="2000000" width="1280" height="720"/>
  </AdaptationSet>
  <AdaptationSet mimeType="audio/mp4">
   <Representation id="a" bandwidth="128000">
    <BaseURL>audio.mp4</BaseURL>
    <SegmentList><Initialization sourceURL="audio.mp4" range="0-99"/><SegmentURL media="audio.mp4" mediaRange="100-999"/></SegmentList>
   </Representation>
  </AdaptationSet>
 </Period>
</MPD>"#;
        let reps = parse(mpd, "https://site.example.com/m.mpd").unwrap();
        let v = &reps[0];
        assert_eq!(v.height, Some(720));
        assert_eq!(v.track.init.as_ref().unwrap().url, "https://cdn.example.com/media/720/init.mp4");
        let urls: Vec<_> = v.track.segments.iter().map(|s| s.url.as_str()).collect();
        assert_eq!(urls, ["https://cdn.example.com/media/720/0.m4s", "https://cdn.example.com/media/720/1.m4s", "https://cdn.example.com/media/720/2.m4s"]);
        let a = &reps[1];
        assert_eq!(a.content, Content::Audio);
        assert_eq!(a.track.init.as_ref().unwrap().range, Some((0, 99)));
        assert_eq!(a.track.segments[0].range, Some((100, 999)));
        assert_eq!(a.track.segments[0].url, "https://cdn.example.com/media/audio.mp4");
    }

    #[test]
    fn timeline_with_repeats_and_open_end() {
        let mpd = r#"<MPD type="static" mediaPresentationDuration="PT10S"><Period><AdaptationSet contentType="video" mimeType="video/mp4">
          <Representation id="v" bandwidth="1"><SegmentTemplate timescale="10" media="$Time$.m4s">
            <SegmentTimeline><S t="0" d="20" r="1"/><S d="30" r="-1"/></SegmentTimeline></SegmentTemplate></Representation>
          </AdaptationSet></Period></MPD>"#;
        let reps = parse(mpd, "https://x.example/a/m.mpd").unwrap();
        let names: Vec<_> = reps[0].track.segments.iter().map(|s| s.url.rsplit('/').next().unwrap().to_string()).collect();
        assert_eq!(names, ["0.m4s", "20.m4s", "40.m4s", "70.m4s"]);
    }

    #[test]
    fn refuses_live_drm_and_multiperiod() {
        let live = r#"<MPD type="dynamic"><Period/></MPD>"#;
        assert!(parse(live, "https://x.example/m.mpd").is_err());
        let drm = r#"<MPD type="static"><Period><AdaptationSet mimeType="video/mp4"><ContentProtection schemeIdUri="urn:x"/><Representation id="1" bandwidth="1"/></AdaptationSet></Period></MPD>"#;
        assert!(parse(drm, "https://x.example/m.mpd").is_err());
        let two = r#"<MPD type="static"><Period/><Period/></MPD>"#;
        assert!(parse(two, "https://x.example/m.mpd").is_err());
    }
}
