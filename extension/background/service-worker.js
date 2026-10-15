/**
 * Background service worker — IDM-style download interception & routing.
 */
import {
  loadSettings,
  shouldCapture,
  filenameFromUrl,
} from '../shared/config.js';
import {
  captureToDaemon,
  checkDaemon,
  cookiesForUrl,
  probeYoutube,
  estimateSizeFromProbe,
  bitrateFromProbe,
  pickFormatFromProbe,
} from '../shared/api.js';
import {
  isLikelyMediaUrl,
  isMediaMime,
  guessResolution,
  parseM3u8Master,
  parseM3u8Duration,
  parseM3u8Media,
  extrapolatePlaylistBytes,
  sampleSegmentIndexes,
  parseHlsAudioGroups,
  parseDashManifest,
  dedupeVariants,
  enrichVariantForMenu,
  formatBytes,
  idmDetailLine,
  pageTitleFilename,
  qualityDisplayLabel,
  heightFromMediaUrl,
  canonicalizeMediaUrl,
  sanitizeByteLength,
  shouldKeepMediaHit,
  youtubeItagInfo,
  youtubeProcessorVariants,
  variantsFromPlayerResponse,
  variantsFromQualityLevels,
  extractPlayerResponseFromHtml,
  attachSniffedStreams,
  isGooglevideoStreamUrl,
  clenFromUrl,
} from '../shared/media.js';

/** Recent URLs we already redirected (avoid loops / double prompts). */
const recentCaptures = new Map();
/** URLs the user explicitly chose to open in the browser. */
const browserAllow = new Map();
/** tabId -> Map(url -> media item) sniffed from network */
const tabMedia = new Map();
const RECENT_TTL_MS = 15_000;
const MEDIA_TTL_MS = 10 * 60_000;
const ICON = () => chrome.runtime.getURL('icons/icon128.png');
/** yt-dlp probe cache: watchUrl -> { info, ts, inflight } */
const youtubeProbeCache = new Map();
/** Instant page scrape cache: watchUrl -> { items, title, duration, ts } */
const youtubePageCache = new Map();
const YT_PROBE_TTL_MS = 10 * 60_000;
const YT_PAGE_TTL_MS = 10 * 60_000;

function markRecent(url) {
  recentCaptures.set(url, Date.now());
}

function wasRecent(url) {
  const t = recentCaptures.get(url);
  if (!t) return false;
  if (Date.now() - t > RECENT_TTL_MS) {
    recentCaptures.delete(url);
    return false;
  }
  return true;
}

function allowBrowserOnce(url) {
  browserAllow.set(url, Date.now());
}

function isBrowserAllowed(url) {
  const t = browserAllow.get(url);
  if (!t) return false;
  browserAllow.delete(url);
  return Date.now() - t < RECENT_TTL_MS;
}

function getYoutubeProbeCached(watchUrl) {
  const hit = youtubeProbeCache.get(watchUrl);
  if (!hit?.info) return null;
  if (Date.now() - hit.ts > YT_PROBE_TTL_MS) {
    youtubeProbeCache.delete(watchUrl);
    return null;
  }
  return hit.info;
}

function getYoutubePageCached(watchUrl) {
  const hit = youtubePageCache.get(watchUrl);
  if (!hit?.items?.length) return null;
  if (Date.now() - hit.ts > YT_PAGE_TTL_MS) {
    youtubePageCache.delete(watchUrl);
    return null;
  }
  return hit;
}

function rememberYoutubePage(watchUrl, parsed) {
  if (!watchUrl || !parsed?.items?.length) return;
  youtubePageCache.set(watchUrl, {
    items: parsed.items,
    title: parsed.title || '',
    duration: parsed.duration || 0,
    ts: Date.now(),
  });
}

/** Merge live sniffed CDN URLs into quality rows (IDM / online-downloader handoff). */
function withSniffedStreams(tabId, parsed) {
  if (!parsed?.items?.length) return parsed;
  const sniffed = tabId != null ? listTabMedia(tabId) : [];
  const items = attachSniffedStreams(parsed.items, sniffed);
  return { ...parsed, items };
}

/** Keep page resolution rows; fill in sizes/bitrate from yt-dlp when available. */
function mergeProbeSizesIntoPageItems(pageItems, info) {
  if (!pageItems?.length) return pageItems || [];
  if (!info?.formats?.length) return pageItems;
  return pageItems.map((v) => {
    const size = estimateSizeFromProbe(info, v.quality) || Number(v.size) || 0;
    const bandwidth = bitrateFromProbe(info, v.quality) || Number(v.bandwidth) || 0;
    const title = pageTitleFilename(info.title || v.title || '');
    const duration = Number(info.duration) || Number(v.duration) || 0;
    return enrichVariantForMenu(
      {
        ...v,
        title: title || v.title,
        size,
        sizeLabel: size ? formatBytes(size) : v.sizeLabel || '',
        duration,
        bandwidth,
        bitrate: bandwidth,
      },
      { title: title || v.title, duration }
    );
  });
}

