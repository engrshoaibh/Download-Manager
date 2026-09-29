/**
 * Media URL helpers — IDM-style resolution labels (incl. YouTube itag map).
 */

const RES_ORDER = [2160, 1440, 1080, 720, 480, 360, 240, 144];

/**
 * YouTube itag → quality (same idea IDM uses when sniffing googlevideo).
 * progressive = muxed A+V (preferred in the menu).
 */
const YT_ITAG = {
  17: { label: '144p', ext: '3gp', progressive: true },
  18: { label: '360p', ext: 'mp4', progressive: true },
  22: { label: '720p', ext: 'mp4', progressive: true },
  36: { label: '240p', ext: '3gp', progressive: true },
  37: { label: '1080p', ext: 'mp4', progressive: true },
  38: { label: '3072p', ext: 'mp4', progressive: true },
  43: { label: '360p', ext: 'webm', progressive: true },
  44: { label: '480p', ext: 'webm', progressive: true },
  45: { label: '720p', ext: 'webm', progressive: true },
  46: { label: '1080p', ext: 'webm', progressive: true },
  59: { label: '480p', ext: 'mp4', progressive: true },
  78: { label: '480p', ext: 'mp4', progressive: true },

  133: { label: '240p', ext: 'mp4', progressive: false },
  134: { label: '360p', ext: 'mp4', progressive: false },
  135: { label: '480p', ext: 'mp4', progressive: false },
  136: { label: '720p', ext: 'mp4', progressive: false },
  137: { label: '1080p', ext: 'mp4', progressive: false },
  138: { label: '2160p', ext: 'mp4', progressive: false },
  160: { label: '144p', ext: 'mp4', progressive: false },
  212: { label: '480p', ext: 'mp4', progressive: false },
  213: { label: '480p', ext: 'mp4', progressive: false },
  214: { label: '720p', ext: 'mp4', progressive: false },
  215: { label: '720p', ext: 'mp4', progressive: false },
  248: { label: '1080p', ext: 'webm', progressive: false },
  247: { label: '720p', ext: 'webm', progressive: false },
  244: { label: '480p', ext: 'webm', progressive: false },
  243: { label: '360p', ext: 'webm', progressive: false },
  242: { label: '240p', ext: 'webm', progressive: false },
  278: { label: '144p', ext: 'webm', progressive: false },
  271: { label: '1440p', ext: 'webm', progressive: false },
  313: { label: '2160p', ext: 'webm', progressive: false },
  298: { label: '720p60', ext: 'mp4', progressive: false },
  299: { label: '1080p60', ext: 'mp4', progressive: false },
  302: { label: '720p60', ext: 'webm', progressive: false },
  303: { label: '1080p60', ext: 'webm', progressive: false },
  308: { label: '1440p60', ext: 'webm', progressive: false },
  315: { label: '2160p60', ext: 'webm', progressive: false },
  330: { label: '144p', ext: 'webm', progressive: false },
  331: { label: '240p', ext: 'webm', progressive: false },
  332: { label: '360p', ext: 'webm', progressive: false },
  333: { label: '480p', ext: 'webm', progressive: false },
  334: { label: '720p', ext: 'webm', progressive: false },
  335: { label: '1080p', ext: 'webm', progressive: false },
  336: { label: '1440p', ext: 'webm', progressive: false },
  337: { label: '2160p', ext: 'webm', progressive: false },
  394: { label: '144p', ext: 'mp4', progressive: false },
  395: { label: '240p', ext: 'mp4', progressive: false },
  396: { label: '360p', ext: 'mp4', progressive: false },
  397: { label: '480p', ext: 'mp4', progressive: false },
  398: { label: '720p', ext: 'mp4', progressive: false },
  399: { label: '1080p', ext: 'mp4', progressive: false },
  400: { label: '1440p', ext: 'mp4', progressive: false },
  401: { label: '2160p', ext: 'mp4', progressive: false },
  694: { label: '144p', ext: 'mp4', progressive: false },
  695: { label: '240p', ext: 'mp4', progressive: false },
  696: { label: '360p', ext: 'mp4', progressive: false },
  697: { label: '480p', ext: 'mp4', progressive: false },
  698: { label: '720p', ext: 'mp4', progressive: false },
  699: { label: '1080p', ext: 'mp4', progressive: false },
  700: { label: '1440p', ext: 'mp4', progressive: false },
  701: { label: '2160p', ext: 'mp4', progressive: false },

  /* audio-only */
  139: { label: 'Audio', ext: 'm4a', progressive: false, audio: true },
  140: { label: 'Audio', ext: 'm4a', progressive: false, audio: true },
  141: { label: 'Audio', ext: 'm4a', progressive: false, audio: true },
  249: { label: 'Audio', ext: 'webm', progressive: false, audio: true },
  250: { label: 'Audio', ext: 'webm', progressive: false, audio: true },
  251: { label: 'Audio', ext: 'webm', progressive: false, audio: true },
  256: { label: 'Audio', ext: 'm4a', progressive: false, audio: true },
  258: { label: 'Audio', ext: 'm4a', progressive: false, audio: true },
};

/** Exported for capture pipeline — pin yt-dlp `-f` to the sniffed itag. */
export { YT_ITAG };

/**
 * Adaptive googlevideo URLs are video-only (or audio-only). Online downloaders
 * merge A+V; we do the same by pinning yt-dlp to the sniffed itag + bestaudio.
 * Progressive itags are muxed and can stay on segmented HTTP.
 */
export function resolveCaptureTarget(url, pageUrl = '', qualityHint = '') {
  const out = {
    url,
    quality: qualityHint || undefined,
    format: undefined,
    pinnedItag: 0,
    adaptive: false,
    audioOnly: false,
  };
  try {
    const u = new URL(url);
    const isCdn =
      /googlevideo\.com/i.test(u.hostname) && /videoplayback/i.test(u.pathname);
    if (!isCdn) return out;

    const itag = Number(u.searchParams.get('itag') || 0);
    const meta = YT_ITAG[itag];
    out.pinnedItag = itag;
    if (!itag || !meta) return out;

    if (meta.audio) {
      out.audioOnly = true;
      out.quality = 'audio';
      /* audio-only CDN can download via segmented HTTP */
      return out;
    }

    if (meta.progressive) {
      /* muxed A+V — keep CDN URL for fast multi-segment HTTP */
      const m = String(meta.label || '').match(/(\d{3,4})/);
      if (m) out.quality = m[1];
      return out;
    }

    const watch = normalizeYouTubeWatchUrl(pageUrl);
    if (!watch) return out;

    /* Adaptive video-only — pin exact itag (e.g. 313 = 4K webm) + best audio */
    out.url = watch;
    out.adaptive = true;
    out.format = `${itag}+bestaudio/ba`;
    const m = String(meta.label || qualityHint || '').match(/(\d{3,4})/);
    out.quality = m ? m[1] : qualityHint || undefined;
    return out;
  } catch {
    return out;
  }
}

