import { DAEMON_URL, NATIVE_HOST } from './config.js';
import { suggestFilename, resolveCaptureTarget } from './media.js';

export async function checkDaemon() {
  try {
    const res = await fetch(`${DAEMON_URL}/health`, { method: 'GET' });
    if (!res.ok) return { ok: false, error: `HTTP ${res.status}` };
    const data = await res.json();
    return { ok: true, version: data.version || 'unknown' };
  } catch (e) {
    return { ok: false, error: e.message || 'Daemon unreachable' };
  }
}

/**
 * Like checkDaemon, but when the app is closed ask the native host to start it
 * (hidden in the tray) and wait for it, the way IDM's extension wakes IDM.
 */
export async function ensureDaemon() {
  const health = await checkDaemon();
  if (health.ok) return health;
  try {
    const reply = await chrome.runtime.sendNativeMessage(NATIVE_HOST, { type: 'launch' });
    if (reply?.status === 'ERROR') return { ok: false, error: reply.message || 'App failed to start' };
  } catch (e) {
    /* Host not registered (unpacked dev install) — report the original failure. */
    return health;
  }
  for (let i = 0; i < 10; i++) {
    const again = await checkDaemon();
    if (again.ok) return again;
    await new Promise((r) => setTimeout(r, 500));
  }
  return { ok: false, error: 'App started but is not answering yet' };
}

/** Probe YouTube formats/sizes via the local daemon (yt-dlp). */
export async function probeYoutube(url) {
  const res = await fetch(`${DAEMON_URL}/youtube/probe`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ url }),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(data.error || `Probe failed (${res.status})`);
  }
  return data;
}

/** Pick best matching format row from yt-dlp probe for a quality bucket. */
export function pickFormatFromProbe(info, quality) {
  if (!info?.formats?.length) return null;
  const q = String(quality || 'best').toLowerCase();
  const formats = info.formats;

  if (q === 'audio' || q === 'audio-only') {
    let best = null;
    for (const f of formats) {
      if (!(f.is_audio && !f.is_video)) continue;
      if (
        !best ||
        Number(f.filesize || f.filesize_approx || 0) >
          Number(best.filesize || best.filesize_approx || 0) ||
        Number(f.tbr || 0) > Number(best.tbr || 0)
      ) {
        best = f;
      }
    }
    return best;
  }

  const height = q === 'best' ? 99999 : Number(String(q).replace(/\D/g, '')) || 0;
  let bestVideo = null;
  for (const f of formats) {
    if (!f.is_video) continue;
    const h = Number(f.height) || 0;
    if (height < 99999 && h > height) continue;
    if (
      !bestVideo ||
      h > (bestVideo.height || 0) ||
      (h === (bestVideo.height || 0) &&
        Number(f.filesize || f.filesize_approx || 0) >
          Number(bestVideo.filesize || bestVideo.filesize_approx || 0))
    ) {
      bestVideo = f;
    }
  }
  return bestVideo;
}

/**
 * Bitrate in bits/sec for a quality (video+audio when merged).
 * yt-dlp `tbr` is kbps.
 */
export function bitrateFromProbe(info, quality) {
  const f = pickFormatFromProbe(info, quality);
  if (!f) return 0;
  let tbr = Number(f.tbr) || 0;
  if (String(quality || '').toLowerCase() === 'audio') {
    return tbr > 0 ? Math.round(tbr * 1000) : 0;
  }
  if (!f.is_progressive) {
    let audioTbr = 0;
    for (const a of info.formats || []) {
      if (a.is_audio && !a.is_video) {
        audioTbr = Math.max(audioTbr, Number(a.tbr) || 0);
      }
    }
    tbr += audioTbr;
  }
  if (tbr > 0) return Math.round(tbr * 1000);
  const size = estimateSizeFromProbe(info, quality);
  const duration = Number(info.duration) || 0;
  if (size > 0 && duration > 0) return Math.round((size * 8) / duration);
  return 0;
}

/**
 * Estimate bytes for a quality bucket from yt-dlp probe formats.
 */