/** Drop quality buckets the probe never actually saw (avoid 8× fake 360p rows). */
function filterAvailableYoutubeItems(items, info) {
  if (!items?.length) return [];
  if (!info?.formats?.length) return items;
  const heights = new Set();
  for (const f of info.formats) {
    const h = Number(f.height) || 0;
    if (h) heights.add(h);
  }
  if (!heights.size) return items;
  const maxH = Math.max(...heights);
  return items.filter((v) => {
    if (v.audio || v.quality === 'best' || v.quality === 'audio') return true;
    const q = Number(String(v.quality).replace(/\D/g, '')) || Number(v.height) || 0;
    if (!q) return true;
    /* Keep bucket if probe has this height or any higher (yt-dlp can still target it) */
    if (heights.has(q)) return true;
    if (q <= maxH && [...heights].some((h) => h >= q - 8 && h <= q + 8)) return true;
    /* Only keep standard ladders that exist at or below max available */
    return q <= maxH && [2160, 1440, 1080, 720, 480, 360, 240, 144].includes(q) &&
      [...heights].some((h) => h >= q);
  });
}

/** Read player quality levels + ytInitialPlayerResponse (MAIN world) — IDM-speed path. */
async function scrapeYoutubePlayerFromTab(tabId, pageUrl, { bypassCache = false } = {}) {
  const watch = normalizeWatchOrEmpty(pageUrl);
  if (!watch) return null;

  if (!bypassCache) {
    const cached = getYoutubePageCached(watch);
    /* Prefer quality_levels cache; allow re-scrape if we only got a weak progressive hit */
    if (cached?.items?.length) {
      const maxH = Math.max(0, ...cached.items.map((i) => Number(i.height) || 0));
      const fromLevels = cached.items.some((i) => i.source === 'quality_levels' || i.source === 'sniffed' || i.stream);
      if (fromLevels || maxH >= 720) return withSniffedStreams(tabId, cached);
    }
  }

  let scraped = null;
  if (tabId != null && tabId >= 0) {
    try {
      const results = await chrome.scripting.executeScript({
        target: { tabId },
        world: 'MAIN',
        func: () => {
          const out = { playerResponse: null, qualityLevels: [] };
          try {
            const movie = document.getElementById('movie_player');
            if (movie && typeof movie.getAvailableQualityLevels === 'function') {
              out.qualityLevels = movie.getAvailableQualityLevels() || [];
            }
            if (movie && typeof movie.getPlayerResponse === 'function') {
              out.playerResponse = movie.getPlayerResponse();
            }
          } catch {
            /* ignore */
          }
          if (!out.playerResponse) {
            try {
              if (window.ytInitialPlayerResponse?.streamingData) {
                out.playerResponse = window.ytInitialPlayerResponse;
              }
            } catch {
              /* ignore */
            }
          }
          if (!out.playerResponse) {
            try {
              const raw = window.ytplayer?.config?.args?.player_response;
              if (typeof raw === 'string') out.playerResponse = JSON.parse(raw);
              else if (raw && typeof raw === 'object') out.playerResponse = raw;
            } catch {
              /* ignore */
            }
          }
          if (!out.qualityLevels?.length) {
            try {
              const ytd = document.querySelector('ytd-player');
              const levels = ytd?.player_?.getAvailableQualityLevels?.();
              if (levels?.length) out.qualityLevels = levels;
              if (!out.playerResponse) {
                out.playerResponse = ytd?.player_?.getPlayerResponse?.() || null;
              }
            } catch {
              /* ignore */
            }
          }
          return out;
        },
      });
      scraped = results?.[0]?.result || null;
    } catch {
      /* tab may be restricted */
    }
  }

  let player = scraped?.playerResponse || null;
  const levels = scraped?.qualityLevels || [];

  if (!player) {
    try {
      const results = await chrome.scripting.executeScript({
        target: { tabId },
        world: 'ISOLATED',
        func: () => document.documentElement?.innerHTML || '',
      });
      const html = results?.[0]?.result || '';
      player = extractPlayerResponseFromHtml(html);
    } catch {
      /* ignore */
    }
  }

  /* Prefer live quality menu (matches YouTube UI exactly) */
  let parsed = levels.length
    ? variantsFromQualityLevels(levels, player, watch)
    : null;
  if (!parsed?.items?.length) {
    parsed = variantsFromPlayerResponse(player, watch);
  }
  if (!parsed?.items?.length) return null;
  rememberYoutubePage(watch, parsed);
  const cached = getYoutubePageCached(watch);
  return withSniffedStreams(tabId, cached);
}

function normalizeWatchOrEmpty(pageUrl) {
  const base = youtubeProcessorVariants(pageUrl || '');
  return base[0]?.url || '';
}

/** Start or reuse an in-flight yt-dlp probe (never blocks callers by itself). */
function ensureYoutubeProbe(watchUrl) {
  const hit = youtubeProbeCache.get(watchUrl);
  if (hit?.info && Date.now() - hit.ts <= YT_PROBE_TTL_MS) {
    return Promise.resolve(hit.info);
  }
  if (hit?.inflight) return hit.inflight;

  const inflight = probeYoutube(watchUrl)
    .then((info) => {
      youtubeProbeCache.set(watchUrl, { info, ts: Date.now(), inflight: null });
      return info;
    })
    .catch((err) => {
      const cur = youtubeProbeCache.get(watchUrl);
      if (cur?.inflight === inflight) {
        youtubeProbeCache.delete(watchUrl);
      }
      throw err;
    });

  youtubeProbeCache.set(watchUrl, {
    info: hit?.info || null,
    ts: hit?.ts || 0,
    inflight,
  });
  return inflight;
}