export function isLikelyMediaUrl(url) {
  if (!url || typeof url !== 'string') return false;
  if (!/^https?:\/\//i.test(url)) return false;
  if (/^(blob:|data:)/i.test(url)) return false;
  if (/favicon|sprite|tracking|analytics|pixel|beacon|doubleclick|googlesyndication/i.test(url)) {
    return false;
  }

  try {
    const u = new URL(url);
    const path = u.pathname.toLowerCase();
    const full = url.toLowerCase();

    /* HLS/DASH playlists — including extensionless CDN paths ending in master/index */
    if (/\.(m3u8|mpd)(\?|$)/i.test(path) || /\/(master|index|playlist)\.m3u8/i.test(full)) {
      return true;
    }
    if (/\.(mp4|m4v|webm|mkv|mov|avi|flv|mp3|m4a|aac|ogg|wav|flac)(\?|$)/i.test(path)) {
      return true;
    }
    /* Lone .ts/.m4s segments are not useful menu entries */
    if (/\.(ts|m4s)(\?|$)/i.test(path) || /[?&]url=.*\.(ts|m4s)/i.test(full)) {
      return false;
    }
    if (/googlevideo\.com/i.test(u.hostname) && /videoplayback/i.test(path)) return true;
    /* Streaming sites often omit file extensions on HLS/DASH endpoints */
    if (
      /\/(hls|dash|stream|streams|manifest|playlist|master|index|play|vod|media|litro)\b/i.test(path) ||
      /[?&](m3u8|mpd|playlist|hls|format=m3u8|type=m3u8)/i.test(full) ||
      /mpegurl|dash\+xml|application\/vnd\.apple\.mpegurl/i.test(full)
    ) {
      return true;
    }
    if (/\/video\/|\/videos\/|\/media\/|\/stream|playback|get_video|videoplayback/i.test(full)) {
      return true;
    }
    /* Common CDN hosts used by movie streamers */
    if (
      /\.(cloudfront\.net|akamaized\.net|cdn\.|fastly|bunnycdn|stackpathcdn|workers\.dev|premilkyway)/i.test(
        u.hostname
      ) &&
      /video|stream|hls|dash|mp4|m3u8|litro/i.test(full)
    ) {
      return true;
    }
  } catch {
    /* fall through */
  }
  return false;
}

/** YouTube watch / shorts / embed / youtu.be page (not CDN). */
export function isYouTubePageUrl(url) {
  try {
    const u = new URL(url);
    const host = u.hostname.toLowerCase();
    const path = u.pathname.toLowerCase();
    if (host === 'youtu.be') return path.replace(/\//g, '').length >= 6;
    if (!/(^|\.)youtube\.com$/i.test(host) && host !== 'music.youtube.com') return false;
    return (
      path.startsWith('/watch') ||
      path.startsWith('/shorts/') ||
      path.startsWith('/embed/') ||
      path.startsWith('/live/') ||
      u.searchParams.has('v')
    );
  } catch {
    return false;
  }
}

/** Canonical https://www.youtube.com/watch?v=ID */
export function normalizeYouTubeWatchUrl(url) {
  try {
    const u = new URL(url);
    const host = u.hostname.toLowerCase();
    if (host === 'youtu.be') {
      const id = u.pathname.replace(/\//g, '');
      if (id) return `https://www.youtube.com/watch?v=${id}`;
    }
    const path = u.pathname;
    if (/^\/(shorts|embed|live)\//i.test(path)) {
      const id = path.split('/').filter(Boolean)[1];
      if (id) return `https://www.youtube.com/watch?v=${id}`;
    }
    const v = u.searchParams.get('v');
    if (v) return `https://www.youtube.com/watch?v=${v}`;
  } catch {
    /* ignore */
  }
  return url;
}

/**
 * Build yt-dlp-backed quality options for the current YouTube page.
 * These bypass googlevideo sniffing and let the desktop processor merge A/V.
 */
export function youtubeProcessorVariants(pageUrl) {
  if (!isYouTubePageUrl(pageUrl)) return [];
  const watch = normalizeYouTubeWatchUrl(pageUrl);
  const mk = (label, quality, extra = {}) => ({
    url: watch,
    label,
    quality,
    processor: true,
    progressive: true,
    ext: quality === 'audio' ? 'm4a' : 'mp4',
    ...extra,
  });
  return [
    mk('Best (yt-dlp)', 'best'),
    mk('2160p (4K)', '2160'),
    mk('1440p', '1440'),
    mk('1080p', '1080'),
    mk('720p', '720'),
    mk('480p', '480'),
    mk('360p', '360'),
    mk('Audio only', 'audio', { audio: true }),
  ];
}

/**
 * Map YouTube player quality level ids → height.
 * Matches the in-player Settings → Quality menu (incl. when only 360p is selected).
 */
export const YT_QUALITY_LEVEL_HEIGHT = {
  highres: 2160,
  hd2880: 2880,
  hd2160: 2160,
  hd1440: 1440,
  hd1080: 1080,
  hd720: 720,
  large: 480,
  medium: 360,
  small: 240,
  tiny: 144,
};

export function heightFromYoutubeQualityLevel(level) {
  const key = String(level || '').toLowerCase();
  if (YT_QUALITY_LEVEL_HEIGHT[key]) return YT_QUALITY_LEVEL_HEIGHT[key];
  const m = key.match(/(\d{3,4})/);
  return m ? Number(m[1]) : 0;
}

/** Exact byte size from googlevideo `clen=` (same as online YT downloaders). */
export function clenFromUrl(url) {
  try {
    const u = new URL(url);
    const n = Number(u.searchParams.get('clen') || 0);
    return Number.isFinite(n) && n > 0 ? n : 0;
  } catch {
    return 0;
  }
}

export function isGooglevideoStreamUrl(url) {
  try {
    const u = new URL(url);
    return (
      /googlevideo\.com/i.test(u.hostname) && /videoplayback/i.test(u.pathname)
    );
  } catch {
    return false;
  }
}

/** Decode player_response format.url or leave null if only signatureCipher. */
export function streamUrlFromFormat(f) {
  if (!f || typeof f !== 'object') return '';
  if (typeof f.url === 'string' && /^https?:\/\//i.test(f.url)) return f.url;
  return '';
}

/**
 * Pick best signed stream URL + size for a target height from streamingData.
 * Prefers progressive muxed, then mp4 video-only (IDM / online-downloader style).
 */
export function pickStreamForHeight(playerResponse, height, { audioOnly = false } = {}) {
  const sd = playerResponse?.streamingData || {};
  const all = [...(sd.formats || []), ...(sd.adaptiveFormats || [])];
  const progressiveItags = new Set((sd.formats || []).map((x) => Number(x.itag) || 0));
  let best = null;
  let bestScore = -1;

  for (const f of all) {
    const url = streamUrlFromFormat(f);
    if (!url) continue;
    const mime = String(f.mimeType || '');
    const isAudio = mime.startsWith('audio/');
    if (audioOnly) {
      if (!isAudio) continue;
    } else if (isAudio) {
      continue;
    }

    let h = Number(f.height) || 0;
    if (!h) {
      const m = String(f.qualityLabel || '').match(/(\d{3,4})/);
      if (m) h = Number(m[1]);
    }
    if (!audioOnly && height > 0 && h !== height) continue;

    const size = Number(f.contentLength || 0) || clenFromUrl(url);
    const progressive = progressiveItags.has(Number(f.itag) || 0);
    const mp4 = /mp4|avc1|av01|mp4a/i.test(mime);
    const score =
      (progressive ? 1e12 : 0) +
      (mp4 ? 1e9 : 0) +
      size +
      (Number(f.averageBitrate || f.bitrate || 0) || 0);
    if (score > bestScore) {
      bestScore = score;
      best = { url, size, itag: Number(f.itag) || 0, progressive, mime, height: h };
    }
  }
  return best;
}

/**
 * Attach sniffed googlevideo URLs (from webRequest) onto quality rows.
 * This is how IDM / online downloaders get instant segmented downloads.
 */
export function attachSniffedStreams(items, sniffed = []) {
  if (!items?.length) return items || [];
  const byHeight = new Map();
  const byItag = new Map();
  for (const s of sniffed) {
    if (!isGooglevideoStreamUrl(s.url)) continue;
    const yt = youtubeItagInfo(s.url);
    const size = Number(s.size) || clenFromUrl(s.url) || yt?.size || 0;
    const height =
      Number(s.height) ||
      Number(String(yt?.label || s.label || '').replace(/\D/g, '')) ||
      0;
    const row = { ...s, size, height, itag: yt?.itag, progressive: !!yt?.progressive, audio: !!yt?.audio };
    if (yt?.itag) {
      const prev = byItag.get(yt.itag);
      if (!prev || size > (prev.size || 0)) byItag.set(yt.itag, row);
    }
    if (height && !yt?.audio) {
      const prev = byHeight.get(height);
      const prefer =
        !prev ||
        (row.progressive && !prev.progressive) ||
        (row.progressive === prev.progressive && size > (prev.size || 0));
      if (prefer) byHeight.set(height, row);
    }
  }

  return items.map((v) => {
    if (v.audio) {
      const hit = [...byItag.values()].find((s) => s.audio) || null;
      if (!hit?.url) return v;
      return {
        ...v,
        url: hit.url,
        size: hit.size || v.size || 0,
        sizeLabel: hit.size || v.size ? formatBytes(hit.size || v.size) : v.sizeLabel,
        processor: false,
        stream: true,
        source: 'sniffed',
      };
    }
    const h = Number(v.height) || Number(String(v.quality).replace(/\D/g, '')) || 0;
    const hit = (h && byHeight.get(h)) || null;
    if (!hit?.url) return v;
    return {
      ...v,
      url: hit.url,
      size: hit.size || v.size || 0,
      sizeLabel: hit.size || v.size ? formatBytes(hit.size || v.size) : v.sizeLabel,
      processor: false,
      progressive: !!hit.progressive,
      stream: true,
      source: 'sniffed',
      formatId: hit.itag ? String(hit.itag) : v.formatId,
    };
  });
}

/**
 * Build menu rows from the live player quality list (IDM-accurate resolutions)
 * and attach sizes / direct stream URLs from streamingData when available.
 */
export function variantsFromQualityLevels(levels, playerResponse, pageUrl = '') {
  const watch = normalizeYouTubeWatchUrl(pageUrl) || pageUrl || '';
  if (!watch || !isYouTubePageUrl(watch)) return null;

  const vd = playerResponse?.videoDetails || {};
  const title = pageTitleFilename(vd.title || '');
  const duration = Number(vd.lengthSeconds) || 0;
  const sd = playerResponse?.streamingData || {};
  const allFormats = [...(sd.formats || []), ...(sd.adaptiveFormats || [])];

  const sizeByHeight = new Map();
  let bestAudioSize = 0;
  let bestAudioBitrate = 0;
  for (const f of allFormats) {
    const mime = String(f.mimeType || '');
    if (mime.startsWith('audio/')) {
      const sz = Number(f.contentLength || 0);
      const br = Number(f.averageBitrate || f.bitrate || 0);
      if (sz > bestAudioSize) bestAudioSize = sz;
      if (br > bestAudioBitrate) bestAudioBitrate = br;
      continue;
    }
    let h = Number(f.height) || 0;
    if (!h) {
      const m = String(f.qualityLabel || '').match(/(\d{3,4})/);
      if (m) h = Number(m[1]);
    }
    if (!h) continue;
    let size = Number(f.contentLength || 0);
    const br = Number(f.averageBitrate || f.bitrate || 0);
    if (!size && br > 0 && duration > 0) size = Math.round((br / 8) * duration);
    const progressive = (sd.formats || []).some((x) => Number(x.itag) === Number(f.itag));
    const total = size > 0 ? size + (progressive ? 0 : bestAudioSize) : 0;
    const prev = sizeByHeight.get(h) || 0;
    if (total > prev) sizeByHeight.set(h, total);
  }

  const heights = [];
  const seen = new Set();
  for (const level of levels || []) {
    const h = heightFromYoutubeQualityLevel(level);
    if (!h || seen.has(h)) continue;
    seen.add(h);
    heights.push(h);
  }
  heights.sort((a, b) => b - a);
  if (!heights.length) return null;

  const items = heights.map((height) => {
    const stream = pickStreamForHeight(playerResponse, height);
    const size = stream?.size || sizeByHeight.get(height) || 0;
    const label = qualityDisplayLabel(height, { hdSuffix: true }) || `${height}p`;
    const hasStream = !!stream?.url;
    return enrichVariantForMenu(
      {
        url: hasStream ? stream.url : watch,
        label,
        quality: String(height),
        processor: !hasStream,
        progressive: hasStream ? !!stream.progressive : true,
        stream: hasStream,
        ext: 'mp4',
        container: 'MP4',
        height,
        size,
        sizeLabel: size ? formatBytes(size) : '',
        duration,
        title,
        formatId: stream?.itag ? String(stream.itag) : '',
        source: hasStream ? 'player_stream' : 'quality_levels',
      },
      { title, duration }
    );
  });

  const bestStream = items.find((i) => i.stream) || items[0];
  const bestSize = bestStream?.size || items[0]?.size || 0;
  const best = enrichVariantForMenu(
    {
      url: bestStream?.stream ? bestStream.url : watch,
      label: 'Best',
      quality: bestStream?.stream ? String(bestStream.height || bestStream.quality) : 'best',
      processor: !bestStream?.stream,
      progressive: true,
      stream: !!bestStream?.stream,
      ext: 'mp4',
      container: 'MP4',
      height: items[0]?.height || 0,
      size: bestSize,
      sizeLabel: bestSize ? formatBytes(bestSize) : '',
      duration,
      title,
      source: bestStream?.stream ? 'player_stream' : 'quality_levels',
    },
    { title, duration }
  );

  let audioItem = null;
  const audioStream = pickStreamForHeight(playerResponse, 0, { audioOnly: true });
  for (const f of allFormats) {
    if (!String(f.mimeType || '').startsWith('audio/')) continue;
    const size = Number(f.contentLength || 0) || audioStream?.size || bestAudioSize;
    const hasStream = !!audioStream?.url;
    audioItem = enrichVariantForMenu(
      {
        url: hasStream ? audioStream.url : watch,
        label: 'Audio only',
        quality: 'audio',
        processor: !hasStream,
        audio: true,
        stream: hasStream,
        ext: 'm4a',
        container: 'M4A',
        size,
        sizeLabel: size ? formatBytes(size) : '',
        duration,
        title,
        source: hasStream ? 'player_stream' : 'quality_levels',
      },
      { title, duration }
    );
    break;
  }

  return {
    title,
    duration,
    items: audioItem ? [best, ...items, audioItem] : [best, ...items],
  };
}

/**
 * IDM-style: turn ytInitialPlayerResponse into resolution rows instantly
 * (no yt-dlp). Uses direct stream URLs when present; otherwise watch+processor.
 */
export function variantsFromPlayerResponse(playerResponse, pageUrl = '') {
  if (!playerResponse || typeof playerResponse !== 'object') return null;
  const watch = normalizeYouTubeWatchUrl(pageUrl) || pageUrl || '';
  if (!watch || !isYouTubePageUrl(watch)) return null;

  const vd = playerResponse.videoDetails || {};
  const title = pageTitleFilename(vd.title || '');
  const duration = Number(vd.lengthSeconds) || 0;
  const sd = playerResponse.streamingData || {};
  const progressiveList = sd.formats || [];
  const adaptiveList = sd.adaptiveFormats || [];
  const progressiveItags = new Set(progressiveList.map((x) => Number(x.itag) || 0));
  const raw = [...progressiveList, ...adaptiveList];
  if (!raw.length) return null;

  let bestAudioSize = 0;
  let bestAudioBitrate = 0;
  for (const f of raw) {
    const mime = String(f.mimeType || '');
    if (!mime.startsWith('audio/')) continue;
    const sz = Number(f.contentLength || 0);
    const br = Number(f.averageBitrate || f.bitrate || 0);
    if (sz > bestAudioSize) bestAudioSize = sz;
    if (br > bestAudioBitrate) bestAudioBitrate = br;
  }
  if (!bestAudioSize && bestAudioBitrate > 0 && duration > 0) {
    bestAudioSize = Math.round((bestAudioBitrate / 8) * duration);
  }

  /** @type {Map<string, object>} */
  const byKey = new Map();

  for (const f of raw) {
    const itag = Number(f.itag) || 0;
    const mime = String(f.mimeType || '');
    const isAudio = mime.startsWith('audio/');
    const isVideo = mime.startsWith('video/') || (!!f.width && !!f.height);
    if (!isAudio && !isVideo) continue;

    let height = Number(f.height) || 0;
    const ql = String(f.qualityLabel || '');
    if (!height) {
      const m = ql.match(/(\d{3,4})/);
      if (m) height = Number(m[1]);
    }
    const fps = Number(f.fps) || 0;
    const fromProgressiveList = progressiveItags.has(itag);

    let size = Number(f.contentLength || 0);
    const bitrate = Number(f.averageBitrate || f.bitrate || 0);
    if (!size && bitrate > 0 && duration > 0) {
      size = Math.round((bitrate / 8) * duration);
    }

    const ext = mime.includes('webm')
      ? 'webm'
      : mime.includes('mp4') || mime.includes('mp4a')
        ? isAudio
          ? 'm4a'
          : 'mp4'
        : isAudio
          ? 'm4a'
          : 'mp4';

    const streamUrl = streamUrlFromFormat(f);
    const hasStream = !!streamUrl;
    if (!size && hasStream) size = clenFromUrl(streamUrl);

    if (isAudio && !isVideo) {
      const key = `audio:${ext}`;
      const prev = byKey.get(key);
      if (!prev || size > (prev.size || 0) || bitrate > (prev.bandwidth || 0)) {
        byKey.set(
          key,
          enrichVariantForMenu(
            {
              url: hasStream ? streamUrl : watch,
              label: 'Audio only',
              quality: 'audio',
              processor: !hasStream,
              stream: hasStream,
              audio: true,
              progressive: false,
              ext,
              container: ext === 'webm' ? 'WEBM' : 'M4A',
              size,
              sizeLabel: size ? formatBytes(size) : '',
              duration,
              title,
              bandwidth: bitrate,
              bitrate,
              formatId: itag ? String(itag) : '',
              source: hasStream ? 'player_stream' : 'player_response',
            },
            { title, duration }
          )
        );
      }
      continue;
    }

    if (!height) continue;

    const fpsTag = fps >= 50 ? '60' : '';
    const key = `${height}${fpsTag}:${ext === 'webm' ? 'webm' : 'mp4'}`;
    let totalSize = size;
    if (!fromProgressiveList) {
      totalSize = size > 0 ? size + bestAudioSize : 0;
    }

    const label = fpsTag
      ? `${height}p${fpsTag}`
      : qualityDisplayLabel(height, { hdSuffix: true }) || `${height}p`;
    /* For adaptive (non-progressive) keep watch+processor unless we have a signed URL —
       video-only CDN files need a separate audio track; online downloaders often ship video-only. */
    const useStream = hasStream;
    const row = enrichVariantForMenu(
      {
        url: useStream ? streamUrl : watch,
        label,
        quality: String(height),
        processor: !useStream,
        progressive: fromProgressiveList,
        stream: useStream,
        ext: 'mp4',
        container: 'MP4',
        height,
        size: useStream ? size || totalSize : totalSize,
        sizeLabel: (useStream ? size || totalSize : totalSize)
          ? formatBytes(useStream ? size || totalSize : totalSize)
          : '',
        duration,
        title,
        bandwidth: bitrate + (!fromProgressiveList ? bestAudioBitrate : 0),
        bitrate: bitrate + (!fromProgressiveList ? bestAudioBitrate : 0),
        formatId: itag ? String(itag) : '',
        source: useStream ? 'player_stream' : 'player_response',
      },
      { title, duration }
    );

    const prev = byKey.get(key);
    const score = (v) =>
      (v.progressive ? 1e15 : 0) +
      (String(v.container || '') === 'MP4' ? 1e12 : 0) +
      (Number(v.size) || 0) +
      (Number(v.bandwidth) || 0);
    if (!prev || score(row) > score(prev)) {
      byKey.set(key, row);
    }
  }

  const videos = [...byKey.values()]
    .filter((v) => !v.audio)
    .sort((a, b) => (b.height || 0) - (a.height || 0) || (b.size || 0) - (a.size || 0));
  const audios = [...byKey.values()].filter((v) => v.audio);

  if (!videos.length && !audios.length) return null;

  const bestSize = videos[0]?.size || 0;
  const best = enrichVariantForMenu(
    {
      url: watch,
      label: 'Best',
      quality: 'best',
      processor: true,
      progressive: true,
      ext: 'mp4',
      container: 'MP4',
      height: videos[0]?.height || 0,
      size: bestSize,
      sizeLabel: bestSize ? formatBytes(bestSize) : '',
      duration,
      title,
      bandwidth: videos[0]?.bandwidth || 0,
      source: 'player_response',
    },
    { title, duration }
  );

  /* One row per height (prefer mp4 key already), plus audio */
  const seenH = new Set();
  const uniqueVideos = [];
  for (const v of videos) {
    const h = v.height || 0;
    if (seenH.has(h)) continue;
    seenH.add(h);
    uniqueVideos.push(v);
  }

  return {
    title,
    duration,
    items: [best, ...uniqueVideos, ...audios.slice(0, 2)],
  };
}

/** Extract ytInitialPlayerResponse JSON object text from HTML (CSP-safe fallback). */
export function extractPlayerResponseFromHtml(html) {
  if (!html || typeof html !== 'string') return null;
  const markers = ['ytInitialPlayerResponse', 'var ytInitialPlayerResponse'];
  let start = -1;
  for (const m of markers) {
    const i = html.indexOf(m);
    if (i >= 0) {
      start = html.indexOf('{', i);
      if (start >= 0) break;
    }
  }
  if (start < 0) return null;

  let depth = 0;
  let inStr = false;
  let esc = false;
  for (let j = start; j < html.length && j < start + 5_000_000; j++) {
    const c = html[j];
    if (inStr) {
      if (esc) esc = false;
      else if (c === '\\') esc = true;
      else if (c === '"') inStr = false;
      continue;
    }
    if (c === '"') {
      inStr = true;
      continue;
    }
    if (c === '{') depth += 1;
    else if (c === '}') {
      depth -= 1;
      if (depth === 0) {
        try {
          return JSON.parse(html.slice(start, j + 1));
        } catch {
          return null;
        }
      }
    }
  }
  return null;
}

export function isMediaMime(mime) {
  if (!mime) return false;
  const m = mime.toLowerCase().split(';')[0].trim();
  return (
    m.startsWith('video/') ||
    m.startsWith('audio/') ||
    m === 'application/vnd.apple.mpegurl' ||
    m === 'application/x-mpegurl' ||
    m === 'application/dash+xml'
  );
}

/** Strip only byte-range chunk params — keep pot/alr (YouTube auth tokens). */
export function canonicalizeMediaUrl(url) {
  try {
    const u = new URL(url);
    ['range', 'rn', 'rbuf', 'ump', 'stream_fragment', 'sq', 'headm', 'keepalive', 'cmo'].forEach(
      (p) => u.searchParams.delete(p)
    );
    return u.href;
  } catch {
    return url;
  }
}

export function youtubeItagInfo(url) {
  try {
    const u = new URL(url);
    if (!/googlevideo\.com|youtube\.com/i.test(u.hostname) && !/videoplayback/i.test(u.pathname)) {
      return null;
    }
    const itag = Number(u.searchParams.get('itag') || 0);
    if (!itag) return null;
    const meta = YT_ITAG[itag];
    const clen = Number(u.searchParams.get('clen') || 0);
    const mime = decodeURIComponent(u.searchParams.get('mime') || '').replace(/\+/g, ' ');
    return {
      itag,
      ...(meta || { label: `itag ${itag}`, ext: '', progressive: false }),
      size: clen || 0,
      mime,
    };
  } catch {
    return null;
  }
}

export function mediaDedupeKey(url) {
  const yt = youtubeItagInfo(url);
  if (yt) {
    try {
      const u = new URL(url);
      const id = u.searchParams.get('id') || u.searchParams.get('v') || u.hostname;
      return `yt:${id}:${yt.itag}`;
    } catch {
      return `yt:${yt.itag}:${url}`;
    }
  }
  return canonicalizeMediaUrl(url).split('#')[0];
}

/**
 * Guess a human label like "1080p" / "720p MP4" from URL / size / dimensions.
 */
export function guessResolution({ url = '', width = 0, height = 0, label = '', size = 0 } = {}) {
  const yt = youtubeItagInfo(url);
  if (yt) {
    if (yt.audio) return `Audio (${yt.ext || 'm4a'})`;
    const ext = yt.ext ? ` ${yt.ext.toUpperCase()}` : '';
    return `${yt.label}${ext}`;
  }

  if (label && /\d{3,4}\s*p/i.test(label)) {
    return normalizeResLabel(label.match(/(\d{3,4})\s*p/i)[1] + 'p');
  }
  if (label && /4k|uhd|2160/i.test(label)) return '2160p';
  if (label && /1440|2k|qhd/i.test(label)) return '1440p';
  if (label && /1080|fhd|full\s*hd/i.test(label)) return '1080p';
  if (label && /720|hd/i.test(label)) return '720p';
  if (label && /480|sd/i.test(label)) return '480p';
  if (label && /360/i.test(label)) return '360p';
  if (label && /240/i.test(label)) return '240p';

  const h = height || 0;
  const w = width || 0;
  const dim = Math.max(h, w > 0 && h === 0 ? Math.round((w * 9) / 16) : 0);
  if (dim >= 2000) return '2160p';
  if (dim >= 1400) return '1440p';
  if (dim >= 1000) return '1080p';
  if (dim >= 700) return '720p';
  if (dim >= 450) return '480p';
  if (dim >= 300) return '360p';
  if (dim >= 200) return '240p';

  const fromUrl = url.match(
    /(?:^|[_\-/=.?&])((?:2160|1440|1080|720|480|360|240|144))p?(?:[_\-/=.?&]|$)/i
  );
  if (fromUrl) return `${fromUrl[1]}p`;

  if (/[_\-/=]4k[_\-/=.]|uhd/i.test(url)) return '2160p';
  if (/[_\-/=]2k[_\-/=.]|qhd/i.test(url)) return '1440p';
  if (/full[\-_]?hd|fhd/i.test(url)) return '1080p';
  if (/(?:^|[_\-/=])hd(?:[_\-/=.]|$)/i.test(url)) return '720p';
  if (/(?:^|[_\-/=])sd(?:[_\-/=.]|$)/i.test(url)) return '480p';

  if (size > 0) {
    if (size > 800 * 1024 * 1024) return '1080p+';
    if (size > 250 * 1024 * 1024) return '720p+';
    if (size > 80 * 1024 * 1024) return '480p+';
  }

  return 'Default';
}

export function normalizeResLabel(label) {
  const m = String(label).match(/(\d{3,4})\s*p/i);
  return m ? `${m[1]}p` : label || 'Default';
}

export function resolutionSortKey(label) {
  const m = String(label).match(/(\d{3,4})/);
  if (!m) return String(label).toLowerCase().includes('audio') ? -1 : 0;
  const n = Number(m[1]);
  const idx = RES_ORDER.indexOf(n);
  return idx >= 0 ? RES_ORDER.length - idx : n;
}

export function formatBytes(n) {
  if (!n || n <= 0) return '';
  const u = ['B', 'KB', 'MB', 'GB'];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v < 10 && i > 0 ? v.toFixed(1) : Math.round(v)} ${u[i]}`;
}

/**
 * Drop sentinel lengths. 2^32 (4294967296) formats as exactly "4.0 GB"
 * and is a bogus Content-Length, not a movie file.
 */
export function sanitizeByteLength(n) {
  const x = Number(n) || 0;
  if (x <= 0 || !Number.isFinite(x)) return 0;
  if (x === 4294967295 || x === 4294967296 || x >= 2 ** 32) return 0;
  if (x > 64 * 1024 * 1024 * 1024) return 0;
  return Math.round(x);
}

/** IDM-style bitrate: "4882 kbps" */
export function formatBitrate(bps) {
  const n = Number(bps) || 0;
  if (n <= 0) return '';
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)} Mbps`;
  const kbps = Math.round(n / 1000);
  return `${kbps} kbps`;
}

/** IDM-style duration: "2 hours 17 min" */
export function formatDuration(seconds) {
  const s = Math.max(0, Math.floor(Number(seconds) || 0));
  if (!s) return '';
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) {
    if (m > 0) return `${h} hour${h > 1 ? 's' : ''} ${m} min`;
    return `${h} hour${h > 1 ? 's' : ''}`;
  }
  if (m > 0) {
    if (sec > 0 && m < 5) return `${m} min ${sec} sec`;
    return `${m} min`;
  }
  return `${sec} sec`;
}

/** "1080p HD" / "360p" like IDM */
export function qualityDisplayLabel(heightOrLabel, { hdSuffix = true } = {}) {
  const raw = String(heightOrLabel || '');
  const m = raw.match(/(\d{3,4})\s*p?/i);
  const h = m ? Number(m[1]) : Number(heightOrLabel) || 0;
  if (!h) return raw && raw !== 'Default' ? raw : '';
  const base = `${h}p`;
  if (!hdSuffix) return base;
  if (h >= 2160) return `${base} 4K`;
  if (h >= 1440) return `${base} QHD`;
  if (h >= 1080) return `${base} HD`;
  if (h >= 720) return `${base} HD`;
  return base;
}

export function containerFromUrlOrMime(url = '', mime = '') {
  const m = String(mime || '').toLowerCase();
  if (m.includes('mp4') || m.includes('m4v')) return 'MP4';
  if (m.includes('webm')) return 'WEBM';
  if (m.includes('mpegurl') || m.includes('m3u8')) return 'HLS';
  if (m.includes('mp2t') || m.includes('mp2ts')) return 'TS';
  try {
    const path = new URL(url).pathname.toLowerCase();
    if (/\.m3u8(\?|$)/i.test(path)) return 'HLS';
    if (/\.mpd(\?|$)/i.test(path)) return 'DASH';
    if (/\.ts(\?|$)/i.test(path)) return 'TS';
    if (/\.mp4|\.m4v(\?|$)/i.test(path)) return 'MP4';
    if (/\.webm(\?|$)/i.test(path)) return 'WEBM';
    if (/\.mkv(\?|$)/i.test(path)) return 'MKV';
  } catch {
    /* ignore */
  }
  return '';
}

/**
 * Build IDM-style detail string:
 * "TS file, 2 hours 17 min, quality 1080p HD, 4882 kbps"
 */
export function idmDetailLine(v = {}) {
  const container =
    v.container ||
    containerFromUrlOrMime(v.url, v.mime) ||
    (v.type === 'hls' ? 'TS' : v.ext ? String(v.ext).toUpperCase() : 'MP4');
  const fileWord = /HLS|DASH|TS/i.test(container) ? `${container === 'HLS' ? 'TS' : container} file` : `${container} file`;
  const dur = formatDuration(v.duration || v.durationSeconds || 0);
  const quality = qualityDisplayLabel(v.height || v.label || v.quality);
  const br = formatBitrate(v.bandwidth || v.bitrate || 0);
  const size = v.sizeLabel || (v.size ? formatBytes(v.size) : '');

  const parts = [fileWord];
  if (dur) parts.push(dur);
  if (quality) parts.push(`quality ${quality}`);
  if (size) parts.push(size);
  else if (br) parts.push(br);
  return parts.join(', ');
}

/** Page title cleaned for download naming (IDM uses this as the row title). */
export function pageTitleFilename(title = '') {
  let t = String(title || '')
    .replace(/\s*[\|–—]\s*YouTube\s*$/i, '')
    .replace(/\s+-\s+YouTube\s*$/i, '')
    .replace(/\s*[\|–—]\s*HDMovies?\s*Stream.*$/i, '')
    .replace(/\s*[\|–—]\s*Watch Free.*$/i, '')
    .replace(/\s+Full Movie Online Free.*$/i, '')
    .replace(/\s*\(\d{4}\)\s*$/i, '')
    .replace(/\s+/g, ' ')
    .trim();
  /* Prefer "Watch …" form when the site title already has it */
  if (t.length > 90) t = `${t.slice(0, 87)}…`;
  return t || 'Video';
}

/** Guess height from playlist path (/1080p/, /720/, index_1080.m3u8). */
export function heightFromMediaUrl(url = '') {
  const s = String(url || '');
  const m =
    s.match(/[/_-](\d{3,4})p[/_?&#.\-]/i) ||
    s.match(/[?&](?:quality|res|height|q)=(\d{3,4})/i) ||
    s.match(/\/(\d{3,4})p\//i);
  if (!m) return 0;
  const h = Number(m[1]);
  return h >= 144 && h <= 4320 ? h : 0;
}

/** Suggest a download filename from a media URL (avoids bare "videoplayback"). */
export function suggestFilename(url, label = '') {
  const yt = youtubeItagInfo(url);
  const quality = (label || (yt && !yt.audio ? yt.label : '') || 'video')
    .replace(/\s+/g, '_')
    .replace(/[^\w.\-()]+/g, '');
  if (yt) {
    const ext = yt.ext || (yt.audio ? 'm4a' : 'mp4');
    return `youtube_${quality}.${ext}`;
  }
  try {
    const u = new URL(url);
    const last = decodeURIComponent(u.pathname.split('/').filter(Boolean).pop() || 'download');
    const base = last.split('?')[0];
    if (base && base !== 'videoplayback' && base !== 'download') return base;
  } catch {
    /* ignore */
  }
  return `video_${quality || 'download'}.mp4`;
}

/**
 * Media playlist → segments. BYTERANGE lengths are exact byte sizes.
 * Returns { duration, segments, mapUrl, mapBytes }.
 */
export function parseM3u8Media(text, baseUrl) {
  const lines = String(text || '').split(/\r?\n/);
  const segments = [];
  let duration = 0;
  let pendingDur = 0;
  let pendingRange = null;
  let mapUrl = '';
  let mapBytes = 0;
  for (const raw of lines) {
    const line = raw.trim();
    if (!line) continue;
    if (line.startsWith('#EXTINF:')) {
      pendingDur = Number(line.slice('#EXTINF:'.length).split(',')[0]) || 0;
      duration += pendingDur;
      continue;
    }
    if (line.startsWith('#EXT-X-BYTERANGE:')) {
      const spec = line.slice('#EXT-X-BYTERANGE:'.length).trim();
      const [len, off] = spec.split('@');
      pendingRange = { length: Number(len) || 0, offset: off != null ? Number(off) : null };
      continue;
    }
    if (line.startsWith('#EXT-X-MAP:')) {
      const uri = /URI="([^"]+)"/i.exec(line);
      const br = /BYTERANGE="(\d+)(?:@(\d+))?"/i.exec(line);
      if (uri) {
        try {
          mapUrl = new URL(uri[1], baseUrl).href;
        } catch {
          mapUrl = uri[1];
        }
      }
      if (br) mapBytes = Number(br[1]) || 0;
      continue;
    }
    if (line.startsWith('#')) continue;
    let abs = line;
    try {
      abs = new URL(line, baseUrl).href;
    } catch {
      /* keep */
    }
    segments.push({ url: abs, duration: pendingDur, byterange: pendingRange });
    pendingDur = 0;
    pendingRange = null;
  }
  if (!duration) duration = parseM3u8Duration(text);
  return { duration, segments, mapUrl, mapBytes };
}

/** Exact size when every segment declares EXT-X-BYTERANGE, plus the init map. */
export function sizeFromByteranges(parsed) {
  if (!parsed?.segments?.length) return 0;
  if (!parsed.segments.every((s) => s.byterange && s.byterange.length > 0)) return 0;
  const seg = parsed.segments.reduce((sum, s) => sum + s.byterange.length, 0);
  return seg + (Number(parsed.mapBytes) || 0);
}

/** Indexes of segments to sample for Content-Length (first, middle, last). */
export function sampleSegmentIndexes(count) {
  const n = Number(count) || 0;
  if (n <= 0) return [];
  if (n <= 3) return [...Array(n).keys()];
  return [0, Math.floor(n / 2), n - 1];
}

/**
 * Turn sampled segment sizes into a playlist total.
 * Prefers bytes/second × real duration so a short last segment does not skew the total.
 */
export function extrapolatePlaylistBytes(parsed, samples) {
  const exact = sizeFromByteranges(parsed);
  if (exact > 0) return exact;
  const good = (samples || []).filter((s) => s && s.bytes > 0);
  if (!good.length || !parsed?.segments?.length) return 0;
  const byteSum = good.reduce((sum, s) => sum + s.bytes, 0);
  const durSum = good.reduce((sum, s) => sum + (Number(s.duration) || 0), 0);
  const mapBytes = Number(parsed.mapBytes) || 0;
  if (byteSum > 0 && durSum > 0 && parsed.duration > 0) {
    return Math.round((byteSum / durSum) * parsed.duration) + mapBytes;
  }
  const avg = byteSum / good.length;
  return Math.round(avg * parsed.segments.length) + mapBytes;
}

/** Default audio rendition URI for each EXT-X-MEDIA GROUP-ID. */
export function parseHlsAudioGroups(text, baseUrl) {
  const groups = new Map();
  const re = /#EXT-X-MEDIA:([^\n\r]*)/gi;
  let m;
  while ((m = re.exec(text || ''))) {
    const attrs = m[1] || '';
    if (!/TYPE=AUDIO/i.test(attrs)) continue;
    const id = /GROUP-ID="([^"]+)"/i.exec(attrs);
    const uri = /URI="([^"]+)"/i.exec(attrs);
    if (!id || !uri) continue;
    let abs = uri[1];
    try {
      abs = new URL(uri[1], baseUrl).href;
    } catch {
      /* keep */
    }
    const isDefault = /DEFAULT=YES/i.test(attrs);
    const prev = groups.get(id[1]);
    if (!prev || (isDefault && !prev.def)) {
      groups.set(id[1], { url: abs, def: isDefault });
    }
  }
  const out = new Map();
  for (const [id, v] of groups) out.set(id, v.url);
  return out;
}

