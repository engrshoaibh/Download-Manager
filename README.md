# Download Manager Pro – A Fast, Free IDM & Ghost Downloader Alternative

**Download Manager Pro** is a fast, open-source download manager written entirely in **Rust**. It splits files into parallel segments, resumes safely after interruptions, downloads YouTube video and audio, and captures downloads straight from your browser. It is built as a free replacement for **IDM (Internet Download Manager)** and a **Ghost Downloader alternative**.

The desktop app is a native GPU-rendered window (egui), so there is no web view, no Node.js and no background Electron process.

## 🚀 Why Download Manager Pro?

- **Fast segmented downloads:** files are split into up to 32 parallel connections. When a connection finishes early, it takes over part of the slowest one, so bandwidth stays saturated to the end.
- **Resume that verifies itself:** progress is saved next to the file (`.part` and `.part.meta`). On resume, the app checks that the saved state still matches the partial file. If either is missing or inconsistent it restarts cleanly instead of producing a corrupt file.
- **Checked before "complete":** every byte range is verified and the finished file is moved into place before a download is marked done.
- **YouTube support:** powered by `yt-dlp` and `ffmpeg`, with a quality picker (best, 1080p, 720p, audio only).
- **Browser extension:** captures downloads and videos from Chrome, Edge and Brave and hands them to the app, including your session cookies for authenticated downloads.
- **Native, modern interface:** dark and light themes, category sidebar, live progress, search, and a right-click menu on every download. The layout adapts to narrow windows.
- **Free and open source:** MIT licensed, no licence prompts, no trials.

## ✨ Features

| | |
|---|---|
| **Segmentation** | Adaptive connection count, work stealing, HTTP/2, native root certificates for HTTPS on Windows |
| **Pause & resume** | Survives restarts; stale or missing state triggers a clean restart |
| **Bandwidth control** | Global speed limit and global connection cap |
| **YouTube** | Video, audio-only, quality selection, DASH merge via ffmpeg |
| **Browser capture** | Link and download interception, video banner, media grabber, HLS/DASH sniffing |
| **Local API** | REST and WebSocket server on `127.0.0.1:7373` used by the extension |
| **Self-update** | Checks GitHub releases and replaces the running executable |

## 📥 Installation

1. Open the [Releases](https://github.com/engrshoaibh/Download-Manager/releases) page.
2. Download `DownloadManagerPro-Setup-<version>.exe` and run it.
3. Finish the wizard and launch **Download Manager Pro**.

The installer is a single file. It installs per user (no administrator prompt) to `%LOCALAPPDATA%\Programs\Download Manager Pro`, adds a Start menu entry (and optionally a desktop shortcut or start-with-Windows), and registers an uninstaller. Upgrades install over the top: run a newer installer, or click **Update - Restart** in the app when a new version has been downloaded. Updates are verified against the release's `SHA256SUMS.txt` before they run.

It installs `dm-gui.exe` (the desktop app, which also runs the download engine and local API), `dm-daemon.exe` (headless engine), `dm-native-host.exe`, and the browser extension in the `browser-extension` folder.

### Browser extension

1. Open `chrome://extensions` (or `edge://extensions`) and enable **Developer mode**.
2. Click **Load unpacked** and choose `%LOCALAPPDATA%\Programs\Download Manager Pro\browser-extension` (there is a Start menu shortcut to the folder).
3. Keep Download Manager Pro running; the extension popup shows **App connected** when it can reach it.

See [extension/README.md](extension/README.md) for details.

### YouTube tips

YouTube sometimes answers yt-dlp with *"Sign in to confirm you're not a bot"*. If that happens, open **Options → YouTube sign-in** and either point to a `cookies.txt` exported from your browser or choose a browser to read cookies from (Firefox works most reliably). Sending the video from the browser extension also works, because it forwards your session cookies. `ffmpeg` must be installed for formats that need audio and video merged.

## 🛠️ Building from Source

You only need **Rust** (stable).

```bash
git clone https://github.com/engrshoaibh/Download-Manager.git
cd Download-Manager/core

# Run the desktop app
cargo run --bin dm-gui

# Run the tests
cargo test
```

Binaries built from `core/`:

| Binary | Description |
|---|---|
| `dm-gui` | Desktop app |
| `dm-daemon` | Headless engine + API |
| `dm-native-host` | Browser-extension native messaging host |

To produce the release assets locally (Windows, PowerShell):

```powershell
.\build_release.ps1   # builds the installer into .\release (needs Inno Setup: winget install JRSoftware.InnoSetup)
```

The live YouTube test needs network access, `yt-dlp` and `ffmpeg`, so it is opt-in:

```bash
cargo test --test youtube_live -- --ignored --nocapture
```

### Project layout

```
core/
  src/engine/     download engine: segments, work stealing, integrity checks, chunked streams
  src/network/    HTTP client and probing
  src/io/         sparse files, memory-mapped writes, resume metadata
  src/youtube/    yt-dlp / ffmpeg integration
  src/api/        REST + WebSocket server for the extension
  src/gui_*.rs    egui desktop app
  tests/          end-to-end download tests against a local server
extension/        Chrome / Edge / Brave extension
branding/         app icon
```

## 🤝 Contributing

Contributions are welcome. Fork the repository, open issues, and submit pull requests. Please run `cargo test` before sending a change.

## 📝 License

Released under the MIT License. See [LICENSE](LICENSE).

---
*Keywords: free IDM alternative, Internet Download Manager replacement, Ghost Downloader alternative, open source download manager, fast download manager, Rust download manager, YouTube video downloader.*