function buildYoutubeMenuItems(baseItems, info, pageTitle = '', pageDuration = 0) {
  const duration = Number(info?.duration) || pageDuration || 0;
  const title = pageTitleFilename(info?.title || pageTitle || '');
  return baseItems.map((v) => {
    const requested =
      v.audio || String(v.quality).toLowerCase() === 'audio'
        ? 0
        : String(v.quality).toLowerCase() === 'best'
          ? 99999
          : Number(String(v.quality).replace(/\D/g, '')) || 0;
    const size = info ? estimateSizeFromProbe(info, v.quality) : Number(v.size) || 0;
    const bandwidth = info ? bitrateFromProbe(info, v.quality) : Number(v.bandwidth) || 0;
    const fmt = info ? pickFormatFromProbe(info, v.quality) : null;
    /* Never let a lower picked format relabel a higher quality bucket as 360p etc. */
    const height =
      v.audio || requested === 0
        ? 0
        : requested >= 99999
          ? Number(fmt?.height) || Number(v.height) || 0
          : requested;
    const container = v.audio ? 'M4A' : 'MP4';
    return enrichVariantForMenu(
      {
        ...v,
        title,
        size: size || 0,
        sizeLabel: size ? formatBytes(size) : '',
        duration,
        bandwidth,
        bitrate: bandwidth,
        height,
        label: v.audio
          ? 'Audio only'
          : v.quality === 'best'
            ? 'Best'
            : qualityDisplayLabel(height || v.label) || v.label,
        container,
        ext: v.audio ? 'm4a' : 'mp4',
      },
      { title, duration }
    );
  });
}

chrome.runtime.onInstalled.addListener(async () => {
  await setupContextMenus();
  const health = await checkDaemon();
  if (!health.ok) {
    chrome.notifications.create('dm-daemon-missing', {
      type: 'basic',
      iconUrl: ICON(),
      title: 'Download Manager',
      message: 'Download Manager Pro is not running. Open it to capture downloads.',
    });
  }
});

chrome.runtime.onStartup.addListener(() => {
  setupContextMenus();
});

async function setupContextMenus() {
  await chrome.contextMenus.removeAll();
  chrome.contextMenus.create({
    id: 'dm-download-link',
    title: 'Download with Download Manager',
    contexts: ['link'],
  });
  chrome.contextMenus.create({
    id: 'dm-download-media',
    title: 'Download media with Download Manager',
    contexts: ['image', 'video', 'audio'],
  });
  chrome.contextMenus.create({
    id: 'dm-download-page',
    title: 'Download page / selection with DM',
    contexts: ['page', 'selection'],
  });
}

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  let url = null;
  if (info.menuItemId === 'dm-download-link') url = info.linkUrl;
  else if (info.menuItemId === 'dm-download-media') {
    url = info.srcUrl || info.linkUrl;
  } else if (info.menuItemId === 'dm-download-page') {
    url = info.selectionText?.match(/https?:\/\/\S+/)?.[0] || info.pageUrl;
  }
  if (!url) return;

  const settings = await loadSettings();
  if (settings.captureMode === 'auto') {
    await safeCapture(url, { ...settings, pageUrl: tab?.url || '', referer: tab?.url || '' }, tab?.id);
  } else {
    await openCaptureDialog(url, tab?.id, {
      pageUrl: tab?.url || '',
      referer: tab?.url || '',
    });
  }
});

/**
 * Sniff media responses for the IDM-style video banner / resolution menu.
 */
