import { loadSettings, filenameFromUrl } from '../shared/config.js';
import { guessResolution, isYouTubePageUrl } from '../shared/media.js';
import { probeYoutube, estimateSizeFromProbe } from '../shared/api.js';

const params = new URLSearchParams(location.search);
const url = params.get('url') || '';
const pageUrl = params.get('pageUrl') || '';
const referer = params.get('referer') || pageUrl;
const labelHint = params.get('label') || '';
const filenameHint = params.get('filename') || '';
const qualityParam = params.get('quality') || '';
const titleParam = params.get('title') || '';
const sizeParam = Number(params.get('size') || 0) || 0;

let finalSize = sizeParam;

const elUrl = document.getElementById('url');
const elName = document.getElementById('filename');
const elSeg = document.getElementById('segments');
const elDir = document.getElementById('saveDir');
const elStatus = document.getElementById('status');
const btnStart = document.getElementById('btnStart');
const btnCancel = document.getElementById('btnCancel');
const btnBrowser = document.getElementById('btnBrowser');
const btnBrowse = document.getElementById('btnBrowse');

elUrl.value = url;
const quality = qualityParam || labelHint || guessResolution({ url });
const isCdn = /googlevideo\.com/i.test(url) && /videoplayback/i.test(url);
const isYt =
  !isCdn &&
  (isYouTubePageUrl(url) ||
    isYouTubePageUrl(pageUrl) ||
    /googlevideo\.com/i.test(url));
const baseName = filenameFromUrl(url);
const safeTitle = titleParam
  .replace(/[<>:"/\\|?*]+/g, '')
  .replace(/\s+/g, ' ')
  .trim()
  .slice(0, 120);
elName.value =
  filenameHint ||
  (safeTitle
    ? `${safeTitle}${quality && !/^best$/i.test(quality) ? ` [${String(quality).replace(/\s+/g, '')}]` : ''}.${/audio/i.test(quality) ? 'm4a' : 'mp4'}`
    : baseName === 'videoplayback' || baseName === 'download' || isYt || isCdn
      ? `youtube_${String(quality).replace(/\s+/g, '_')}.${/audio/i.test(quality) ? 'm4a' : 'mp4'}`
      : baseName);

(async () => {
  const settings = await loadSettings();
  elSeg.value = String(settings.segments || 16);
  if (settings.saveDir) elDir.value = settings.saveDir;
  if (isYt && !isCdn) {
    elSeg.disabled = true;
    elSeg.title = 'YouTube processor uses yt-dlp (connections not applicable)';
  } else if (isCdn) {
    elSeg.disabled = false;
    elSeg.title = 'Direct googlevideo stream — multi-segment HTTP download';
  }

  /*
   * No app status here: Start wakes the app through the native host when it is closed,
   * and real failures are shown when Start fails. Only fetch the YouTube size quietly.
   */
  if (isYt && !isCdn && !finalSize) {
    try {
      const info = await probeYoutube(url);
      finalSize = estimateSizeFromProbe(info, quality || 'best') || 0;
    } catch (err) {
      console.error('Probe failed:', err);
    }
  }
})();

btnCancel.addEventListener('click', () => window.close());

btnBrowser.addEventListener('click', async () => {
  await chrome.runtime.sendMessage({ type: 'CAPTURE_URL', url, mode: 'browser' });
  window.close();
});

if (btnBrowse) {
  btnBrowse.addEventListener('click', async () => {
    try {
      const res = await fetch('http://127.0.0.1:7373/dialog/folder');
      const data = await res.json();
      if (data.path) {
        elDir.value = data.path;
      }
    } catch (err) {
      console.error('Failed to open folder dialog:', err);
    }
  });
}

btnStart.addEventListener('click', async () => {
  btnStart.disabled = true;
  setStatus('Starting download…', 'ok');
  try {
    const ytQuality = (() => {
      if (isCdn) return ''; /* direct CDN — no yt-dlp */
      if (!isYt && !qualityParam) return '';
      const raw = String(qualityParam || quality || '').trim();
      if (!raw) return isYt ? 'best' : '';
      if (/^audio/i.test(raw)) return 'audio';
      if (/^best$/i.test(raw)) return 'best';
      const m = raw.match(/(\d{3,4})/);
      return m ? m[1] : isYt ? 'best' : '';
    })();
    const res = await chrome.runtime.sendMessage({
      type: 'CAPTURE_URL',
      /* Adaptive CDN itags are rewritten in captureToDaemon to watch+itag+bestaudio */
      url,
      segments: Number(elSeg.value) || 16,
      saveDir: elDir.value.trim() || undefined,
      pageUrl,
      referer,
      filename: elName.value.trim() || undefined,
      title: safeTitle || titleParam || undefined,
      size: finalSize || undefined,
      totalBytes: finalSize || undefined,
      label: quality,
      quality: ytQuality || undefined,
    });
    if (!res?.ok) throw new Error(res?.error || 'Failed');
    setStatus(`Queued (id ${res.id})`, 'ok');
    setTimeout(() => window.close(), 450);
  } catch (err) {
    setStatus(err.message || String(err), 'error');
    btnStart.disabled = false;
  }
});

function setStatus(text, kind) {
  elStatus.hidden = false;
  elStatus.textContent = text;
  elStatus.className = `status ${kind || ''}`;
}
