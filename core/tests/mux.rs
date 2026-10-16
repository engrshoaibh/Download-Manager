//! Built-in muxer against small DASH-style fixtures. When FFmpeg is installed the
//! output is also probed and fully decoded to prove players will accept it.

use std::path::{Path, PathBuf};
use std::process::Command;

use dm_core::mux::{mux_av, Container, MuxError};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mux").join(name)
}

fn out_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("dm-mux-test");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn have_ffmpeg() -> bool {
    Command::new("ffprobe").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// (codec names in stream order, container duration in seconds)
fn probe(path: &Path) -> (Vec<String>, f64) {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_name:format=duration", "-of", "json"])
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "ffprobe failed: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let codecs = v["streams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["codec_name"].as_str().unwrap_or("").to_string())
        .collect();
    let dur = v["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    (codecs, dur)
}

/// Both streams start together (B-frame delay hidden by an edit list), so A/V stay in sync.
fn assert_starts_at_zero(path: &Path) {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=start_time", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let t: f64 = line.trim().parse().unwrap_or(f64::NAN);
        assert!(t.abs() < 0.001, "stream starts at {line}");
    }
}

/// Decode every frame; any error output means a broken file.
fn decodes_cleanly(path: &Path) {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && err.trim().is_empty(), "decode errors: {err}");
}

fn no_progress() -> impl FnMut(u64, u64) -> bool {
    |_, _| true
}

#[test]
fn muxes_fragmented_mp4_in_either_order() {
    for (a, b, name) in [("video.mp4", "audio.m4a", "va.mp4"), ("audio.m4a", "video.mp4", "av.mp4")] {
        let out = out_path(name);
        let mut last = (0, 0);
        let mut progress = |d, t| {
            last = (d, t);
            true
        };
        let kind = mux_av(&fixture(a), &fixture(b), &out, &mut progress).unwrap();
        assert_eq!(kind, Container::Mp4);
        assert!(last.1 > 0 && last.0 == last.1, "progress should finish at 100%: {last:?}");
        if have_ffmpeg() {
            let (codecs, dur) = probe(&out);
            assert_eq!(codecs, ["h264", "aac"]);
            assert!((dur - 6.0).abs() < 0.2, "duration {dur}");
            assert_starts_at_zero(&out);
            decodes_cleanly(&out);
        }
    }
}

#[test]
fn muxes_webm() {
    let out = out_path("va.webm");
    let kind = mux_av(&fixture("audio.webm"), &fixture("video.webm"), &out, &mut no_progress()).unwrap();
    assert_eq!(kind, Container::Webm);
    if have_ffmpeg() {
        let (codecs, dur) = probe(&out);
        assert_eq!(codecs, ["vp9", "opus"]);
        assert!((dur - 6.0).abs() < 0.2, "duration {dur}");
        decodes_cleanly(&out);
    }
}

#[test]
fn refuses_what_it_cannot_mux() {
    let out = out_path("refused.mp4");
    let plain = mux_av(&fixture("plain.mp4"), &fixture("audio.m4a"), &out, &mut no_progress());
    assert!(matches!(plain, Err(MuxError::Unsupported(_))), "{plain:?}");
    let mixed = mux_av(&fixture("video.mp4"), &fixture("audio.webm"), &out, &mut no_progress());
    assert!(matches!(mixed, Err(MuxError::Unsupported(_))), "{mixed:?}");
    assert!(!out.exists());
}

#[test]
fn cancel_removes_partial_output() {
    let out = out_path("cancelled.mp4");
    let res = mux_av(&fixture("video.mp4"), &fixture("audio.m4a"), &out, &mut |_, _| false);
    assert!(matches!(res, Err(MuxError::Cancelled)), "{res:?}");
    assert!(!out.exists());
}