chrome.webRequest.onHeadersReceived.addListener(
  (details) => {
    if (details.tabId < 0) return;
    const mime = headerValue(details.responseHeaders, 'content-type');
    const len = Number(headerValue(details.responseHeaders, 'content-length') || 0);
    const raw = details.url;
    if (!isMediaMime(mime) && !isLikelyMediaUrl(raw)) return;

    const url = canonicalizeMediaUrl(raw);
    const yt = youtubeItagInfo(url);
    /* A playlist's own Content-Length is the manifest, not the movie.
       2^32 here is what the download list showed as 4.0 GB. */
    const manifest =
      /\.m3u8(\?|$)/i.test(url) ||
      /\.mpd(\?|$)/i.test(url) ||
      /mpegurl|dash\+xml/i.test(mime || '');
    const size = manifest ? 0 : sanitizeByteLength(yt?.size || len || 0);
    if (!shouldKeepMediaHit(url, size)) return;

    rememberTabMedia(details.tabId, {
      url,
      mime: mime || yt?.mime || '',
      size,
      label: guessResolution({ url, size }),
      progressive: yt ? !!yt.progressive : false,
      audio: yt ? !!yt.audio : false,
      itag: yt?.itag,
      ts: Date.now(),
    });

    /* Async-enrich HLS/DASH manifests with IDM-style variant details */
    const looksHls =
      /\.m3u8(\?|$)/i.test(url) ||
      /mpegurl/i.test(mime || '') ||
      /[?&](m3u8|playlist|hls)/i.test(url) ||
      /\/(hls|playlist|master)(\/|\?|$)/i.test(url);
    const looksDash =
      /\.mpd(\?|$)/i.test(url) ||
      /dash\+xml/i.test(mime || '') ||
      /[?&]mpd=/i.test(url);
    if (looksHls) {
      enrichPlaylistIntoTab(details.tabId, url, 'hls').catch(() => {});
    } else if (looksDash) {
      enrichPlaylistIntoTab(details.tabId, url, 'dash').catch(() => {});
    }

    chrome.tabs.sendMessage(details.tabId, { type: 'MEDIA_UPDATED' }).catch(() => {});

    /* If this is a YouTube CDN hit, refresh any open quality menu with the stream URL */
    if (yt || /googlevideo\.com/i.test(url)) {
      chrome.tabs
        .get(details.tabId)
        .then((tab) => {
          if (!tab?.url || !/youtube\.com|youtu\.be/i.test(tab.url)) return;
          const watch = youtubeProcessorVariants(tab.url)[0]?.url;
          if (!watch) return;
          const hit = getYoutubePageCached(watch);
          if (!hit?.items?.length) return;
          const items = attachSniffedStreams(hit.items, listTabMedia(details.tabId));
          chrome.tabs
            .sendMessage(details.tabId, {
              type: 'YOUTUBE_PROBE_READY',
              pageUrl: tab.url,
              items,
            })
            .catch(() => {});
        })
        .catch(() => {});
    }
  },
  { urls: ['http://*/*', 'https://*/*'] },
  ['responseHeaders']
);

async function enrichPlaylistIntoTab(tabId, url, kind) {
  try {
    const parsed =
      kind === 'dash' ? await fetchDashVariants(url, 0) : await fetchM3u8Variants(url, 0);
    for (const v of parsed) {
      rememberTabMedia(tabId, {
        ...v,
        label: v.label || guessResolution(v),
        sizeLabel: v.sizeLabel || formatBytes(v.size || 0),
        detail: v.detail || idmDetailLine(v),
        ts: Date.now(),
      });
    }
    if (parsed.length) {
      chrome.tabs.sendMessage(tabId, { type: 'MEDIA_UPDATED' }).catch(() => {});
    }
  } catch {
    /* ignore */
  }
}

chrome.tabs.onRemoved.addListener((tabId) => {
  tabMedia.delete(tabId);
});

function headerValue(headers, name) {
  if (!headers) return '';
  const h = headers.find((x) => x.name.toLowerCase() === name.toLowerCase());
  return h?.value || '';
}

function rememberTabMedia(tabId, item) {
  let map = tabMedia.get(tabId);
  if (!map) {
    map = new Map();
    tabMedia.set(tabId, map);
  }
  const key = item.container
    ? `${item.url}|${item.container}|${item.quality || ''}`
    : item.url;
  const prev = map.get(key);
  const nextSize = sanitizeByteLength(item.size);
  const prevSize = sanitizeByteLength(prev?.size);
  const size = nextSize > 0 ? nextSize : prevSize;
  map.set(key, {
    ...prev,
    ...item,
    size,
    sizeLabel: size ? item.sizeLabel || prev?.sizeLabel || formatBytes(size) : '',
    ts: Date.now(),
  });
  const now = Date.now();
  for (const [u, v] of map) {
    if (now - v.ts > MEDIA_TTL_MS) map.delete(u);
  }
}

function listTabMedia(tabId) {
  const map = tabMedia.get(tabId);
  if (!map) return [];
  const now = Date.now();
  const out = [];
  for (const item of map.values()) {
    if (now - item.ts > MEDIA_TTL_MS) continue;
    out.push(item);
  }
  return dedupeVariants(
    out.map((m) => {
      const size = sanitizeByteLength(m.size);
      return {
      url: m.url,
      label: m.label || guessResolution(m),
      size,
      sizeLabel: size ? m.sizeLabel || formatBytes(size) : '',
      mime: m.mime || '',
      progressive: !!m.progressive,
      audio: !!m.audio,
      container: m.container || '',
      ext: m.ext || '',
      height: m.height || 0,
      duration: m.duration || 0,
      bandwidth: m.bandwidth || m.bitrate || 0,
      type:
        m.type ||
        (/\.m3u8(\?|$)/i.test(m.url) || /\/hls\b/i.test(m.url) ? 'hls' : 'file'),
      detail: idmDetailLine({
        ...m,
        size,
        sizeLabel: size ? formatBytes(size) : '',
      }),
    };
    })
  );
}

/**
 * Intercept native browser downloads (core IDM Integration Module behavior).
 */
chrome.downloads.onCreated.addListener(async (item) => {
  const settings = await loadSettings();
  if (!settings.enabled || !settings.interceptBrowserDownloads) return;
  if (settings.captureMode === 'off') return;

  const url = item.finalUrl || item.url;
  if (!url || url.startsWith('blob:') || url.startsWith('data:')) return;
  if (isBrowserAllowed(url) || wasRecent(url)) return;
  if (!shouldCapture(url, settings)) return;

  try {
    await chrome.downloads.cancel(item.id);
    await chrome.downloads.erase({ id: item.id });
  } catch {
    /* already gone */
  }

  markRecent(url);

  if (settings.captureMode === 'auto') {
    await safeCapture(url, settings, item.tabId >= 0 ? item.tabId : undefined);
  } else {
    await openCaptureDialog(url, item.tabId >= 0 ? item.tabId : undefined);
  }
});

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  handleMessage(msg, sender)
    .then(sendResponse)
    .catch((err) => sendResponse({ ok: false, error: err.message }));
  return true;
});

