# IDM parity plan

What the Internet Download Manager 6.43 install folder tells us, and what we build next.

Findings come from file metadata only: file names, version info, PE import/export tables,
the native-messaging manifests, the extension manifest and the registry keys the installer
wrote. No IDM code was disassembled, and nothing from IDM (binaries, extension code,
extension IDs) is reused. We copy the architecture, not the implementation.

## 1. What IDM installs

| IDM file | What it is (from exports/metadata) | Our equivalent today |
|---|---|---|
| `IDMan.exe` (6 MB, x86) | The whole app: UI, engine, scheduler. Imports only Windows system DLLs. | `dm-gui.exe` |
| `libssl.dll`, `libcrypto.dll` | OpenSSL 1.1.1 for HTTPS | `rustls`, compiled in |
| `idmvconv.dll` (835 KB) | **Its own muxer, no ffmpeg.** Exports `MuxMP4AndMKVToMKV`, `MuxMKVAndMKVToMKV`, `MuxTSandTStoMP4`, `MuxTSandTStoMKV`, `ConvertTsToMp4`, `ConvertTsAndAACToMp4`, `ConvertFlvToMp4`, `NormalizeWebmAudioStream`, all with progress callbacks. Imports only kernel32/user32. | ffmpeg (downloaded at runtime, ~100 MB) |
| `IDMVMPrs*.dll` | Stream manifest parser: libxml2 plus `GetMPDParserInfo` and `Parse1` (DASH MPD / HTML parsing) | yt-dlp |
| `IDMMsgHost.exe` + `IDMMsgHost.json` | Native messaging host `com.tonec.idm`, registered for Chrome, Edge and Firefox | `dm-native-host.exe` exists but is **not registered** and the extension doesn't use it |
| `idmnmcl.dll` | Native-messaging client inside the app: `GetCks` (cookies from the browser), `SendDwnlResult`, `GetActiveOnWakeup` | Extension posts to `http://127.0.0.1:7373` |
| `IDMGCExt.crx`, `IDMEdgeExt.crx`, `idmmzcc*.xpi` | Browser extensions (MV3: `webRequest`, `declarativeNetRequest`, `downloads`, `cookies`, `nativeMessaging`, `<all_urls>`) | `extension/` (MV3, no `nativeMessaging`) |
| `idmmkb.dll` | Keyboard/mouse hooks: `GetAltState`, `GetCtrlState`, `NeedForce`, `NeedPrevent` (hold Alt to skip capture, Insert to force it) | none |
| `IDMNetMon*.dll`, `idmwfp*.sys`, `idmtdi*.sys` | Network monitor and kernel drivers (WFP/TDI) that catch downloads from any app, not just browsers | none (we skip this, see section 4) |
| `idmindex.dll` | SQLite (download list index) | `sled` |
| `IDMShellExt*.dll`, `IDMIECC*.dll`, `downlWithIDM*.dll`, `IEMonitor.exe` | Explorer menu and legacy Internet Explorer integration | none (skip) |
| `idmvs.dll` | Version info for the app and its extensions | `Cargo.toml` version |
| `Languages\*.lng` | Translations | none |

### Registry and startup

- `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` → `IDMan.exe /onboot`: it starts at sign-in **hidden in the tray**.
- `HKCU`/`HKLM\Software\Google\Chrome\NativeMessagingHosts\com.tonec.idm`, and the same under Edge and Mozilla, point to `IDMMsgHost.json`.
- `Software\Google\Chrome\Extensions\<id>` → `path=IDMGCExt.crx`, `version=6.43.1`: the installer pushes the extension into Chrome. Chrome only accepts this for an extension ID that is published on the Chrome Web Store.
- Settings live in `HKCU\Software\DownloadManager`.

## 2. The key lessons

1. **No ffmpeg and no yt-dlp.** IDM merges video and audio with a small built-in muxer (`idmvconv.dll`) that only copies data and never re-encodes. That is why it works right after install and offline.
2. **The browser does the YouTube work.** The extension sniffs the real `googlevideo.com` stream URLs and cookies, then IDM downloads them as normal HTTP files and merges them. Our extension already sniffs these URLs (`extension/shared/media.js`) but then hands the job to yt-dlp pinned to the itag.
3. **The browser can always reach the app.** The extension talks to a registered native host, which can start or wake the app. Ours fails when the app isn't running.
4. **It starts quietly with Windows** (`/onboot`, tray only).