/** Sum #EXTINF durations from a media playlist. */
export function parseM3u8Duration(text) {
  if (!text) return 0;
  let total = 0;
  const re = /#EXTINF:\s*([\d.]+)/gi;
  let m;
  while ((m = re.exec(text))) {
    total += Number(m[1]) || 0;
  }
  if (total > 0) return total;
  const target = /#EXT-X-TARGETDURATION:(\d+)/i.exec(text);
  const count = (text.match(/#EXTINF:/gi) || []).length;
  if (target && count) return Number(target[1]) * count;
  return 0;
}

/**
 * Parse HLS master playlist → variants with bandwidth / resolution.
 * Optionally expand each quality into TS + MP4 rows (IDM style).
 */
export function parseM3u8Master(text, baseUrl, { expandContainers = true, duration = 0 } = {}) {
  const lines = text.split(/\r?\n/);
  const variants = [];
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i].trim();
    if (!line.startsWith('#EXT-X-STREAM-INF:')) continue;
    const info = line.slice('#EXT-X-STREAM-INF:'.length);
    const res = /RESOLUTION=(\d+)x(\d+)/i.exec(info);
    const bw = /BANDWIDTH=(\d+)/i.exec(info);
    const avgBw = /AVERAGE-BANDWIDTH=(\d+)/i.exec(info);
    const name = /NAME="([^"]+)"/i.exec(info);
    const codecs = /CODECS="([^"]+)"/i.exec(info);
    let next = '';
    for (let j = i + 1; j < lines.length; j += 1) {
      const t = lines[j].trim();
      if (!t || t.startsWith('#')) continue;
      next = t;
      break;
    }
    if (!next) continue;
    let abs = next;
    try {
      abs = new URL(next, baseUrl).href;
    } catch {
      /* keep */
    }
    const width = res ? Number(res[1]) : 0;
    const height = res ? Number(res[2]) : 0;
    const bandwidth = Number((avgBw && avgBw[1]) || (bw && bw[1]) || 0);
    const audioAttr = /AUDIO="([^"]+)"/i.exec(info);
    const label =
      (name && name[1]) ||
      guessResolution({ width, height, url: abs }) ||
      'Default';
    /* Size is filled later from real segment lengths. BANDWIDTH is a peak
       ceiling, so peak × duration (the old 4.0 GB figure) is not the file size. */
    const base = {
      url: abs,
      masterUrl: baseUrl,
      label: normalizeResLabel(label),
      width,
      height,
      bandwidth,
      bitrate: bandwidth,
      duration,
      size: 0,
      sizeLabel: '',
      codecs: codecs ? codecs[1] : '',
      audioGroup: audioAttr ? audioAttr[1] : '',
      type: 'hls',
      progressive: false,
    };
    if (expandContainers) {
      variants.push({
        ...base,
        container: 'TS',
        ext: 'ts',
        detail: idmDetailLine({ ...base, container: 'TS' }),
      });
      variants.push({
        ...base,
        container: 'MP4',
        ext: 'mp4',
        detail: idmDetailLine({ ...base, container: 'MP4' }),
      });
    } else {
      variants.push({
        ...base,
        container: 'TS',
        ext: 'ts',
        detail: idmDetailLine({ ...base, container: 'TS' }),
      });
    }
  }
  return variants;
}

