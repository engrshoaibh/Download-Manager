//! Fragmented MP4 (DASH/CMAF, as YouTube serves it) video + audio → one fragmented MP4.
//!
//! Each input has `ftyp`, `moov` (one `trak` plus `mvex`), optional `sidx`, then
//! `moof`+`mdat` pairs. The output gets one `moov` holding both tracks, the fragments of
//! both inputs interleaved by decode time (renumbered, track IDs rewritten), and an
//! `mfra` index so players can seek. Sample data is copied byte for byte.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use super::{copy_range, invalid, read_at, unsupported, MuxError, Progress, Result};

const VIDEO_ID: u32 = 1;
const AUDIO_ID: u32 = 2;

fn u32_at(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_be_bytes(s.try_into().unwrap()))
        .ok_or_else(|| invalid("truncated MP4 box"))
}

fn u64_at(b: &[u8], o: usize) -> Result<u64> {
    b.get(o..o + 8)
        .map(|s| u64::from_be_bytes(s.try_into().unwrap()))
        .ok_or_else(|| invalid("truncated MP4 box"))
}

fn put_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

fn put_u64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_be_bytes());
}

/// A box inside an in-memory buffer: `start..end` is the whole box, `body` where its payload starts.
#[derive(Clone, Copy, Debug)]
struct BoxRef {
    typ: [u8; 4],
    start: usize,
    body: usize,
    end: usize,
}

fn children(buf: &[u8], from: usize, to: usize) -> Result<Vec<BoxRef>> {
    let mut out = Vec::new();
    let mut o = from;
    while o + 8 <= to {
        let size32 = u32_at(buf, o)? as u64;
        let typ: [u8; 4] = buf[o + 4..o + 8].try_into().unwrap();
        let (size, hdr) = match size32 {
            0 => ((to - o) as u64, 8),
            1 => (u64_at(buf, o + 8)?, 16),
            s => (s, 8),
        };
        if size < hdr as u64 || o as u64 + size > to as u64 {
            return Err(invalid(format!("bad size for box {}", String::from_utf8_lossy(&typ))));
        }
        out.push(BoxRef { typ, start: o, body: o + hdr, end: o + size as usize });
        o += size as usize;
    }
    Ok(out)
}

fn kids(buf: &[u8], b: BoxRef) -> Result<Vec<BoxRef>> {
    children(buf, b.body, b.end)
}

fn find(list: &[BoxRef], typ: &[u8; 4]) -> Option<BoxRef> {
    list.iter().copied().find(|b| &b.typ == typ)
}

fn need(list: &[BoxRef], typ: &[u8; 4]) -> Result<BoxRef> {
    find(list, typ).ok_or_else(|| invalid(format!("missing {} box", String::from_utf8_lossy(typ))))
}

/// A top-level box in a file on disk.
struct FileBox {
    typ: [u8; 4],
    start: u64,
    size: u64,
}

fn scan_top(f: &mut File) -> Result<Vec<FileBox>> {
    let len = f.seek(SeekFrom::End(0))?;
    let mut out = Vec::new();
    let mut o = 0u64;
    while o + 8 <= len {
        let head = read_at(f, o, if o + 16 <= len { 16 } else { 8 })?;
        let size32 = u32_at(&head, 0)? as u64;
        let typ: [u8; 4] = head[4..8].try_into().unwrap();
        let (size, hdr) = match size32 {
            0 => (len - o, 8),
            1 => (u64_at(&head, 8)?, 16),
            s => (s, 8),
        };
        if size < hdr || o + size > len {
            return Err(invalid(format!("bad size for top-level box {}", String::from_utf8_lossy(&typ))));
        }
        out.push(FileBox { typ, start: o, size });
        o += size;
    }
    Ok(out)
}

struct Fragment {
    moof: Vec<u8>,
    moof_start: u64,
    /// The `mdat` boxes that directly follow the `moof`: (file offset, length).
    data: Vec<(u64, u64)>,
    seq_off: usize,
    track_id_off: usize,
    base_offset_off: Option<usize>,
    /// Decode time of the first sample, in the track's media timescale.
    time: u64,
    duration: u64,
    /// Composition offset of the first sample (B-frame delay), in media timescale.
    first_cto: u64,
}