async function handleMessage(msg, sender) {
  const settings = await loadSettings();

  switch (msg.type) {
    case 'GET_SETTINGS':
      return { ok: true, settings };

    case 'SAVE_SETTINGS':
      await chrome.storage.sync.set({ settings: msg.settings });
      return { ok: true };

    case 'CHECK_DAEMON':
      return { ok: true, ...(await checkDaemon()) };

    case 'CAPTURE_URL': {
      const url = msg.url;
      if (!url) return { ok: false, error: 'Missing URL' };
      if (msg.mode === 'browser') {
        allowBrowserOnce(url);
        /* Trigger a normal browser download without our interceptor stealing it. */
        await chrome.downloads.download({ url, saveAs: false });
        return { ok: true, browser: true };
      }
      markRecent(url);
      const measured = await captureByteLength(url, msg.totalBytes || msg.size || 0);
      const result = await captureToDaemon(url, {
        ...settings,
        segments: msg.segments ?? settings.segments,
        saveDir: msg.saveDir || settings.saveDir,
        pageUrl: msg.pageUrl || sender.tab?.url || '',
        referer: msg.referer || sender.tab?.url || '',
        filename: msg.filename || '',
        title: msg.title || '',
        label: msg.label || '',
        quality: msg.quality || '',
        format: msg.format || '',
        totalBytes: measured,
        size: measured,
      });
      notifyCaptured(url);
      return { ok: true, id: result.id };
    }

    case 'OPEN_CAPTURE': {
      const measured = await captureByteLength(msg.url, msg.totalBytes || msg.size || 0);
      await openCaptureDialog(msg.url, sender.tab?.id, {
        pageUrl: msg.pageUrl || sender.tab?.url || '',
        referer: msg.referer || sender.tab?.url || '',
        label: msg.label || '',
        filename: msg.filename || '',
        quality: msg.quality || '',
        title: msg.title || '',
        size: measured,
        totalBytes: measured,
      });
      return { ok: true };
    }

    case 'GET_YOUTUBE_VARIANTS': {
      const page = msg.pageUrl || sender.tab?.url || '';
      const pageTitle = pageTitleFilename(msg.title || '');
      const pageDuration = Number(msg.duration) || 0;
      const fast = msg.fast !== false; /* default: don't block on probe */
      const base = youtubeProcessorVariants(page);
      if (!base.length) return { ok: true, items: [] };

      const watch = base[0].url;
      const tabId = sender.tab?.id;

      /* Always re-read live quality levels so menu matches YouTube Settings */
      let pageHit =
        tabId != null
          ? await scrapeYoutubePlayerFromTab(tabId, page, { bypassCache: true })
          : getYoutubePageCached(watch);

      if (pageHit?.items?.length) {
        ensureYoutubeProbe(watch)
          .then((info) => {
            if (tabId == null) return;
            /* Polish sizes only — keep the resolution list from the player */
            const polished = mergeProbeSizesIntoPageItems(pageHit.items, info);
            chrome.tabs
              .sendMessage(tabId, {
                type: 'YOUTUBE_PROBE_READY',
                pageUrl: page,
                items: polished,
              })
              .catch(() => {});
          })
          .catch(() => {});
        return {
          ok: true,
          items: pageHit.items,
          fromPage: true,
          title: pageHit.title || pageTitle,
          duration: pageHit.duration || pageDuration,
        };
      }

      const cached = getYoutubeProbeCached(watch);

      if (cached) {
        return {
          ok: true,
          items: filterAvailableYoutubeItems(
            buildYoutubeMenuItems(base, cached, pageTitle, pageDuration),
            cached
          ),
          fromCache: true,
        };
      }

      /* Kick off probe in background; return instant rows for the menu */
      const inflight = ensureYoutubeProbe(watch);
      if (!fast) {
        try {
          const info = await inflight;
          return {
            ok: true,
            items: filterAvailableYoutubeItems(
              buildYoutubeMenuItems(base, info, pageTitle, pageDuration),
              info
            ),
          };
        } catch {
          /* fall through to static */
        }
      } else {
        inflight
          .then((info) => {
            if (tabId == null) return;
            chrome.tabs
              .sendMessage(tabId, {
                type: 'YOUTUBE_PROBE_READY',
                pageUrl: page,
                items: filterAvailableYoutubeItems(
                  buildYoutubeMenuItems(base, info, pageTitle, pageDuration),
                  info
                ),
              })
              .catch(() => {});
          })
          .catch(() => {});
      }

      return {
        ok: true,
        items: buildYoutubeMenuItems(base, null, pageTitle, pageDuration),
        probing: true,
      };
    }

    case 'PREFETCH_YOUTUBE': {
      const page = msg.pageUrl || sender.tab?.url || '';
      const base = youtubeProcessorVariants(page);
      if (!base.length) return { ok: true };
      const tabId = sender.tab?.id;
      if (tabId != null) {
        scrapeYoutubePlayerFromTab(tabId, page, { bypassCache: true })
          .then((hit) => {
            if (!hit?.items?.length) return;
            chrome.tabs
              .sendMessage(tabId, {
                type: 'YOUTUBE_PROBE_READY',
                pageUrl: page,
                items: hit.items,
              })
              .catch(() => {});
          })
          .catch(() => {});
      }
      ensureYoutubeProbe(base[0].url).catch(() => {});
      return { ok: true };
    }

    case 'SCRAPE_YOUTUBE_PLAYER': {
      const page = msg.pageUrl || sender.tab?.url || '';
      const tabId = sender.tab?.id;
      const hit = await scrapeYoutubePlayerFromTab(tabId, page, {
        bypassCache: !!msg.force,
      });
      if (!hit?.items?.length) return { ok: true, items: [] };
      return {
        ok: true,
        items: hit.items,
        title: hit.title || '',
        duration: hit.duration || 0,
        fromPage: true,
      };
    }

    case 'SHOULD_CAPTURE':
      return {
        ok: true,
        capture: shouldCapture(msg.url, settings, { force: !!msg.force }),
        settings,
      };

    case 'GET_TAB_MEDIA': {
      const tabId = msg.tabId ?? sender.tab?.id;
      if (tabId == null) return { ok: true, items: [] };
      return { ok: true, items: listTabMedia(tabId) };
    }

    case 'SNIFF_TAB_NOW': {
      const tabId = msg.tabId ?? sender.tab?.id;
      if (tabId == null) return { ok: false };
      /* Re-notify frames so PerformanceObserver / banners refresh */
      chrome.tabs.sendMessage(tabId, { type: 'MEDIA_UPDATED' }).catch(() => {});
      const items = listTabMedia(tabId);
      /* Kick enrich on any known HLS masters */
      for (const it of items) {
        if (/\.m3u8(\?|$)/i.test(it.url) || /\/master\.m3u8/i.test(it.url)) {
          enrichPlaylistIntoTab(tabId, it.url, 'hls').catch(() => {});
        }
      }
      return { ok: true, items };
    }

    case 'REGISTER_MEDIA': {
      const tabId = sender.tab?.id;
      if (tabId == null || !msg.url) return { ok: false };
      const url = canonicalizeMediaUrl(msg.url);
      if (!shouldKeepMediaHit(url, msg.size || 0)) return { ok: true, skipped: true };
      const manifest = /\.m3u8(\?|$)/i.test(url) || /\.mpd(\?|$)/i.test(url);
      rememberTabMedia(tabId, {
        url,
        mime: msg.mime || '',
        size: manifest ? 0 : sanitizeByteLength(msg.size),
        width: msg.width || 0,
        height: msg.height || 0,
        label: msg.label || guessResolution({ ...msg, url }),
        ts: Date.now(),
      });
      if (
        /\.m3u8(\?|$)/i.test(url) ||
        /[?&](m3u8|playlist|hls)/i.test(url) ||
        /\/(hls|playlist|master)(\/|\?|$)/i.test(url)
      ) {
        enrichPlaylistIntoTab(tabId, url, 'hls').catch(() => {});
      } else if (/\.mpd(\?|$)/i.test(url) || /[?&]mpd=/i.test(url)) {
        enrichPlaylistIntoTab(tabId, url, 'dash').catch(() => {});
      }
      chrome.tabs.sendMessage(tabId, { type: 'MEDIA_UPDATED' }).catch(() => {});
      return { ok: true };
    }

    case 'RESOLVE_VARIANTS': {
      const tabId = sender.tab?.id;
      const sniffed = tabId != null ? listTabMedia(tabId) : [];
      const domSources = [
        ...(msg.domSources || []),
        ...sniffed.map((s) => ({
          url: s.url,
          label: s.label,
          width: s.width,
          height: s.height,
          size: s.size,
          duration: s.duration,
          bandwidth: s.bandwidth,
          container: s.container,
          type: s.type,
        })),
      ];
      /* Prefer HLS masters from sniff for variant expansion */
      let primary = msg.url || '';
      if (!primary || primary.startsWith('blob:')) {
        const master = sniffed.find(
          (s) => /master\.m3u8/i.test(s.url) || (s.type === 'hls' && /\.m3u8/i.test(s.url))
        );
        if (master) primary = master.url;
      }
      const variants = await resolveVariants(primary, domSources, {
        duration: Number(msg.duration) || 0,
        title: msg.title || '',
      });
      return { ok: true, variants };
    }

    default:
      return { ok: false, error: 'Unknown message' };
  }
}

