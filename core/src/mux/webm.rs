//! WebM/Matroska video + audio (one track each, e.g. YouTube VP9/AV1 + Opus) → one file.
//!
//! The output is a fresh Segment: SeekHead, Info, Tracks (video = 1, audio = 2), the
//! Clusters of both inputs interleaved by timecode (with the track number in each block
//! rewritten), then Cues for the video clusters so players can seek. Frames are copied as is.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use super::{invalid, read_at, unsupported, MuxError, Progress, Result};

const ID_EBML: u32 = 0x1A45_DFA3;
const ID_SEGMENT: u32 = 0x1853_8067;
const ID_SEEKHEAD: u32 = 0x114D_9B74;
const ID_SEEK: u32 = 0x4DBB;
const ID_SEEKID: u32 = 0x53AB;
const ID_SEEKPOSITION: u32 = 0x53AC;
const ID_INFO: u32 = 0x1549_A966;
const ID_TIMECODESCALE: u32 = 0x2A_D7B1;
const ID_DURATION: u32 = 0x4489;
const ID_MUXINGAPP: u32 = 0x4D80;
const ID_WRITINGAPP: u32 = 0x5741;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_TRACKENTRY: u32 = 0xAE;
const ID_TRACKNUMBER: u32 = 0xD7;
const ID_TRACKUID: u32 = 0x73C5;
const ID_TRACKTYPE: u32 = 0x83;
const ID_CODECID: u32 = 0x86;
const ID_CLUSTER: u32 = 0x1F43_B675;
const ID_TIMECODE: u32 = 0xE7;
const ID_SIMPLEBLOCK: u32 = 0xA3;
const ID_BLOCKGROUP: u32 = 0xA0;
const ID_BLOCK: u32 = 0xA1;
const ID_POSITION: u32 = 0xA7;
const ID_PREVSIZE: u32 = 0xAB;
const ID_CRC32: u32 = 0xBF;
const ID_VOID: u32 = 0xEC;
const ID_CUES: u32 = 0x1C53_BB6B;
const ID_CUEPOINT: u32 = 0xBB;
const ID_CUETIME: u32 = 0xB3;
const ID_CUETRACKPOSITIONS: u32 = 0xB7;
const ID_CUETRACK: u32 = 0xF7;
const ID_CUECLUSTERPOSITION: u32 = 0xF1;

const VIDEO_TRACK: u64 = 1;
const AUDIO_TRACK: u64 = 2;

// ── EBML primitives ───────────────────────────────────────────────────────────

/// Element ID (marker bits kept) and its length in bytes.
fn read_id(b: &[u8], o: usize) -> Result<(u32, usize)> {
    let first = *b.get(o).ok_or_else(|| invalid("truncated EBML id"))?;
    let len = first.leading_zeros() as usize + 1;
    if first == 0 || len > 4 || o + len > b.len() {
        return Err(invalid("bad EBML id"));
    }
    Ok((b[o..o + len].iter().fold(0u32, |v, &x| (v << 8) | x as u32), len))
}

/// Data size (`None` = unknown) and its length in bytes.
fn read_size(b: &[u8], o: usize) -> Result<(Option<u64>, usize)> {
    let first = *b.get(o).ok_or_else(|| invalid("truncated EBML size"))?;
    let len = first.leading_zeros() as usize + 1;
    if first == 0 || len > 8 || o + len > b.len() {
        return Err(invalid("bad EBML size"));
    }
    let mut v = (first & (0xFFu16 >> len) as u8) as u64;
    for &x in &b[o + 1..o + len] {
        v = (v << 8) | x as u64;
    }
    let all_ones = (1u64 << (7 * len)) - 1;
    Ok((if v == all_ones { None } else { Some(v) }, len))
}

/// An element in a buffer: `start..end` whole element, `data..end` its payload.
#[derive(Clone, Copy, Debug)]
struct Elem {
    id: u32,
    start: usize,
    data: usize,
    end: usize,
}

