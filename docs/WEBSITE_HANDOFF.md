# Website hand-off (hdmoviesshow.com → Download Manager Pro)

The website's download page can pass a video to this app, so the visitor does not
have to keep the browser tab open. This document describes how that works, what
this branch (`feature/website-handoff`) adds to the app, and what is still to do.

## How the hand-off works

1. The visitor finishes the website's download pages and reaches the save page.
2. They click **Send to Download Manager Pro**.
3. The page asks the website for a hand-off link. It looks like this:

   ```
   https://hdmoviesshow.com/catalog/download/<id>/playlist.m3u8?t=<signed token>&v=<quality>
   ```

4. The page sends that link to the app, in one of two ways:
   - **App already open:** `POST http://127.0.0.1:7373/downloads` with
     `{ "url": "<link>", "title": "<name>", "user_agent": "<browser UA>" }`.
   - **App closed (new in this branch):** the page opens
     `dmpro://download?url=<link>&title=<name>`. Windows starts the app, and the
     app starts the download.
5. The app sees `m3u8` in the link, treats it as an HLS stream and downloads it
   with yt-dlp. The playlist lists every part as a link back to the website with
   the same token.

### Why the link points at the website and not at the video host

- The video hosts need both a `Referer` and an `Origin` header. yt-dlp is only
  given `--referer`, so direct requests are answered with 403.
- Many provider links are bare tokens with no `m3u8` in them. The app's
  `is_stream_manifest_url` would not recognise them and would save the playlist
  text as a file.
- The website already adds the right headers and limits how hard each video host
  is hit. The token is only issued after the website's download pages and lasts
  6 hours.

The cost is that the video bytes pass through the website's server.

## What this branch adds

Nothing on this branch changes how existing downloads behave. YouTube downloads,
normal file downloads, the extension, the tray, the updater and tag releases use
the same code paths as before.

### 1. FFmpeg is fetched automatically for stream playlists

Without FFmpeg, an HLS download was saved with an `.mp4` name but the content was
raw MPEG-TS, which some players refuse.

- `core/src/youtube/mod.rs`: `find_or_fetch_ffmpeg()` and `fetch_ffmpeg()`.
- Runs only when the job is a stream manifest (HLS/DASH) **and** no FFmpeg is
  found on PATH, in `%LOCALAPPDATA%\download-manager\bin`, or next to the exe.
- Downloads the yt-dlp FFmpeg build (about 150 MB, once), unpacks `ffmpeg.exe`
  and `ffprobe.exe` with Windows' built-in `tar`, and stores them in
  `%LOCALAPPDATA%\download-manager\bin`.
- If the download fails or takes longer than 10 minutes, the job continues as it
  did before.
- While it is running, `/health` reports `ffmpeg.error: "FFmpeg is downloading…"`.
  The job itself shows no progress during this time.
- YouTube and normal downloads do not trigger the fetch. There is no download at
  app start.

### 2. `dmpro://` links

- `ui/src-tauri/Cargo.toml`, `tauri.conf.json`, `capabilities/default.json`,
  `src/lib.rs`: the `tauri-plugin-deep-link` plugin and the `dmpro` scheme. The
  installer registers the scheme with Windows.
- `ui/src/lib/deepLink.ts`, called from `ui/src/App.tsx`:
  - Accepts only `dmpro://download?url=…&title=…`.
  - The `url` must be on `https://hdmoviesshow.com`,
    `https://www.hdmoviesshow.com` or `http://localhost:3000` (development).
    Links to any other site are ignored.
  - Waits up to 20 seconds for the local API, then calls the same
    `POST /downloads` the UI and the extension use.
- A second launch is forwarded to the running app by the single-instance plugin.

### 3. Faster `/health`

`/health` scanned for the tools on every call by running `where` several times,
which took 0.8–1.5 seconds. The website waits for this answer.

- `tools_health_cached()` keeps the result for 30 seconds.
- The cache is cleared when yt-dlp or FFmpeg is installed.
- The response format is unchanged.

### 4. Build

- Version 0.1.5, so the installer upgrades an installed 0.1.3.
- `.github/workflows/release.yml`: manual and branch runs now upload the MSI and
  the setup exe as a run artifact called `installers`. Tag builds still create a
  release exactly as before.

## Still to do

In order of benefit:

1. **Lock the local API.** `core/src/api/mod.rs` allows any origin and has no
   authentication. Any website a user visits can call `POST /downloads`,
   `PUT /settings` and `POST /update` on their PC.
   - Allow only the extension (`chrome-extension://…`), the app's own window
     (`http://tauri.localhost`, `tauri://localhost`) and the website.
   - Reject requests whose `Origin` header is not on the list.
   - Send `Access-Control-Allow-Private-Network: true` on preflight answers so
     Chrome lets the website through.
2. **Direct downloads from the video host.** Add `headers` (passed to yt-dlp as
   `--add-headers`) and `kind: "hls"` to `POST /downloads`. The website could then
   hand over the real stream link and the video would no longer pass through the
   website's server.
3. **Show FFmpeg download progress** on the job instead of a silent wait.
4. **Start with Windows**, in the tray, so the app is usually already open.
5. **A public download link for the installer.** This repository is private, so
   visitors cannot use its releases. The website shows a "Get the app" link when
   `NEXT_PUBLIC_DM_APP_URL` is set.
6. **Extension in the Chrome Web Store**, if visitors should use it. The website
   hand-off does not need the extension.

## Build and test

Build (Windows, Rust and Node installed):

```powershell
.\build_release.ps1
```

Or on GitHub: **Actions → Release → Run workflow**, pick the branch, then download
the `installers` artifact from the run.

Checks after installing:

1. `curl http://127.0.0.1:7373/health` twice. The second call answers at once.
2. Send a video from the website's save page. After the first stream download,
   `ffmpeg.exe` is in `%LOCALAPPDATA%\download-manager\bin` and `/health` reports
   `ffmpeg.available: true`.
3. The saved file starts with the bytes `....ftyp` (a real MP4), not `G@`.
4. Close the app completely (tray → Quit). Open
   `dmpro://download?url=<a hand-off link>` from the website. The app starts and
   the download begins.
5. `dmpro://download?url=https://example.com/x.m3u8` does nothing.
6. Existing flows: a YouTube download, a normal file download, a capture from the
   extension, close-to-tray and second-launch focus all behave as before.