impl Fragment {
    fn bytes(&self) -> u64 {
        self.moof.len() as u64 + self.data.iter().map(|d| d.1).sum::<u64>()
    }
}

struct Input {
    file: File,
    mvhd: Vec<u8>,
    movie_timescale: u32,
    trak: Vec<u8>,
    trex: Vec<u8>,
    media_timescale: u32,
    handler: [u8; 4],
    frags: Vec<Fragment>,
    duration: u64,
}

fn mvhd_timescale(b: &[u8], body: usize) -> Result<u32> {
    // Same layout for mvhd and mdhd: v1 has 64-bit creation/modification times.
    if b[body] == 1 { u32_at(b, body + 20) } else { u32_at(b, body + 12) }
}

fn open_input(path: &Path) -> Result<Input> {
    let mut file = File::open(path)?;
    let top = scan_top(&mut file)?;
    let moov_box = top.iter().find(|b| &b.typ == b"moov").ok_or_else(|| invalid("missing moov"))?;
    let moov = read_at(&mut file, moov_box.start, moov_box.size as usize)?;
    let moov_root = children(&moov, 0, moov.len())?;
    let mk = kids(&moov, moov_root[0])?;

    let mvhd = need(&mk, b"mvhd")?;
    let movie_timescale = mvhd_timescale(&moov, mvhd.body)?;
    let traks: Vec<_> = mk.iter().filter(|b| &b.typ == b"trak").collect();
    if traks.len() != 1 {
        return Err(unsupported(format!("expected one track, found {}", traks.len())));
    }
    let trak = *traks[0];
    let mvex = find(&mk, b"mvex").ok_or_else(|| unsupported("not a fragmented MP4 (no mvex)"))?;
    let trex = need(&kids(&moov, mvex)?, b"trex")?;

    let tk = kids(&moov, trak)?;
    let mdia = need(&tk, b"mdia")?;
    let md = kids(&moov, mdia)?;
    let mdhd = need(&md, b"mdhd")?;
    let media_timescale = mvhd_timescale(&moov, mdhd.body)?;
    let hdlr = need(&md, b"hdlr")?;
    let handler: [u8; 4] = moov
        .get(hdlr.body + 8..hdlr.body + 12)
        .ok_or_else(|| invalid("truncated hdlr"))?
        .try_into()
        .unwrap();
    if media_timescale == 0 || movie_timescale == 0 {
        return Err(invalid("zero timescale"));
    }
    let trex_default_duration = u32_at(&moov, trex.body + 12)?;

    let mut frags = Vec::new();
    let mut running = 0u64;
    let mut i = 0;
    while i < top.len() {
        if &top[i].typ != b"moof" {
            i += 1;
            continue;
        }
        let moof_start = top[i].start;
        let moof = read_at(&mut file, moof_start, top[i].size as usize)?;
        let mut data = Vec::new();
        i += 1;
        while i < top.len() && &top[i].typ == b"mdat" {
            data.push((top[i].start, top[i].size));
            i += 1;
        }
        if data.is_empty() {
            return Err(unsupported("moof without a following mdat"));
        }
        let mut frag = parse_moof(moof, moof_start, data, trex_default_duration)?;
        if frag.time == u64::MAX {
            frag.time = running;
        }
        running = frag.time + frag.duration;
        frags.push(frag);
    }
    if frags.is_empty() {
        return Err(unsupported("no movie fragments"));
    }
    let duration = running;

    Ok(Input {
        file,
        mvhd: moov[mvhd.start..mvhd.end].to_vec(),
        movie_timescale,
        trak: moov[trak.start..trak.end].to_vec(),
        trex: moov[trex.start..trex.end].to_vec(),
        media_timescale,
        handler,
        frags,
        duration,
    })
}