fn elems(b: &[u8], from: usize, to: usize) -> Result<Vec<Elem>> {
    let mut out = Vec::new();
    let mut o = from;
    while o < to {
        let (id, il) = read_id(b, o)?;
        let (size, sl) = read_size(b, o + il)?;
        let size = size.ok_or_else(|| unsupported("unknown-size element inside a cluster or header"))?;
        let data = o + il + sl;
        let end = data
            .checked_add(size as usize)
            .filter(|&e| e <= to)
            .ok_or_else(|| invalid("EBML element runs past its parent"))?;
        out.push(Elem { id, start: o, data, end });
        o = end;
    }
    Ok(out)
}

fn read_uint(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |v, &x| (v << 8) | x as u64)
}

fn read_float(b: &[u8]) -> Option<f64> {
    match b.len() {
        4 => Some(f32::from_be_bytes(b.try_into().ok()?) as f64),
        8 => Some(f64::from_be_bytes(b.try_into().ok()?)),
        _ => None,
    }
}

fn put_id(out: &mut Vec<u8>, id: u32) {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().take_while(|&&x| x == 0).count();
    out.extend_from_slice(&bytes[skip..]);
}

/// Shortest size vint for `n` (all-ones values are reserved for "unknown").
fn put_size(out: &mut Vec<u8>, n: u64) {
    let len = (1..=8).find(|&l| n < (1u64 << (7 * l)) - 1).unwrap_or(8);
    put_size_len(out, n, len);
}

fn put_size_len(out: &mut Vec<u8>, n: u64, len: usize) {
    let v = n | (1u64 << (7 * len));
    out.extend_from_slice(&v.to_be_bytes()[8 - len..]);
}

fn put_elem(out: &mut Vec<u8>, id: u32, data: &[u8]) {
    put_id(out, id);
    put_size(out, data.len() as u64);
    out.extend_from_slice(data);
}

fn uint_bytes(v: u64) -> Vec<u8> {
    let b = v.to_be_bytes();
    let skip = b.iter().take_while(|&&x| x == 0).count().min(7);
    b[skip..].to_vec()
}

fn put_uint(out: &mut Vec<u8>, id: u32, v: u64) {
    put_elem(out, id, &uint_bytes(v));
}

// ── Input ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct ClusterRef {
    start: u64,
    len: u64,
    timecode: u64,
}

struct Input {
    file: File,
    timecode_scale: u64,
    duration: Option<f64>,
    track_entry: Vec<u8>,
    track_number: u64,
    track_type: u64,
    track_uid: Option<u64>,
    codec: String,
    clusters: Vec<ClusterRef>,
}