/**
 * Build resolution menu: DOM sources + sniffed network + HLS/DASH parse.
 * Options include page title + video duration for IDM-style detail lines.
 */
async function resolveVariants(primaryUrl, domSources = [], opts = {}) {
  const pageDuration = Number(opts.duration) || 0;
  const pageTitle = pageTitleFilename(opts.title || '');
  const collected = [];

  for (const src of domSources) {
    if (!src?.url) continue;
    const isHls = /\.m3u8(\?|$)/i.test(src.url);
    const isDash = /\.mpd(\?|$)/i.test(src.url);
    collected.push({
      url: src.url,
      label: src.label || guessResolution(src),
      width: src.width || 0,
      height: src.height || 0,
      size: src.size || 0,
      duration: src.duration || pageDuration,
      bandwidth: src.bandwidth || 0,
      type: isHls ? 'hls' : isDash ? 'dash' : 'file',
      container: src.container || '',
    });
  }

  if (primaryUrl) {
    collected.push({
      url: primaryUrl,
      label: guessResolution({ url: primaryUrl }),
      duration: pageDuration,
      type: /\.m3u8(\?|$)/i.test(primaryUrl)
        ? 'hls'
        : /\.mpd(\?|$)/i.test(primaryUrl)
          ? 'dash'
          : 'file',
    });
  }

  const masters = collected.filter((v) => v.type === 'hls' || v.type === 'dash');
  for (const master of masters.slice(0, 3)) {
    try {
      if (master.type === 'hls') {
        const parsed = await fetchM3u8Variants(master.url, pageDuration);
        collected.push(...parsed);
      } else {
        const parsed = await fetchDashVariants(master.url, pageDuration);
        collected.push(...parsed);
      }
    } catch {
      /* ignore parse failures */
    }
  }

  return dedupeVariants(collected).map((v) => {
    const enriched = enrichVariantForMenu(v, {
      title: pageTitle,
      duration: pageDuration,
    });
    return {
      ...enriched,
      title: enriched.title || pageTitle,
      label: enriched.label && enriched.label !== 'Default' ? enriched.label : guessResolution(enriched),
      sizeLabel: enriched.sizeLabel || formatBytes(enriched.size || 0),
      detail: enriched.detail || idmDetailLine(enriched),
    };
  });
}