/// Read what we need from a `moof`. `time` is `u64::MAX` when there is no `tfdt`.
fn parse_moof(moof: Vec<u8>, moof_start: u64, data: Vec<(u64, u64)>, trex_dur: u32) -> Result<Fragment> {
    let root = children(&moof, 0, moof.len())?;
    let mk = kids(&moof, root[0])?;
    let mfhd = need(&mk, b"mfhd")?;
    let trafs: Vec<_> = mk.iter().filter(|b| &b.typ == b"traf").collect();
    if trafs.len() != 1 {
        return Err(unsupported(format!("expected one traf per moof, found {}", trafs.len())));
    }
    let tk = kids(&moof, *trafs[0])?;
    let tfhd = need(&tk, b"tfhd")?;
    let flags = u32_at(&moof, tfhd.body)? & 0x00FF_FFFF;
    let mut p = tfhd.body + 8;
    let mut base_offset_off = None;
    if flags & 0x1 != 0 {
        base_offset_off = Some(p);
        p += 8;
    }
    if flags & 0x2 != 0 {
        p += 4;
    }
    let default_dur = if flags & 0x8 != 0 { u32_at(&moof, p)? } else { trex_dur };

    let time = match find(&tk, b"tfdt") {
        Some(t) if moof[t.body] == 1 => u64_at(&moof, t.body + 4)?,
        Some(t) => u32_at(&moof, t.body + 4)? as u64,
        None => u64::MAX,
    };

    let mut duration = 0u64;
    let mut first_cto = None;
    for trun in tk.iter().filter(|b| &b.typ == b"trun") {
        let tf = u32_at(&moof, trun.body)? & 0x00FF_FFFF;
        let count = u32_at(&moof, trun.body + 4)? as usize;
        let mut q = trun.body + 8;
        if tf & 0x1 != 0 {
            q += 4;
        }
        if tf & 0x4 != 0 {
            q += 4;
        }
        let per_sample = [0x100, 0x200, 0x400, 0x800].iter().filter(|&&f| tf & f != 0).count() * 4;
        if first_cto.is_none() && count > 0 {
            let cto = if tf & 0x800 != 0 {
                let at = q + [0x100, 0x200, 0x400].iter().filter(|&&f| tf & f != 0).count() * 4;
                let raw = u32_at(&moof, at)?;
                // trun v1 offsets are signed; a negative one already starts at zero.
                if moof[trun.body] == 1 { (raw as i32).max(0) as u64 } else { raw as u64 }
            } else {
                0
            };
            first_cto = Some(cto);
        }
        if tf & 0x100 != 0 {
            for s in 0..count {
                duration += u32_at(&moof, q + s * per_sample)? as u64;
            }
        } else {
            duration += default_dur as u64 * count as u64;
        }
    }

    Ok(Fragment {
        seq_off: mfhd.body + 4,
        track_id_off: tfhd.body + 4,
        base_offset_off,
        time,
        duration,
        first_cto: first_cto.unwrap_or(0),
        moof,
        moof_start,
        data,
    })
}

fn make_box(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len() + 8);
    b.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    b.extend_from_slice(typ);
    b.extend_from_slice(body);
    b
}

fn rescale(v: u64, from: u32, to: u32) -> u64 {
    if from == to {
        return v;
    }
    (v as u128 * to as u128 / from.max(1) as u128) as u64
}