/** Lightweight DASH MPD parse for Representation bandwidth/height. */
export function parseDashManifest(text, baseUrl, { expandContainers = true, duration = 0 } = {}) {
  if (!text || !/<MPD[\s>]/i.test(text)) return [];
  let dur = duration;
  if (!dur) {
    const dm = /mediaPresentationDuration="PT(?:(\d+)H)?(?:(\d+)M)?(?:([\d.]+)S)?"/i.exec(text);
    if (dm) {
      dur =
        (Number(dm[1]) || 0) * 3600 +
        (Number(dm[2]) || 0) * 60 +
        (Number(dm[3]) || 0);
    }
  }
  const variants = [];
  const re =
    /<Representation\b([^>]*)>(?:[\s\S]*?<BaseURL>([^<]+)<\/BaseURL>)?/gi;
  let m;
  while ((m = re.exec(text))) {
    const attrs = m[1] || '';
    const baseRel = (m[2] || '').trim();
    const id = /id="([^"]+)"/i.exec(attrs);
    const bw = /bandwidth="(\d+)"/i.exec(attrs);
    const width = /width="(\d+)"/i.exec(attrs);
    const height = /height="(\d+)"/i.exec(attrs);
    const mime = /mimeType="([^"]+)"/i.exec(attrs);
    if (mime && /audio\//i.test(mime[1]) && !height) continue;
    const bandwidth = bw ? Number(bw[1]) : 0;
    const w = width ? Number(width[1]) : 0;
    const h = height ? Number(height[1]) : 0;
    if (!h && !bandwidth) continue;
    let abs = baseUrl;
    if (baseRel) {
      try {
        abs = new URL(baseRel, baseUrl).href;
      } catch {
        abs = baseUrl;
      }
    }
    const label = guessResolution({ width: w, height: h, url: abs });
    const size = dur > 0 && bandwidth > 0 ? Math.round((bandwidth * dur) / 8) : 0;
    const base = {
      url: abs,
      masterUrl: baseUrl,
      label: normalizeResLabel(label),
      width: w,
      height: h,
      bandwidth,
      bitrate: bandwidth,
      duration: dur,
      size,
      sizeLabel: size ? formatBytes(size) : '',
      mime: mime ? mime[1] : '',
      type: 'dash',
      formatId: id ? id[1] : '',
      progressive: false,
    };
    if (expandContainers) {
      variants.push({
        ...base,
        container: 'MP4',
        ext: 'mp4',
        detail: idmDetailLine({ ...base, container: 'MP4' }),
      });
    } else {
      variants.push({
        ...base,
        container: containerFromUrlOrMime(abs, base.mime) || 'MP4',
        detail: idmDetailLine(base),
      });
    }
  }
  return variants;
}

