//! Built-in muxer: joins a separate video stream and audio stream into one file by copying
//! their packets, never re-encoding. Covers what YouTube/DASH serves:
//! fragmented MP4 + fragmented MP4 (H.264/AV1 + AAC) and WebM + WebM (VP9/AV1 + Opus).
//!
//! Anything else returns [`MuxError::Unsupported`] so callers can fall back to FFmpeg.

pub mod mp4;
pub mod webm;

use std::io::Read;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum MuxError {
    /// The inputs are valid but not a combination this muxer handles; use FFmpeg instead.
    #[error("unsupported input: {0}")]
    Unsupported(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, MuxError>;

/// Progress callback: `(bytes_done, bytes_total)`. Return `false` to cancel.
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64) -> bool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Mp4,
    Webm,
}

/// Sniff the container from the first bytes of a file.
pub fn detect(path: &Path) -> std::io::Result<Option<Container>> {
    let mut head = [0u8; 12];
    let mut f = std::fs::File::open(path)?;
    let n = f.read(&mut head)?;
    if n >= 8 && &head[4..8] == b"ftyp" {
        return Ok(Some(Container::Mp4));
    }
    if n >= 4 && head[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Ok(Some(Container::Webm));
    }
    Ok(None)
}

/// Mux one video file and one audio file (in either order) into `out`.
/// On error `out` is removed. Returns the container that was written.
pub fn mux_av(a: &Path, b: &Path, out: &Path, progress: Progress) -> Result<Container> {
    let kind = match (detect(a)?, detect(b)?) {
        (Some(Container::Mp4), Some(Container::Mp4)) => Container::Mp4,
        (Some(Container::Webm), Some(Container::Webm)) => Container::Webm,
        (x, y) => return Err(MuxError::Unsupported(format!("cannot mux {x:?} with {y:?}"))),
    };
    let res = match kind {
        Container::Mp4 => mp4::mux(a, b, out, progress),
        Container::Webm => webm::mux(a, b, out, progress),
    };
    if res.is_err() {
        let _ = std::fs::remove_file(out);
    }
    res.map(|_| kind)
}

pub(crate) fn invalid(msg: impl Into<String>) -> MuxError {
    MuxError::Invalid(msg.into())
}

pub(crate) fn unsupported(msg: impl Into<String>) -> MuxError {
    MuxError::Unsupported(msg.into())
}

/// Copy `len` bytes from `src` at `pos` into `dst`, reporting progress.
pub(crate) fn copy_range(
    src: &mut std::fs::File,
    pos: u64,
    len: u64,
    dst: &mut impl std::io::Write,
    done: &mut u64,
    total: u64,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<()> {
    use std::io::{Seek, SeekFrom};
    src.seek(SeekFrom::Start(pos))?;
    let mut left = len;
    let mut buf = vec![0u8; 1 << 20];
    while left > 0 {
        let n = (left as usize).min(buf.len());
        src.read_exact(&mut buf[..n])?;
        dst.write_all(&buf[..n])?;
        left -= n as u64;
        *done += n as u64;
        if !progress(*done, total) {
            return Err(MuxError::Cancelled);
        }
    }
    Ok(())
}

pub(crate) fn read_at(f: &mut std::fs::File, pos: u64, len: usize) -> Result<Vec<u8>> {
    use std::io::{Seek, SeekFrom};
    // Header-sized structures only; refuse absurd sizes from corrupt files.
    if len > 256 << 20 {
        return Err(invalid(format!("metadata block of {len} bytes is too large")));
    }
    f.seek(SeekFrom::Start(pos))?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf)?;
    Ok(buf)
}