/// Copy of a `trak` with a new track ID and duration, and edit-list durations moved to
/// the output movie timescale. All edits are in place (same sizes).
fn patch_trak(inp: &Input, track_id: u32, movie_ts: u32) -> Result<Vec<u8>> {
    let mut t = inp.trak.clone();
    let root = children(&t, 0, t.len())?;
    let tk = kids(&t, root[0])?;
    let tkhd = need(&tk, b"tkhd")?;
    let dur = rescale(inp.duration, inp.media_timescale, movie_ts);
    if t[tkhd.body] == 1 {
        put_u32(&mut t, tkhd.body + 20, track_id);
        put_u64(&mut t, tkhd.body + 28, dur);
    } else {
        put_u32(&mut t, tkhd.body + 12, track_id);
        put_u32(&mut t, tkhd.body + 20, dur.min(u32::MAX as u64) as u32);
    }
    if let Some(edts) = find(&tk, b"edts") {
        if let Some(elst) = find(&kids(&t, edts)?, b"elst") {
            let v1 = t[elst.body] == 1;
            let n = u32_at(&t, elst.body + 4)? as usize;
            let entry = if v1 { 20 } else { 12 };
            for e in 0..n {
                let o = elst.body + 8 + e * entry;
                if v1 {
                    let d = rescale(u64_at(&t, o)?, inp.movie_timescale, movie_ts);
                    put_u64(&mut t, o, d);
                } else {
                    let d = rescale(u32_at(&t, o)? as u64, inp.movie_timescale, movie_ts);
                    put_u32(&mut t, o, d.min(u32::MAX as u64) as u32);
                }
            }
        }
    }
    let first_cto = inp.frags[0].first_cto;
    if find(&tk, b"edts").is_none() && first_cto > 0 {
        // Like FFmpeg: an edit list skips the B-frame delay so video starts at 0, in sync with audio.
        let mut elst = vec![1, 0, 0, 0];
        elst.extend_from_slice(&1u32.to_be_bytes());
        elst.extend_from_slice(&dur.to_be_bytes());
        elst.extend_from_slice(&first_cto.to_be_bytes());
        elst.extend_from_slice(&[0, 1, 0, 0]);
        let edts = make_box(b"edts", &make_box(b"elst", &elst));
        let mut body = Vec::with_capacity(t.len() + edts.len());
        for c in &tk {
            body.extend_from_slice(&t[c.start..c.end]);
            if &c.typ == b"tkhd" {
                body.extend_from_slice(&edts);
            }
        }
        return Ok(make_box(b"trak", &body));
    }
    Ok(t)
}

fn build_moov(video: &Input, audio: &Input) -> Result<Vec<u8>> {
    let movie_ts = video.movie_timescale;
    let movie_dur = rescale(video.duration, video.media_timescale, movie_ts)
        .max(rescale(audio.duration, audio.media_timescale, movie_ts));

    let mut mvhd = video.mvhd.clone();
    let mroot = children(&mvhd, 0, mvhd.len())?[0];
    if mvhd[mroot.body] == 1 {
        put_u64(&mut mvhd, mroot.body + 24, movie_dur);
    } else {
        put_u32(&mut mvhd, mroot.body + 16, movie_dur.min(u32::MAX as u64) as u32);
    }
    let next_id_off = mvhd.len() - 4;
    put_u32(&mut mvhd, next_id_off, AUDIO_ID + 1);

    let mut mvex = Vec::new();
    let mut mehd = vec![1, 0, 0, 0];
    mehd.extend_from_slice(&movie_dur.to_be_bytes());
    mvex.extend(make_box(b"mehd", &mehd));
    for (inp, id) in [(video, VIDEO_ID), (audio, AUDIO_ID)] {
        let mut trex = inp.trex.clone();
        let body = children(&trex, 0, trex.len())?[0].body;
        put_u32(&mut trex, body + 4, id);
        mvex.extend(trex);
    }

    let mut body = mvhd;
    body.extend(patch_trak(video, VIDEO_ID, movie_ts)?);
    body.extend(patch_trak(audio, AUDIO_ID, movie_ts)?);
    body.extend(make_box(b"mvex", &mvex));
    Ok(make_box(b"moov", &body))
}

fn ftyp() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"isom");
    body.extend_from_slice(&512u32.to_be_bytes());
    for brand in [b"isom", b"iso6", b"iso2", b"mp41"] {
        body.extend_from_slice(brand);
    }
    make_box(b"ftyp", &body)
}