/** One HLS segment is never multi-gigabyte; reject bogus Content-Length (often 2^32). */
const MAX_SEGMENT_BYTES = 80 * 1024 * 1024;

async function probeContentLength(url, headers) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 8000);
  const accept = (n) => n > 256 && n <= MAX_SEGMENT_BYTES;
  try {
    try {
      const head = await fetch(url, {
        method: 'HEAD',
        headers,
        credentials: 'omit',
        signal: ctrl.signal,
      });
      const n = Number(head.headers.get('content-length') || 0);
      if (accept(n)) return n;
    } catch {
      /* HEAD is often unsupported on segment proxies */
    }
    const res = await fetch(url, {
      headers: { ...headers, Range: 'bytes=0-1' },
      credentials: 'omit',
      signal: ctrl.signal,
    });
    const cr = res.headers.get('content-range') || '';
    const m = /\/(\d+)\s*$/.exec(cr);
    const fromRange = m ? Number(m[1]) : 0;
    const fromLen = Number(res.headers.get('content-length') || 0);
    try {
      await res.body?.cancel();
    } catch {
      /* already closed */
    }
    if (accept(fromRange)) return fromRange;
    if (res.status === 200 && accept(fromLen)) return fromLen;
  } catch {
    /* timeout or network */
  } finally {
    clearTimeout(timer);
  }
  return 0;
}

/** Real playlist size from BYTERANGE sums or sampled segment Content-Length. */
async function estimatePlaylistBytes(playlistUrl, text, headers) {
  const parsed = parseM3u8Media(text, playlistUrl);
  const exact = extrapolatePlaylistBytes(parsed, []);
  if (exact > 0) {
    return { bytes: exact, duration: parsed.duration || parseM3u8Duration(text) };
  }
  const indexes = sampleSegmentIndexes(parsed.segments.length);
  const samples = await Promise.all(
    indexes.map(async (i) => {
      const seg = parsed.segments[i];
      const bytes = await probeContentLength(seg.url, headers);
      return { bytes, duration: seg.duration || 0 };
    })
  );
  let bytes = extrapolatePlaylistBytes(parsed, samples);
  if (!parsed.mapBytes && parsed.mapUrl) {
    const mapLen = await probeContentLength(parsed.mapUrl, headers);
    if (mapLen > 0 && bytes > 0) bytes += mapLen;
  }
  return { bytes, duration: parsed.duration || parseM3u8Duration(text) };
}

function withMeasuredSize(variant, measured, audioBytes) {
  const dur = measured?.duration || variant.duration || 0;
  const videoBytes = measured?.bytes || 0;
  const size = videoBytes > 0 ? videoBytes + (audioBytes || 0) : 0;
  const bandwidth =
    size > 0 && dur > 0 ? Math.round((size * 8) / dur) : variant.bandwidth || 0;
  const base = {
    ...variant,
    duration: dur,
    size,
    sizeLabel: formatBytes(size),
    bandwidth,
    bitrate: bandwidth,
  };
  return { ...base, detail: idmDetailLine(base) };
}

function isHlsCaptureUrl(url) {
  return (
    /\.m3u8(\?|$)/i.test(url || '') ||
    /[?&](m3u8|playlist|hls)/i.test(url || '') ||
    /\/(hls|playlist|master)(\/|\?|$)/i.test(url || '')
  );
}