fn open_input(path: &Path) -> Result<Input> {
    let mut file = File::open(path)?;
    let len = file.seek(SeekFrom::End(0))?;

    let head = read_at(&mut file, 0, len.min(64) as usize)?;
    let (id, il) = read_id(&head, 0)?;
    if id != ID_EBML {
        return Err(invalid("not an EBML file"));
    }
    let (size, sl) = read_size(&head, il)?;
    let mut o = (il + sl) as u64 + size.ok_or_else(|| invalid("unknown-size EBML header"))?;

    // Segment header
    let seg_head = read_at(&mut file, o, (len - o).min(16) as usize)?;
    let (id, il) = read_id(&seg_head, 0)?;
    if id != ID_SEGMENT {
        return Err(invalid("missing Segment"));
    }
    let (seg_size, sl) = read_size(&seg_head, il)?;
    o += (il + sl) as u64;
    let seg_end = seg_size.map(|s| (o + s).min(len)).unwrap_or(len);

    let mut info = None;
    let mut tracks = None;
    let mut clusters = Vec::new();
    while o < seg_end {
        let h = read_at(&mut file, o, (seg_end - o).min(16) as usize)?;
        let (id, il) = read_id(&h, 0)?;
        let (size, sl) = read_size(&h, il)?;
        let hdr = (il + sl) as u64;
        let Some(size) = size else {
            return Err(unsupported("unknown-size element in Segment (live WebM)"));
        };
        let total = hdr + size;
        if o + total > seg_end {
            return Err(invalid("element runs past the Segment"));
        }
        match id {
            ID_INFO => info = Some(read_at(&mut file, o + hdr, size as usize)?),
            ID_TRACKS => tracks = Some(read_at(&mut file, o + hdr, size as usize)?),
            ID_CLUSTER => {
                // Only the first child is needed: the cluster Timecode comes first.
                let first = read_at(&mut file, o + hdr, size.min(32) as usize)?;
                let timecode = elems_prefix(&first)
                    .into_iter()
                    .find(|e| e.id == ID_TIMECODE)
                    .map(|e| read_uint(&first[e.data..e.end]))
                    .ok_or_else(|| unsupported("cluster without a leading Timecode"))?;
                clusters.push(ClusterRef { start: o, len: total, timecode });
            }
            _ => {}
        }
        o += total;
    }

    let info = info.ok_or_else(|| invalid("missing Info"))?;
    let mut timecode_scale = 1_000_000;
    let mut duration = None;
    for e in elems(&info, 0, info.len())? {
        match e.id {
            ID_TIMECODESCALE => timecode_scale = read_uint(&info[e.data..e.end]),
            ID_DURATION => duration = read_float(&info[e.data..e.end]),
            _ => {}
        }
    }

    let tracks = tracks.ok_or_else(|| invalid("missing Tracks"))?;
    let entries: Vec<_> = elems(&tracks, 0, tracks.len())?
        .into_iter()
        .filter(|e| e.id == ID_TRACKENTRY)
        .collect();
    if entries.len() != 1 {
        return Err(unsupported(format!("expected one track, found {}", entries.len())));
    }
    let entry = entries[0];
    let (mut track_number, mut track_type, mut track_uid, mut codec) = (0, 0, None, String::new());
    for e in elems(&tracks, entry.data, entry.end)? {
        let v = &tracks[e.data..e.end];
        match e.id {
            ID_TRACKNUMBER => track_number = read_uint(v),
            ID_TRACKTYPE => track_type = read_uint(v),
            ID_TRACKUID => track_uid = Some(read_uint(v)),
            ID_CODECID => codec = String::from_utf8_lossy(v).trim_end_matches('\0').to_string(),
            _ => {}
        }
    }
    if clusters.is_empty() {
        return Err(invalid("no clusters"));
    }

    Ok(Input {
        file,
        timecode_scale,
        duration,
        track_entry: tracks[entry.data..entry.end].to_vec(),
        track_number,
        track_type,
        track_uid,
        codec,
        clusters,
    })
}

/// Parse as many whole elements as fit in a short prefix buffer.
fn elems_prefix(b: &[u8]) -> Vec<Elem> {
    let mut out = Vec::new();
    let mut o = 0;
    while let Ok((id, il)) = read_id(b, o) {
        let Ok((Some(size), sl)) = read_size(b, o + il) else { break };
        let end = o + il + sl + size as usize;
        if end > b.len() {
            break;
        }
        out.push(Elem { id, start: o, data: o + il + sl, end });
        o = end;
    }
    out
}

// ── Rewriting ─────────────────────────────────────────────────────────────────

/// Rewrite the track number at the start of a Block/SimpleBlock payload, keeping its width.
fn patch_block_track(block: &mut [u8], expect: u64, new: u64) -> Result<()> {
    let (num, len) = read_size(block, 0)?;
    if num != Some(expect) {
        return Err(unsupported("block for an unexpected track"));
    }
    let mut v = Vec::with_capacity(len);
    put_size_len(&mut v, new, len);
    block[..len].copy_from_slice(&v);
    Ok(())
}

/// Cluster payload with blocks moved to `new_track`; position/size hints and CRCs dropped.
fn rewrite_cluster(buf: &[u8], from: usize, old_track: u64, new_track: u64) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(buf.len() - from);
    for e in elems(buf, from, buf.len())? {
        match e.id {
            ID_POSITION | ID_PREVSIZE | ID_CRC32 | ID_VOID => {}
            ID_SIMPLEBLOCK => {
                let mut el = buf[e.start..e.end].to_vec();
                patch_block_track(&mut el[e.data - e.start..], old_track, new_track)?;
                out.extend(el);
            }
            ID_BLOCKGROUP => {
                let mut el = buf[e.start..e.end].to_vec();
                for c in elems(buf, e.data, e.end)? {
                    if c.id == ID_BLOCK {
                        patch_block_track(&mut el[c.data - e.start..], old_track, new_track)?;
                    }
                }
                out.extend(el);
            }
            _ => out.extend_from_slice(&buf[e.start..e.end]),
        }
    }
    Ok(out)
}