/** Attach IDM detail line + defaults for menu rows. */
export function enrichVariantForMenu(item, { title = '', duration = 0 } = {}) {
  if (!item) return item;
  const url = item.url || '';
  const yt = youtubeItagInfo(url);
  const container =
    item.container ||
    (item.processor
      ? item.audio
        ? 'M4A'
        : 'MP4'
      : containerFromUrlOrMime(url, item.mime) ||
        (yt?.ext ? yt.ext.toUpperCase() : '') ||
        (item.type === 'hls' ? 'TS' : item.ext ? String(item.ext).toUpperCase() : 'MP4'));
  const height =
    item.height ||
    heightFromMediaUrl(url) ||
    Number(String(item.quality || item.label || '').replace(/\D/g, '')) ||
    0;
  const dur = Number(item.duration || duration || 0);
  const bandwidth =
    Number(item.bandwidth || item.bitrate || 0) ||
    (dur > 0 && item.size ? Math.round((Number(item.size) * 8) / dur) : 0);
  const enriched = {
    ...item,
    title: item.title || title || '',
    container,
    height: height || item.height || 0,
    duration: dur,
    bandwidth,
    bitrate: bandwidth,
    detail:
      item.detail ||
      idmDetailLine({
        ...item,
        container,
        height: height || item.height,
        duration: dur,
        bandwidth,
      }),
  };
  return enriched;
}