/** Use a measured playlist size. Never keep a 4.0 GB sentinel or a missing total. */
async function captureByteLength(url, hinted) {
  const hintedOk = sanitizeByteLength(hinted);
  if (!url || !isHlsCaptureUrl(url)) return hintedOk;
  if (hintedOk > 0) return hintedOk;
  try {
    const variants = await fetchM3u8Variants(url, 0);
    const key = canonicalizeMediaUrl(url);
    const match = variants.filter((v) => canonicalizeMediaUrl(v.url) === key);
    const pool = match.length ? match : variants;
    return pool.reduce((max, v) => Math.max(max, sanitizeByteLength(v.size)), 0);
  } catch {
    return 0;
  }
}

async function fetchM3u8Variants(url, pageDuration = 0) {
  const cookies = await cookiesForUrl(url);
  const headers = { Accept: 'application/vnd.apple.mpegurl,application/x-mpegURL,*/*' };
  if (cookies) headers.Cookie = cookies;
  const res = await fetch(url, { headers, credentials: 'omit' });
  if (!res.ok) throw new Error(`m3u8 ${res.status}`);
  const text = await res.text();
  if (!text.includes('#EXTM3U')) return [];

  if (!text.includes('#EXT-X-STREAM-INF')) {
    const measured = await estimatePlaylistBytes(url, text, headers);
    const height = heightFromMediaUrl(url);
    const base = withMeasuredSize(
      {
        url,
        label: height ? `${height}p` : guessResolution({ url, height }),
        height,
        type: 'hls',
        duration: measured.duration || pageDuration,
        bandwidth: 0,
      },
      measured,
      0
    );
    return [
      { ...base, container: 'TS', ext: 'ts', detail: idmDetailLine({ ...base, container: 'TS' }) },
      { ...base, container: 'MP4', ext: 'mp4', detail: idmDetailLine({ ...base, container: 'MP4' }) },
    ];
  }

  const audioGroups = parseHlsAudioGroups(text, url);
  const audioSizes = new Map();
  await Promise.all(
    [...audioGroups.entries()].map(async ([id, mediaUrl]) => {
      try {
        const audioRes = await fetch(mediaUrl, { headers, credentials: 'omit' });
        if (!audioRes.ok) return;
        const est = await estimatePlaylistBytes(mediaUrl, await audioRes.text(), headers);
        if (est.bytes > 0) audioSizes.set(id, est.bytes);
      } catch {
        /* audio size stays 0; video size is still reported */
      }
    })
  );

  const variants = parseM3u8Master(text, url, {
    expandContainers: true,
    duration: pageDuration,
  });
  const uniqueUrls = [...new Set(variants.map((v) => v.url).filter(Boolean))];
  const measured = new Map();
  await Promise.all(
    uniqueUrls.map(async (mediaUrl) => {
      try {
        const mediaRes = await fetch(mediaUrl, { headers, credentials: 'omit' });
        if (!mediaRes.ok) return;
        measured.set(mediaUrl, await estimatePlaylistBytes(mediaUrl, await mediaRes.text(), headers));
      } catch {
        /* leave this variant without a guessed size */
      }
    })
  );

  return variants.map((v) =>
    withMeasuredSize(v, measured.get(v.url), audioSizes.get(v.audioGroup) || 0)
  );
}

async function fetchDashVariants(url, pageDuration = 0) {
  const cookies = await cookiesForUrl(url);
  const headers = { Accept: 'application/dash+xml,application/xml,*/*' };
  if (cookies) headers.Cookie = cookies;
  const res = await fetch(url, { headers, credentials: 'omit' });
  if (!res.ok) throw new Error(`mpd ${res.status}`);
  const text = await res.text();
  return parseDashManifest(text, url, {
    expandContainers: true,
    duration: pageDuration,
  });
}

async function safeCapture(url, settings, tabId) {
  try {
    const result = await captureToDaemon(url, settings);
    notifyCaptured(url);
    return result;
  } catch (err) {
    chrome.notifications.create({
      type: 'basic',
      iconUrl: ICON(),
      title: 'Download Manager',
      message: err.message || 'Failed to start download',
    });
    if (tabId) {
      try {
        await chrome.tabs.sendMessage(tabId, {
          type: 'SHOW_TOAST',
          message: err.message,
          level: 'error',
        });
      } catch {
        /* no content script */
      }
    }
    throw err;
  }
}

function notifyCaptured(url) {
  const name = filenameFromUrl(url);
  chrome.notifications.create({
    type: 'basic',
    iconUrl: ICON(),
    title: 'Added to Download Manager',
    message: name,
  });
}

/**
 * IDM-style confirmation window.
 */
async function openCaptureDialog(url, openerTabId, extra = {}) {
  const params = new URLSearchParams({
    url,
    tabId: openerTabId != null ? String(openerTabId) : '',
    pageUrl: extra.pageUrl || '',
    referer: extra.referer || '',
    label: extra.label || '',
    filename: extra.filename || '',
    quality: extra.quality || '',
    title: extra.title || '',
    size: String(extra.size || extra.totalBytes || 0),
  });
  const dialogUrl = chrome.runtime.getURL(`capture/capture.html?${params}`);

  await chrome.windows.create({
    url: dialogUrl,
    type: 'popup',
    width: 520,
    height: 400,
    focused: true,
  });
}