fn read_cluster(inp: &mut Input, c: &ClusterRef) -> Result<(Vec<u8>, usize)> {
    let buf = read_at(&mut inp.file, c.start, c.len as usize)?;
    let (_, il) = read_id(&buf, 0)?;
    let (_, sl) = read_size(&buf, il)?;
    Ok((buf, il + sl))
}

fn track_entry(inp: &Input, number: u64, uid: Option<u64>) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    for e in elems(&inp.track_entry, 0, inp.track_entry.len())? {
        match e.id {
            ID_TRACKNUMBER => put_uint(&mut body, ID_TRACKNUMBER, number),
            ID_TRACKUID if uid.is_some() => put_uint(&mut body, ID_TRACKUID, uid.unwrap()),
            ID_CRC32 | ID_VOID => {}
            _ => body.extend_from_slice(&inp.track_entry[e.start..e.end]),
        }
    }
    let mut out = Vec::new();
    put_elem(&mut out, ID_TRACKENTRY, &body);
    Ok(out)
}

fn is_webm_codec(c: &str) -> bool {
    matches!(c, "V_VP8" | "V_VP9" | "V_AV1" | "A_OPUS" | "A_VORBIS")
}

pub fn mux(a: &Path, b: &Path, out: &Path, progress: Progress) -> Result<()> {
    let first = open_input(a)?;
    let second = open_input(b)?;
    let (mut video, mut audio) = match (first.track_type, second.track_type) {
        (1, 2) => (first, second),
        (2, 1) => (second, first),
        (x, y) => return Err(unsupported(format!("need one video and one audio track, got types {x} and {y}"))),
    };
    if video.timecode_scale != audio.timecode_scale {
        return Err(unsupported("inputs use different timecode scales"));
    }

    // Header elements (sizes known up front).
    let mut info = Vec::new();
    put_uint(&mut info, ID_TIMECODESCALE, video.timecode_scale);
    let duration = match (video.duration, audio.duration) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    };
    if let Some(d) = duration {
        put_elem(&mut info, ID_DURATION, &d.to_be_bytes());
    }
    put_elem(&mut info, ID_MUXINGAPP, b"Download Manager Pro");
    put_elem(&mut info, ID_WRITINGAPP, b"Download Manager Pro");
    let mut info_el = Vec::new();
    put_elem(&mut info_el, ID_INFO, &info);

    let audio_uid = match (video.track_uid, audio.track_uid) {
        (Some(v), Some(s)) if v == s => Some(s.wrapping_add(1).max(1)),
        _ => None,
    };
    let mut tracks = track_entry(&video, VIDEO_TRACK, None)?;
    tracks.extend(track_entry(&audio, AUDIO_TRACK, audio_uid)?);
    let mut tracks_el = Vec::new();
    put_elem(&mut tracks_el, ID_TRACKS, &tracks);

    // SeekHead with fixed-width positions so its size doesn't depend on them.
    let seek_entry_len = |pos_ids: &[(u32, u64)]| -> Vec<u8> {
        let mut sh = Vec::new();
        for &(id, pos) in pos_ids {
            let mut s = Vec::new();
            put_elem(&mut s, ID_SEEKID, &id.to_be_bytes());
            put_elem(&mut s, ID_SEEKPOSITION, &pos.to_be_bytes());
            put_elem(&mut sh, ID_SEEK, &s);
        }
        let mut el = Vec::new();
        put_elem(&mut el, ID_SEEKHEAD, &sh);
        el
    };
    let seekhead_len = seek_entry_len(&[(ID_INFO, 0), (ID_TRACKS, 0), (ID_CUES, 0)]).len() as u64;

    // Interleave clusters by timecode (video first on ties).
    let mut order: Vec<(bool, usize, u64)> = video
        .clusters
        .iter()
        .enumerate()
        .map(|(i, c)| (true, i, c.timecode))
        .chain(audio.clusters.iter().enumerate().map(|(i, c)| (false, i, c.timecode)))
        .collect();
    order.sort_by_key(|&(is_video, i, tc)| (tc, !is_video, i));

    // Pass 1: rewritten cluster sizes → positions for Cues.
    let clusters_start = seekhead_len + info_el.len() as u64 + tracks_el.len() as u64;
    let mut pos = clusters_start;
    let mut cues = Vec::new();
    let mut total = 0u64;
    let (vt, at) = (video.track_number, audio.track_number);
    for &(is_video, i, tc) in &order {
        let (inp, old) = if is_video { (&mut video, vt) } else { (&mut audio, at) };
        let c = inp.clusters[i];
        let (buf, hdr) = read_cluster(inp, &c)?;
        let body_len = rewrite_cluster(&buf, hdr, old, if is_video { VIDEO_TRACK } else { AUDIO_TRACK })?.len() as u64;
        if is_video {
            let mut tp = Vec::new();
            put_uint(&mut tp, ID_CUETRACK, VIDEO_TRACK);
            put_uint(&mut tp, ID_CUECLUSTERPOSITION, pos);
            let mut cp = Vec::new();
            put_uint(&mut cp, ID_CUETIME, tc);
            put_elem(&mut cp, ID_CUETRACKPOSITIONS, &tp);
            put_elem(&mut cues, ID_CUEPOINT, &cp);
        }
        let el_len = 4 + 8 + body_len;
        pos += el_len;
        total += el_len;
    }
    let cues_pos = pos;
    let mut cues_el = Vec::new();
    put_elem(&mut cues_el, ID_CUES, &cues);
    let seekhead = seek_entry_len(&[
        (ID_INFO, seekhead_len),
        (ID_TRACKS, seekhead_len + info_el.len() as u64),
        (ID_CUES, cues_pos),
    ]);
    let segment_size = cues_pos + cues_el.len() as u64;

    // Write.
    let mut w = BufWriter::with_capacity(1 << 20, File::create(out)?);
    let doctype: &[u8] = if is_webm_codec(&video.codec) && is_webm_codec(&audio.codec) { b"webm" } else { b"matroska" };
    let mut ebml = Vec::new();
    put_uint(&mut ebml, 0x4286, 1); // EBMLVersion
    put_uint(&mut ebml, 0x42F7, 1); // EBMLReadVersion
    put_uint(&mut ebml, 0x42F2, 4); // EBMLMaxIDLength
    put_uint(&mut ebml, 0x42F3, 8); // EBMLMaxSizeLength
    put_elem(&mut ebml, 0x4282, doctype); // DocType
    put_uint(&mut ebml, 0x4287, 4); // DocTypeVersion
    put_uint(&mut ebml, 0x4285, 2); // DocTypeReadVersion
    let mut head = Vec::new();
    put_elem(&mut head, ID_EBML, &ebml);
    put_id(&mut head, ID_SEGMENT);
    put_size_len(&mut head, segment_size, 8);
    head.extend(seekhead);
    head.extend(info_el);
    head.extend(tracks_el);
    w.write_all(&head)?;

    let mut done = 0u64;
    for &(is_video, i, _) in &order {
        let (inp, old) = if is_video { (&mut video, vt) } else { (&mut audio, at) };
        let c = inp.clusters[i];
        let (buf, hdr) = read_cluster(inp, &c)?;
        let body = rewrite_cluster(&buf, hdr, old, if is_video { VIDEO_TRACK } else { AUDIO_TRACK })?;
        let mut h = Vec::with_capacity(12);
        put_id(&mut h, ID_CLUSTER);
        put_size_len(&mut h, body.len() as u64, 8);
        w.write_all(&h)?;
        w.write_all(&body)?;
        done += (h.len() + body.len()) as u64;
        if !progress(done, total) {
            return Err(MuxError::Cancelled);
        }
    }
    w.write_all(&cues_el)?;
    w.flush()?;
    w.into_inner().map_err(|e| MuxError::Io(e.into_error()))?.sync_all()?;
    Ok(())
}