/**
 * Dedupe + prefer progressive YouTube muxed streams over DASH video-only
 * when the same resolution exists. Keeps TS/MP4 container pairs (IDM style).
 */
export function dedupeVariants(items) {
  const map = new Map();
  for (const item of items) {
    if (!item?.url) continue;
    const url = canonicalizeMediaUrl(item.url);
    const yt = youtubeItagInfo(url);
    const container = item.container || item.ext || '';
    const key = `${mediaDedupeKey(url)}|${container || 'default'}|${item.processor ? item.quality : ''}`;
    const enriched = {
      ...item,
      url,
      label: item.label && item.label !== 'Default' ? item.label : guessResolution({ ...item, url }),
      size: item.size || yt?.size || 0,
      progressive: yt ? !!yt.progressive : !!item.progressive,
      audio: yt ? !!yt.audio : !!item.audio,
      ext: yt?.ext || item.ext || '',
      itag: yt?.itag,
      container: item.container || (yt?.ext ? yt.ext.toUpperCase() : container),
    };
    const prev = map.get(key);
    if (!prev) {
      map.set(key, enriched);
      continue;
    }
    const better =
      (enriched.progressive && !prev.progressive) ||
      ((enriched.size || 0) > (prev.size || 0) && enriched.progressive === prev.progressive);
    if (better) map.set(key, enriched);
  }

  /* Collapse same resolution+container: keep progressive over adaptive.
   * HLS keeps separate bandwidth / URL rows (IDM shows many near-duplicates). */
  const byRes = new Map();
  for (const item of map.values()) {
    if (item.processor) {
      byRes.set(`proc:${item.quality}:${item.container || item.ext}`, item);
      continue;
    }
    if (item.audio) {
      byRes.set(`audio:${item.ext}:${item.itag || item.url}`, item);
      continue;
    }
    const isHls =
      item.type === 'hls' ||
      item.type === 'dash' ||
      /\.m3u8(\?|$)/i.test(item.url || '') ||
      /\.mpd(\?|$)/i.test(item.url || '');
    if (isHls) {
      const bwBucket = Math.round((Number(item.bandwidth) || 0) / 50_000);
      byRes.set(
        `hls:${mediaDedupeKey(item.url)}|${item.container || item.ext || 'file'}|${bwBucket}|${item.height || 0}`,
        item
      );
      continue;
    }
    const res = normalizeResLabel(item.label).replace(/\s+.*/, '');
    const resKey = `${res}|${item.container || item.ext || 'file'}`;
    const prev = byRes.get(resKey);
    if (!prev) {
      byRes.set(resKey, item);
      continue;
    }
    if (item.progressive && !prev.progressive) byRes.set(resKey, item);
    else if (item.progressive === prev.progressive && (item.size || 0) > (prev.size || 0)) {
      byRes.set(resKey, item);
    }
  }

  return [...byRes.values()].sort((a, b) => {
    if (a.audio !== b.audio) return a.audio ? 1 : -1;
    if (!!a.processor !== !!b.processor) return a.processor ? -1 : 1;
    const ha =
      Number(a.height) ||
      Number(String(a.quality || a.label || '').replace(/\D/g, '')) ||
      resolutionSortKey(a.label);
    const hb =
      Number(b.height) ||
      Number(String(b.quality || b.label || '').replace(/\D/g, '')) ||
      resolutionSortKey(b.label);
    if (hb !== ha) return hb - ha;
    /* Higher bitrate first within same height (IDM lists multiple 1080p) */
    const ba = Number(a.bandwidth || a.bitrate || 0);
    const bb = Number(b.bandwidth || b.bitrate || 0);
    if (bb !== ba) return bb - ba;
    /* TS before MP4 within same quality (IDM order) */
    const ca = String(a.container || '');
    const cb = String(b.container || '');
    if (ca === 'TS' && cb !== 'TS') return -1;
    if (cb === 'TS' && ca !== 'TS') return 1;
    return 0;
  });
}