export function estimateSizeFromProbe(info, quality) {
  if (!info?.formats?.length) return 0;
  const q = String(quality || 'best').toLowerCase();
  const formats = info.formats;
  const duration = Number(info.duration) || 0;

  const audioSize = () => {
    let best = 0;
    for (const f of formats) {
      if (f.is_audio && !f.is_video) {
        const sz = Number(f.filesize || f.filesize_approx || 0);
        if (sz > best) best = sz;
      }
    }
    if (best > 0) return best;
    let tbr = 0;
    for (const f of formats) {
      if (f.is_audio && !f.is_video && f.tbr) tbr = Math.max(tbr, Number(f.tbr) || 0);
    }
    return tbr > 0 && duration > 0 ? Math.round((tbr * 1000) / 8 * duration) : 0;
  };

  if (q === 'audio' || q === 'audio-only') return audioSize();

  const bestVideo = pickFormatFromProbe(info, quality);
  if (!bestVideo) return 0;
  let vSize = Number(bestVideo.filesize || bestVideo.filesize_approx || 0);
  if (!vSize && bestVideo.tbr && duration > 0) {
    vSize = Math.round((Number(bestVideo.tbr) * 1000) / 8 * duration);
  }
  const aSize = bestVideo.is_progressive ? 0 : audioSize();
  return vSize + aSize;
}

/**
 * Collect cookies for a URL (and optional page origin — needed for YouTube).
 */
export async function cookiesForUrl(url, pageUrl = '') {
  try {
    const maps = new Map();
    const urls = [url, pageUrl].filter(Boolean);
    /* Also pull youtube.com cookies when hitting googlevideo / watch pages */
    try {
      const u = new URL(url);
      if (/googlevideo\.com|youtube\.com|youtu\.be/i.test(u.hostname)) {
        urls.push('https://www.youtube.com/');
        urls.push('https://youtube.com/');
      }
    } catch {
      /* ignore */
    }
    try {
      if (pageUrl && /youtube\.com|youtu\.be/i.test(pageUrl)) {
        urls.push('https://www.youtube.com/');
      }
    } catch {
      /* ignore */
    }

    for (const target of urls) {
      const list = await chrome.cookies.getAll({ url: target });
      for (const c of list || []) {
        maps.set(c.name, c.value);
      }
    }

    /* Domain-wide youtube cookies */
    try {
      const yt = await chrome.cookies.getAll({ domain: '.youtube.com' });
      for (const c of yt || []) maps.set(c.name, c.value);
    } catch {
      /* ignore */
    }

    if (!maps.size) return '';
    return [...maps.entries()].map(([k, v]) => `${k}=${v}`).join('; ');
  } catch {
    return '';
  }
}

/**
 * Export the browser's YouTube/Google session as a Netscape cookie file.
 * Unlike a flat Cookie header this keeps each cookie's real domain, path, expiry and
 * flags, which is what yt-dlp needs to pass YouTube's "confirm you're not a bot" check.
 * It only ever goes to the local app on 127.0.0.1.
 */
export async function youtubeCookieJar() {
  try {
    const seen = new Map();
    for (const domain of ['youtube.com', 'google.com']) {
      const list = await chrome.cookies.getAll({ domain });
      for (const c of list || []) {
        seen.set(`${c.domain}|${c.path}|${c.name}`, c);
      }
    }
    if (!seen.size) return '';
    const lines = ['# Netscape HTTP Cookie File'];
    for (const c of seen.values()) {
      const expiry = c.session || !c.expirationDate ? 0 : Math.floor(c.expirationDate);
      lines.push([
        `${c.httpOnly ? '#HttpOnly_' : ''}${c.domain}`,
        c.domain.startsWith('.') ? 'TRUE' : 'FALSE',
        c.path || '/',
        c.secure ? 'TRUE' : 'FALSE',
        expiry,
        c.name,
        c.value,
      ].join('\t'));
    }
    return lines.join('\n');
  } catch {
    return '';
  }
}

/**
 * POST a new download to the local daemon.
 */
