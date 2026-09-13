# Download Manager — Browser Integration (Chrome)

IDM-style **Internet Download Module** for this project: sniffs downloadable URLs in the browser, intercepts Chrome’s native downloads, and sends them to the local daemon at `http://127.0.0.1:7373` with **User-Agent** and **cookies**.

## Features (IDM-like)

| Feature | Behavior |
|---|---|
| Download interception | Cancels matching Chrome downloads and routes them to DM |
| Confirm dialog | Ask mode opens an IDM-style “Download File” popup |
| **Video download banner** | Overlay on HTML5 players — click to pick resolution |
| Link sniffing | Click capture on zip/exe/pdf/video/… links |
| Alt+click | Force-capture any link |
| Context menu | “Download with Download Manager” on links & media |
| Media grabber | Floating panel listing video/audio/sources on the page |
| Session pass-through | Sends cookies + UA so authenticated downloads work |

## Prerequisites

1. **Daemon running** — start `dm-daemon` (listens on `127.0.0.1:7373`).
2. Optional: open the Tauri UI to watch progress.

## Install (Chrome / Edge / Brave)

1. Open `chrome://extensions` (or `edge://extensions`).
2. Enable **Developer mode**.
3. Click **Load unpacked**.
4. Select this folder: `extension/`.
5. Pin the toolbar icon and confirm the popup shows **Daemon online**.

## Usage

1. Click a downloadable link, or start any download Chrome would normally save.
2. In **Ask** mode, the capture window appears → **Start download**.
3. Or right-click a link/image/video → **Download with Download Manager**.
4. Use the toolbar popup to switch **Ask / Auto / Off**, edit file types, and toggle the media grabber.

## Settings

Stored in `chrome.storage.sync`:

- **Capture downloads** — master switch  
- **Capture mode** — `ask` (dialog), `auto` (silent to daemon), `off`  
- **Intercept browser downloads** — hook `chrome.downloads`  
- **Media grabber** — on-page floating list  
- **File types** — extension whitelist (same idea as IDM’s file types list)  
- **Segments** — passed to `POST /downloads`

## API used

```http
POST http://127.0.0.1:7373/downloads
Content-Type: application/json

{
  "url": "https://…",
  "user_agent": "Mozilla/5.0 …",
  "cookies": "name=value; …",
  "segments": 16
}
```

## Layout

```
extension/
  manifest.json          # MV3
  background/            # Intercept + context menus + messaging
  content/               # Link sniffer + media grabber
  capture/               # IDM-style confirm dialog
  popup/                 # Toolbar settings
  shared/                # Config + daemon client
  icons/
```

## Notes

- Manifest V3 cannot inject a classic Win32 “browser helper” like old IDM; this uses the supported Chrome APIs (`downloads`, content scripts, contextMenus) the same way modern managers (FDM, Persepolis, etc.) do.
- Native Messaging (blueprint) can replace localhost `fetch` later for tighter security; the daemon already accepts the same payload.