## 3. Implementation plan

### Phase 0: Quick fixes (small) — implemented 2026-10-16
- Fetch the managed ffmpeg for YouTube jobs too, not only HLS (`youtube/mod.rs`, `download_once`).
- Update the yt-dlp that is actually used: prefer the managed copy over the PATH copy, or update whichever one is found.
- Raise the first-time yt-dlp download limit above 25 s.

Done when a fresh install can download a 1080p YouTube video with nothing preinstalled.

### Phase 1: Background start and native messaging (small)
- `dm-gui --background`: start hidden in the tray. The installer's start-with-Windows Run entry passes it.
- Installer registers `dm-native-host.exe` as a native messaging host (HKCU, Chrome + Edge + Firefox) with a JSON manifest that names our extension's ID.
- The native host's only new job: if the app isn't running, start `dm-gui --background`, then wait for `/health`.
- Extension: add `nativeMessaging`. When `127.0.0.1:7373` doesn't answer, ping the native host and retry instead of showing "not running".

Done when closing the app fully, then clicking download in the browser starts the app in the tray and the download goes through.

### Phase 2: Built-in muxer (`core/src/mux/`, medium–large)
Replace ffmpeg for the common case, matching what `idmvconv` covers:
- **fMP4 video + fMP4/M4A audio → MP4.** This is YouTube's H.264/AV1 + AAC. Read both `moov`s, write one `moov` with two `trak`s and `mvex`, then copy `moof`/`mdat` pairs with track IDs rewritten. Copy only, never re-encode.
- **WebM video + WebM audio → MKV/WebM.** This is VP9 + Opus. EBML: merge `Tracks`, re-number tracks, interleave `Cluster`s by timecode.
- Progress callback, cancel token, and output to a temp file renamed at the end (as the engine already does).
- Keep ffmpeg as a fallback for anything else (MPEG-TS, FLV) and when the built-in muxer reports "unsupported".
- Use a small box/EBML reader we write ourselves or a maintained crate; pick after a spike. Test against real YouTube samples, with the outputs checked by `ffprobe` in CI.

Done when YouTube downloads in every quality finish without ffmpeg installed.

### Phase 3: Direct YouTube streams (medium)
- Extension sends the sniffed video URL (chosen itag), the matching audio URL and the cookies to a new `POST /downloads/stream-pair` endpoint.
- Engine downloads both with the existing segmented engine. googlevideo needs `range=`-style chunking to avoid throttling; `clen=` gives the exact size. Then it calls the Phase 2 muxer.
- Expired URLs (HTTP 403) pause the job and ask the extension for fresh URLs, reusing the `EXPIRED_URL` flow from `native_host/main.rs`. Move that logic into the engine so the native host stays a thin launcher.
- yt-dlp becomes the fallback for pasted links and pages where sniffing found nothing.

Done when a YouTube video captured from the browser downloads and merges with no yt-dlp, Deno/Node or ffmpeg present.

### Phase 4: Own manifest parsing (medium)
- Parse DASH (`.mpd`, via `quick-xml`) and HLS (`.m3u8`, via `m3u8-rs`) in Rust, then download segments with the engine (`IDMVMPrs` equivalent).
- fMP4 segments go through the Phase 2 muxer. MPEG-TS keeps the ffmpeg fallback until we add a TS→MP4 remuxer (`MuxTSandTStoMP4` equivalent) as a later step.

### Phase 5: Polish (small, optional)
- Hold Alt while clicking a link to let the browser download it, Insert to force capture. This lives in the extension content script, so no system-wide hooks are needed (`idmmkb` equivalent).
- Publish the extension to the Chrome Web Store and Edge Add-ons, then have the installer write the `Software\Google\Chrome\Extensions\<id>` key (with `update_url`) so the browser offers it automatically.
- Translations, once the UI text settles.

## 4. Not copying

- **Kernel drivers (`idmwfp`, `idmtdi`) and `IDMNetMon`.** They need driver signing, are hard to get right, set off antivirus, and only add capture from non-browser apps.
- **Internet Explorer and Explorer shell extensions.** IE is gone; an Explorer menu is low value.
- **Extra 32-bit/64-bit builds.** We ship x64 only.

## Order

Phase 0 → 1 → 2 → 3 → 4 → 5. Phases 0 and 1 are small and fix real user pain now. Phase 2 must land before Phase 3, because direct streams are only useful once we can merge them without ffmpeg.