/** Skip tiny CDN chunks that are not full representations. */
export function shouldKeepMediaHit(url, size = 0) {
  if (!url) return false;
  if (/favicon|sprite|tracking|analytics|pixel|beacon/i.test(url)) return false;
  const yt = youtubeItagInfo(url);
  if (yt) return true; /* full stream identified by itag */

  /* Prefer playlists over individual HLS/DASH fragments */
  if (/\.m3u8(\?|$)/i.test(url) || /\.mpd(\?|$)/i.test(url)) return true;
  if (/[?&](m3u8|mpd|playlist|hls)/i.test(url)) return true;
  if (/\/(master|index|playlist|manifest)\.m3u8/i.test(url)) return true;
  if (/\/(hls|dash|playlist|master|manifest)\b/i.test(url)) return true;

  /* Drop media segments (proxied or direct) — they flood the menu */
  if (
    /\.m4s(\?|$)/i.test(url) ||
    /\/seg-\d+/i.test(url) ||
    /[?&]url=.*\.(ts|m4s)(%|\?|&|$)/i.test(url) ||
    (/\.ts(\?|$)/i.test(url) && !/\.m3u8/i.test(url))
  ) {
    return false;
  }
  /* /litro/ carries every HLS segment on these players. Keep playlists and full files only. */
  if (/\/litro\b/i.test(url) && !/\.(m3u8|mpd|mp4|m4v|webm)(\?|$)/i.test(url)) {
    return false;
  }

  if (size > 0 && size < 128 * 1024) return false;
  return isLikelyMediaUrl(url);
}