/// `mfra` with one `tfra` per track (every fragment start is a random access point,
/// as DASH segments begin with a keyframe), then `mfro`.
fn build_mfra(index: &[(u32, u64, u64)]) -> Vec<u8> {
    let mut body = Vec::new();
    for id in [VIDEO_ID, AUDIO_ID] {
        let entries: Vec<_> = index.iter().filter(|e| e.0 == id).collect();
        let mut t = vec![1, 0, 0, 0];
        t.extend_from_slice(&id.to_be_bytes());
        t.extend_from_slice(&0u32.to_be_bytes()); // 1-byte traf/trun/sample numbers
        t.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (_, time, offset) in entries {
            t.extend_from_slice(&time.to_be_bytes());
            t.extend_from_slice(&offset.to_be_bytes());
            t.extend_from_slice(&[1, 1, 1]);
        }
        body.extend(make_box(b"tfra", &t));
    }
    let mfra_size = (body.len() + 8 + 16) as u32;
    let mut mfro = vec![0, 0, 0, 0];
    mfro.extend_from_slice(&mfra_size.to_be_bytes());
    body.extend(make_box(b"mfro", &mfro));
    make_box(b"mfra", &body)
}

pub fn mux(a: &Path, b: &Path, out: &Path, progress: Progress) -> Result<()> {
    let first = open_input(a)?;
    let second = open_input(b)?;
    let (mut video, mut audio) = match (&first.handler, &second.handler) {
        (b"vide", b"soun") => (first, second),
        (b"soun", b"vide") => (second, first),
        (x, y) => {
            return Err(unsupported(format!(
                "need one video and one audio track, got {} and {}",
                String::from_utf8_lossy(x),
                String::from_utf8_lossy(y)
            )))
        }
    };

    // Interleave fragments by decode time (compared across timescales without rounding).
    let mut order = Vec::with_capacity(video.frags.len() + audio.frags.len());
    let (mut vi, mut ai) = (0, 0);
    while vi < video.frags.len() || ai < audio.frags.len() {
        let take_video = match (video.frags.get(vi), audio.frags.get(ai)) {
            (Some(v), Some(s)) => {
                v.time as u128 * audio.media_timescale as u128 <= s.time as u128 * video.media_timescale as u128
            }
            (Some(_), None) => true,
            _ => false,
        };
        if take_video {
            order.push((true, vi));
            vi += 1;
        } else {
            order.push((false, ai));
            ai += 1;
        }
    }

    let total: u64 = video.frags.iter().chain(audio.frags.iter()).map(Fragment::bytes).sum();
    let mut done = 0u64;
    let mut w = BufWriter::with_capacity(1 << 20, File::create(out)?);
    let head = [ftyp(), build_moov(&video, &audio)?].concat();
    w.write_all(&head)?;
    let mut pos = head.len() as u64;
    let mut index = Vec::new();

    for (seq, (is_video, i)) in order.into_iter().enumerate() {
        let (inp, id) = if is_video { (&mut video, VIDEO_ID) } else { (&mut audio, AUDIO_ID) };
        let frag = &inp.frags[i];
        let mut moof = frag.moof.clone();
        put_u32(&mut moof, frag.seq_off, seq as u32 + 1);
        put_u32(&mut moof, frag.track_id_off, id);
        if let Some(o) = frag.base_offset_off {
            // Absolute file offset: move it with the fragment.
            let old = u64_at(&moof, o)?;
            let rel = old
                .checked_sub(frag.moof_start)
                .ok_or_else(|| unsupported("base data offset before its moof"))?;
            put_u64(&mut moof, o, pos + rel);
        }
        index.push((id, frag.time, pos));
        w.write_all(&moof)?;
        pos += moof.len() as u64;
        done += moof.len() as u64;
        let ranges = frag.data.clone();
        for (start, len) in ranges {
            copy_range(&mut inp.file, start, len, &mut w, &mut done, total, progress)?;
            pos += len;
        }
    }

    w.write_all(&build_mfra(&index))?;
    w.flush()?;
    w.into_inner().map_err(|e| MuxError::Io(e.into_error()))?.sync_all()?;
    progress(total, total);
    Ok(())
}