export async function addDownload({
  url,
  userAgent,
  cookies,
  referer,
  segments,
  saveDir,
  filename,
  title,
  totalBytes,
  size,
  quality,
  format,
}) {
  const body = { url };
  if (userAgent) body.user_agent = userAgent;
  if (cookies) body.cookies = cookies;
  if (referer) body.referer = referer;
  if (segments) body.segments = segments;
  if (saveDir) body.save_dir = saveDir;
  if (filename) body.filename = filename;
  if (title) body.title = title;
  const bytes = Number(totalBytes || size || 0);
  if (bytes > 0) {
    body.total_bytes = bytes;
    body.size = bytes;
  }
  if (quality) body.quality = quality;
  if (format) body.format = format;

  const res = await fetch(`${DAEMON_URL}/downloads`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });

  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(data.error || `Daemon error ${res.status}`);
  }
  return data;
}

/**
 * Full capture pipeline: cookies + UA + Referer → daemon (IDM-style).
 * Direct googlevideo/videoplayback URLs download with multi-segment HTTP
 * (same as online YT downloaders). Watch-page URLs still use yt-dlp.
 */
export async function captureToDaemon(url, settings = {}) {
  const health = await ensureDaemon();
  if (!health.ok) {
    throw new Error(
      'Download Manager Pro is not running and could not be started. Open the app first.'
    );
  }

  const pageUrl = settings.pageUrl || settings.referer || '';
  const cookies = await cookiesForUrl(url, pageUrl);
  const userAgent = navigator.userAgent;
  const referer = settings.referer || pageUrl || undefined;
  const title = (settings.title || '').trim();
  const label = settings.label || settings.quality || '';
  let bytes = Number(settings.totalBytes || settings.size || 0) || 0;

  /* Prefer clen embedded in CDN URLs (exact size, like online downloaders) */
  try {
    const u = new URL(url);
    if (/googlevideo\.com/i.test(u.hostname) && /videoplayback/i.test(u.pathname)) {
      const clen = Number(u.searchParams.get('clen') || 0);
      if (clen > 0) bytes = clen;
    }
  } catch {
    /* ignore */
  }

  /*
   * Adaptive CDN (itag 313/271/137…) is video-only. Pin yt-dlp to that itag +
   * bestaudio so we merge A/V like online tools, instead of downloading silent
   * video or falling through to ~360p via a height-only selector.
   */
  const resolved = resolveCaptureTarget(url, pageUrl, settings.quality || label);
  const downloadUrl = resolved.url;
  const isCdn =
    /googlevideo\.com/i.test(downloadUrl) && /videoplayback/i.test(downloadUrl);

  let quality = resolved.quality;
  let format = resolved.format || settings.format || undefined;
  if (isCdn) {
    /* Progressive / audio CDN — segmented HTTP; don't send yt-dlp selectors */
    quality = undefined;
    format = undefined;
  } else if (!format && settings.format) {
    format = settings.format;
  }
  /* Height-only processor clicks: keep quality for -f bv*[height=…] */
  if (!isCdn && !format && settings.quality) {
    quality = settings.quality;
  }

  let filename = settings.filename || '';
  if (!filename && title) {
    const safe = title
      .replace(/[<>:"/\\|?*\u0000-\u001f]+/g, '')
      .replace(/\s+/g, ' ')
      .trim()
      .slice(0, 120);
    const q = String(resolved.quality || settings.quality || label || '')
      .replace(/[^\w.\-]+/g, '')
      .slice(0, 24);
    const ext = /^audio$/i.test(String(resolved.quality || settings.quality || ''))
      ? 'm4a'
      : 'mp4';
    filename = q && !/^best$/i.test(q) ? `${safe} [${q}].${ext}` : `${safe}.${ext}`;
  }
  if (!filename) {
    filename = suggestFilename(downloadUrl, label);
  }

  /* yt-dlp jobs get the real browser cookie jar; direct CDN downloads keep the Cookie header. */
  let cookiesToSend = cookies;
  if (!isCdn && /youtube\.com|youtu\.be/i.test(`${downloadUrl} ${pageUrl}`)) {
    const jar = await youtubeCookieJar();
    if (jar) cookiesToSend = jar;
  }

  return addDownload({
    url: downloadUrl,
    userAgent,
    cookies: cookiesToSend || undefined,
    referer,
    segments: settings.segments || 16,
    saveDir: settings.saveDir || undefined,
    filename,
    title: title || undefined,
    totalBytes: bytes > 0 ? bytes : undefined,
    quality,
    format,
  });
}
