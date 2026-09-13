/**
 * Content script — IDM-style link sniffer + video download banner with resolution picker.
 */
(() => {
  const DOWNLOADABLE_HINT =
    /\.(zip|rar|7z|tar|gz|bz2|xz|iso|exe|msi|dmg|pkg|deb|rpm|apk|pdf|doc|docx|xls|xlsx|ppt|pptx|mp4|mkv|avi|mov|wmv|flv|webm|m4v|mp3|flac|wav|aac|ogg|m4a|wma|torrent|bin|nupkg)(\?|#|$)/i;

  /** @type {WeakMap<HTMLVideoElement, HTMLElement>} */
  const banners = new WeakMap();
  /** videos we already observed */
  const watchedVideos = new WeakSet();
  let settings = null;
  let mediaPanel = null;
  let sniffTimer = null;
  let bannerTimer = null;
  let openMenu = null;
  /** @type {{ watch: string, items: any[], title?: string, duration?: number, ts: number } | null} */
  let ytInstantCache = null;

  /** Safe messaging — avoids crashes after extension reload / SW sleep. */
  function runtimeSend(msg) {
    try {
      if (!chrome.runtime?.id) return Promise.resolve(null);
      return chrome.runtime.sendMessage(msg).catch(() => null);
    } catch {
      return Promise.resolve(null);
    }
  }

  async function init() {
    if (window !== window.top) {
      /* Subframes: no banners / sniff / scrape — parent frame owns YouTube UI */
      return;
    }

    const res = await runtimeSend({ type: 'GET_SETTINGS' });
    if (res?.ok) settings = res.settings;

    chrome.storage.onChanged.addListener((changes, area) => {
      if (area === 'sync' && changes.settings) {
        settings = { ...settings, ...changes.settings.newValue };
        if (settings?.mediaGrabber && !isYouTubeHere()) scheduleMediaSniff();
        else removeMediaPanel();
        if (settings?.videoBanner !== false) scheduleBanners();
        else removeAllBanners();
      }
    });

    document.addEventListener('click', onClick, true);
    document.addEventListener('auxclick', onAuxClick, true);
    document.addEventListener('click', closeMenusOnOutside, true);

    chrome.runtime.onMessage.addListener((msg) => {
      if (msg.type === 'SHOW_TOAST') showToast(msg.message, msg.level || 'info');
      if (msg.type === 'SNIFF_NOW' || msg.type === 'MEDIA_UPDATED') {
        if (!isYouTubeHere()) scheduleMediaSniff();
        scheduleBanners();
      }
      if (msg.type === 'YOUTUBE_PROBE_READY' && msg.items?.length) {
        ytInstantCache = {
          watch: youtubeWatchKey(location.href),
          items: msg.items,
          ts: Date.now(),
        };
        refreshOpenMenu(msg.items);
      }
    });

    /* Media grabber is noisy on YouTube (CDN flood); banner handles downloads there */
    if (settings?.mediaGrabber !== false && !isYouTubeHere()) scheduleMediaSniff();
    if (settings?.videoBanner !== false) scheduleBanners();
    prefetchYoutubeIfNeeded();

    const mo = new MutationObserver(() => {
      if (settings?.mediaGrabber && !isYouTubeHere()) scheduleMediaSniff();
      if (settings?.videoBanner !== false) scheduleBanners();
    });
    mo.observe(document.documentElement, { childList: true, subtree: true });

    /* YouTube SPA navigations */
    window.addEventListener('yt-navigate-finish', () => {
      ytInstantCache = null;
      prefetchYoutubeIfNeeded();
    });
    let lastHref = location.href;
    setInterval(() => {
      if (location.href !== lastHref) {
        lastHref = location.href;
        ytInstantCache = null;
        prefetchYoutubeIfNeeded();
      }
    }, 800);
  }

  function isYouTubeHere() {
    return /(?:^|\.)youtube\.com$|(?:^|\.)youtu\.be$/i.test(location.hostname);
  }

  function youtubeWatchKey(url) {
    try {
      const u = new URL(url);
      const v = u.searchParams.get('v');
      if (v) return `https://www.youtube.com/watch?v=${v}`;
      const m = u.pathname.match(/\/(?:shorts|embed|live)\/([^/?#]+)/i);
      if (m) return `https://www.youtube.com/watch?v=${m[1]}`;
    } catch {
      /* ignore */
    }
    return url;
  }

  function prefetchYoutubeIfNeeded() {
    if (window !== window.top) return;
    if (!isYouTubeHere()) return;
    if (!/\/watch|\/shorts\/|\/embed\/|\/live\//i.test(location.pathname) && !/[?&]v=/.test(location.search)) {
      return;
    }
    const watch = youtubeWatchKey(location.href);
    const tryScrape = (attempt) => {
      runtimeSend({ type: 'SCRAPE_YOUTUBE_PLAYER', pageUrl: location.href })
        .then((res) => {
          if (res?.ok && res.items?.length) {
            ytInstantCache = {
              watch,
              items: res.items,
              title: res.title,
              duration: res.duration,
              ts: Date.now(),
            };
            return;
          }
          if (attempt < 4) setTimeout(() => tryScrape(attempt + 1), 400 * attempt);
        })
        .catch(() => {
          if (attempt < 4) setTimeout(() => tryScrape(attempt + 1), 400 * attempt);
        });
    };
    tryScrape(1);
    runtimeSend({ type: 'PREFETCH_YOUTUBE', pageUrl: location.href })
      .catch(() => {});
  }

  function pageMeta(video) {
    const title =
      document.querySelector('meta[property="og:title"]')?.content ||
      document.querySelector('h1.ytd-watch-metadata yt-formatted-string')?.textContent ||
      document.title ||
      '';
    const duration =
      video && Number.isFinite(video.duration) && video.duration > 0 ? video.duration : 0;
    return { title, duration };
  }

  function refreshOpenMenu(items) {
    if (!openMenu?.menu || !items?.length) return;
    const sorted = finalizeVariants(items);
    const newMax = Math.max(0, ...sorted.map((v) => Number(v.height) || 0));
    const oldMax = Math.max(0, ...(openMenu.variants || []).map((v) => Number(v.height) || 0));
    /* Don't replace a good HD list with a degenerate all-360p probe result */
    if (oldMax >= 720 && newMax < 720) return;
    const ul = openMenu.menu.querySelector('.dm-res-list');
    if (!ul) return;
    openMenu.variants = sorted;
    ul.innerHTML = '';
    fillMenuList(ul, sorted);
  }

  function onAuxClick(e) {
    if (e.button !== 1) return;
    const force = e.altKey && (settings?.altClickCapture !== false);
    handlePotentialDownload(e, force);
  }

  function onClick(e) {
    if (e.button !== 0) return;
    if (e.ctrlKey || e.metaKey || e.shiftKey) return;
    if (e.target?.closest?.('.dm-video-banner, .dm-res-menu, #dm-media-grabber, #dm-toast')) return;
    const force = e.altKey && (settings?.altClickCapture !== false);
    handlePotentialDownload(e, force);
  }

  function closeMenusOnOutside(e) {
    if (!openMenu) return;
    if (e.target?.closest?.('.dm-video-banner, .dm-res-menu')) return;
    closeOpenMenu();
  }

  /** Unwrap Google / social redirectors to the real destination URL. */
  function unwrapHref(raw) {
    try {
      const u = new URL(raw);
      if (/google\./i.test(u.hostname) && u.pathname === '/url') {
        return u.searchParams.get('q') || u.searchParams.get('url') || raw;
      }
      if (/facebook\.com$/i.test(u.hostname) && u.pathname.includes('l.php')) {
        return u.searchParams.get('u') || raw;
      }
    } catch {
      /* keep */
    }
    return raw;
  }

  function isVideoPageUrl(href) {
    try {
      const u = new URL(href);
      const h = u.hostname.replace(/^www\./, '');
      if (h === 'youtu.be') return true;
      if (h.endsWith('youtube.com') || h.endsWith('youtube-nocookie.com')) {
        return (
          u.pathname.startsWith('/watch') ||
          u.pathname.startsWith('/shorts/') ||
          u.pathname.startsWith('/embed/') ||
          u.pathname.startsWith('/live/')
        );
      }
      if (h.endsWith('vimeo.com') && /\/\d+/.test(u.pathname)) return true;
      if (h.endsWith('dailymotion.com') && u.pathname.includes('/video/')) return true;
    } catch {
      return false;
    }
    return false;
  }

  function isYouTubeHost(hostname) {
    const h = String(hostname || '').replace(/^www\./, '').toLowerCase();
    return h === 'youtu.be' || h.endsWith('youtube.com') || h.endsWith('youtube-nocookie.com');
  }

  function isYouTubeWatchLike(href) {
    try {
      const u = new URL(href, location.href);
      if (!isYouTubeHost(u.hostname)) return false;
      const path = u.pathname;
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

  function isDownloadSiteOrFile(href, anchor) {
    if (anchor?.hasAttribute?.('download')) return true;
    if (DOWNLOADABLE_HINT.test(href)) return true;
    if (/[?&](download|attachment|dl)=|\bdownload\b|\battachment\b/i.test(href)) return true;

    const extList = settings?.extensions || [];
    try {
      const path = new URL(href).pathname;
      const m = path.match(/\.([a-z0-9]{2,5})$/i);
      if (m && extList.includes(m[1].toLowerCase())) return true;
    } catch {
      /* ignore */
    }

    if (/googlevideo\.com|videoplayback|mime=video|mime=audio/i.test(href)) return true;

    if (
      /savefrom\.net|ssyoutube\.|y2mate\.|yt1s\.|mp3-youtube|download-video|getvideo/i.test(
        href
      )
    ) {
      return true;
    }

    return false;
  }

  function handlePotentialDownload(e, force) {
    if (!settings?.enabled) return;
    if (settings.captureMode === 'off') return;
    if (settings.linkClickCapture === false && !force) return;

    const a = e.target?.closest?.('a[href]');
    if (!a) return;

    let href = a.href;
    if (!href || href.startsWith('javascript:') || href.startsWith('mailto:')) return;
    if (href.startsWith('blob:') || href.startsWith('data:')) return;
    if (href.startsWith('#')) return;

    href = unwrapHref(href);

    const catchVideo = settings.catchVideoPageLinks === true;
    // Never steal in-page YouTube navigation (suggested / related / home tiles).
    // Grabbing is only via the main player banner.
    const youtubeNav =
      isYouTubeHost(location.hostname) && isYouTubeWatchLike(href);
    const looksDownloadable =
      force ||
      isDownloadSiteOrFile(href, a) ||
      (catchVideo && isVideoPageUrl(href) && !youtubeNav);

    if (!looksDownloadable) return;

    e.preventDefault();
    e.stopPropagation();
    if (typeof e.stopImmediatePropagation === 'function') e.stopImmediatePropagation();

    const pageUrl = location.href;
    const type = settings.captureMode === 'auto' ? 'CAPTURE_URL' : 'OPEN_CAPTURE';

    runtimeSend({
        type,
        url: href,
        force: !!force,
        pageUrl,
        referer: pageUrl,
      })
      .then((res) => {
        if (!res) {
          showToast('Reload this tab after updating the extension', 'error');
          return;
        }
        if (res.ok && type === 'CAPTURE_URL') {
          showToast(`Queued: ${filename(href)}`, 'success');
        } else if (res.ok === false) {
          showToast(res.error || 'Could not open download dialog', 'error');
        }
      });
  }

  /* ─── IDM video download banner ─────────────────────────────── */

  function scheduleBanners() {
    clearTimeout(bannerTimer);
    bannerTimer = setTimeout(attachVideoBanners, 350);
  }

  function removeAllBanners() {
    document.querySelectorAll('.dm-video-banner').forEach((el) => el.remove());
    closeOpenMenu();
  }

  function attachVideoBanners() {
    if (settings?.videoBanner === false || settings?.enabled === false) {
      removeAllBanners();
      return;
    }

    const videos = mainPlayerVideos();
    const keep = new Set(videos);

    document.querySelectorAll('.dm-video-banner').forEach((el) => {
      const host = el.parentElement;
      const video = host?.querySelector?.('video') || null;
      if (!video || !keep.has(video)) {
        el.remove();
      }
    });

    videos.forEach((video) => {
      if (!(video instanceof HTMLVideoElement)) return;
      ensureBanner(video);
      if (!watchedVideos.has(video)) {
        watchedVideos.add(video);
        video.addEventListener('loadedmetadata', () => refreshBanner(video));
        video.addEventListener('play', () => refreshBanner(video));
        video.addEventListener('emptied', () => refreshBanner(video));
      }
    });
  }

  /** Only the currently playing main player — not sidebar / suggested previews. */
  function mainPlayerVideos() {
    if (isYouTubeHost(location.hostname)) {
      const main =
        document.querySelector('#movie_player video.html5-main-video') ||
        document.querySelector('#movie_player video') ||
        document.querySelector('ytd-player #movie_player video') ||
        document.querySelector('ytd-watch-flexy video.html5-main-video');
      return main ? [main] : [];
    }

    // Other sites: prefer large playing videos; skip tiny thumbnails.
    return [...document.querySelectorAll('video')].filter((video) => {
      if (!(video instanceof HTMLVideoElement)) return false;
      const rect = video.getBoundingClientRect();
      if (rect.width < 240 || rect.height < 135) return false;
      if (video.closest('[class*="recommend"], [class*="related"], [class*="sidebar"]')) {
        return false;
      }
      return true;
    });
  }

  function ensureBanner(video) {
    let banner = banners.get(video);
    if (banner && banner.isConnected) {
      positionBanner(video, banner);
      return banner;
    }

    banner = document.createElement('div');
    banner.className = 'dm-video-banner';
    banner.innerHTML = `
      <button type="button" class="dm-vb-btn" title="Download this video">
        <span class="dm-vb-icon" aria-hidden="true">⬇</span>
        <span class="dm-vb-text">Download this video</span>
        <span class="dm-vb-caret">▾</span>
      </button>
    `;

    const host = findBannerHost(video);
    host.appendChild(banner);
    banners.set(video, banner);
    positionBanner(video, banner);

    banner.querySelector('.dm-vb-btn').addEventListener('click', (e) => {
      e.preventDefault();
      e.stopPropagation();
      toggleResolutionMenu(video, banner);
    });

    return banner;
  }

  function findBannerHost(video) {
    /* YouTube / HTML5 players — IDM attaches to the player chrome, not raw <video> */
    const yt = video.closest(
      '#movie_player, .html5-video-player, ytd-player, .html5-video-container'
    );
    if (yt) {
      const style = getComputedStyle(yt);
      if (style.position === 'static') yt.classList.add('dm-vb-host');
      return yt;
    }

    const parent = video.parentElement;
    if (!parent) return document.documentElement;
    const style = getComputedStyle(parent);
    if (style.position === 'static') parent.classList.add('dm-vb-host');
    return parent;
  }

  function positionBanner(video, banner) {
    /* Prefer overlaying the video box itself when parent is tight */
    const rect = video.getBoundingClientRect();
    if (rect.width < 160 || rect.height < 90) {
      banner.classList.add('dm-vb-hidden');
      return;
    }
    banner.classList.remove('dm-vb-hidden');
  }

  function refreshBanner(video) {
    const banner = banners.get(video);
    if (banner) positionBanner(video, banner);
    registerVideoSources(video);
  }

  function registerVideoSources(video) {
    const sources = collectDomSources(video);
    sources.forEach((s) => {
      if (!s.url || s.url.startsWith('blob:')) return;
      runtimeSend({
        type: 'REGISTER_MEDIA',
        url: s.url,
        width: s.width,
        height: s.height,
        label: s.label,
      }).catch(() => {});
    });
  }

  function collectDomSources(video) {
    const out = [];
    const push = (url, extra = {}) => {
      if (!url) return;
      out.push({
        url,
        width: extra.width || video.videoWidth || 0,
        height: extra.height || video.videoHeight || 0,
        label: extra.label || '',
      });
    };

    if (video.currentSrc) push(video.currentSrc);
    if (video.src) push(video.src);
    video.querySelectorAll('source').forEach((s) => {
      push(s.src, {
        label: s.getAttribute('label') || s.getAttribute('data-quality') || s.getAttribute('size') || '',
      });
    });
    return out;
  }

  async function toggleResolutionMenu(video, banner) {
    if (openMenu && openMenu.banner === banner) {
      closeOpenMenu();
      return;
    }
    closeOpenMenu();

    const btn = banner.querySelector('.dm-vb-btn');
    const { title: pageTitle, duration } = pageMeta(video);

    /* YouTube: open from live player quality list (matches Settings → Quality) */
    if (isYouTubeHere()) {
      const watch = youtubeWatchKey(location.href);
      const maxCachedH = ytInstantCache?.items
        ? Math.max(0, ...ytInstantCache.items.map((i) => Number(i.height) || 0))
        : 0;
      const fresh =
        ytInstantCache &&
        ytInstantCache.watch === watch &&
        Date.now() - ytInstantCache.ts < 10 * 60_000 &&
        ytInstantCache.items?.length &&
        maxCachedH >= 720;

      if (fresh) {
        showResolutionMenu(video, banner, finalizeVariants(ytInstantCache.items));
        runtimeSend({
            type: 'GET_YOUTUBE_VARIANTS',
            pageUrl: location.href,
            title: pageTitle,
            duration,
            fast: true,
          })
          .then((ytProc) => {
            if (ytProc?.items?.length) {
              ytInstantCache = { watch, items: ytProc.items, ts: Date.now() };
              refreshOpenMenu(ytProc.items);
            }
          })
          .catch(() => {});
        return;
      }

      btn.classList.add('dm-vb-loading');
      btn.querySelector('.dm-vb-text').textContent = 'Opening…';
      try {
        const scraped = await runtimeSend({
          type: 'SCRAPE_YOUTUBE_PLAYER',
          pageUrl: location.href,
          force: true,
        });
        let variants = finalizeVariants(scraped?.items || []);
        if (!variants.length || Math.max(0, ...variants.map((v) => Number(v.height) || 0)) < 480) {
          const ytProc = await runtimeSend({
            type: 'GET_YOUTUBE_VARIANTS',
            pageUrl: location.href,
            title: pageTitle,
            duration,
            fast: true,
          });
          if (ytProc?.items?.length) variants = finalizeVariants(ytProc.items);
        } else {
          ytInstantCache = {
            watch,
            items: scraped.items,
            title: scraped.title,
            duration: scraped.duration,
            ts: Date.now(),
          };
        }
        if (!variants.length) {
          showToast('Play the video for a few seconds, then open the menu again', 'error');
          return;
        }
        showResolutionMenu(video, banner, variants);
      } catch (err) {
        showToast(err.message || 'Failed to detect resolutions', 'error');
      } finally {
        btn.classList.remove('dm-vb-loading');
        btn.querySelector('.dm-vb-text').textContent = 'Download this video';
      }
      return;
    }

    btn.classList.add('dm-vb-loading');
    btn.querySelector('.dm-vb-text').textContent = 'Detecting…';

    try {
      const domSources = collectDomSources(video);
      const primary =
        video.currentSrc ||
        video.src ||
        domSources[0]?.url ||
        '';

      /* Show sniffed media immediately; playlist parse can finish after */
      const sniffed = await runtimeSend({ type: 'GET_TAB_MEDIA' });
      let variants = (sniffed?.items || []).map((n) => ({
        ...n,
        duration: n.duration || duration,
        title: n.title || pageTitle,
      }));

      if (variants.length) {
        variants = finalizeVariants(variants);
        showResolutionMenu(video, banner, variants);
        btn.classList.remove('dm-vb-loading');
        btn.querySelector('.dm-vb-text').textContent = 'Download this video';
      }

      const resolved = await runtimeSend({
        type: 'RESOLVE_VARIANTS',
        url: primary.startsWith('blob:') ? '' : primary,
        domSources: domSources.filter((s) => s.url && !s.url.startsWith('blob:')),
        duration,
        title: pageTitle,
      });

      let merged = resolved?.variants || [];
      for (const n of sniffed?.items || []) {
        const key = n.container ? `${n.url}|${n.container}` : n.url;
        if (!merged.some((v) => (v.container ? `${v.url}|${v.container}` : v.url) === key)) {
          merged.push({
            ...n,
            duration: n.duration || duration,
            title: n.title || pageTitle,
          });
        }
      }

      merged = merged.filter((v) => v.url && !v.url.startsWith('blob:'));

      if (!merged.length && primary && !primary.startsWith('blob:')) {
        merged = [{
          url: primary,
          label: dimLabel(video) || 'Default',
          height: video.videoHeight || 0,
          width: video.videoWidth || 0,
          duration,
          title: pageTitle,
          container: 'MP4',
        }];
      }

      merged = finalizeVariants(merged);

      if (!merged.length) {
        if (!openMenu) {
          showToast('Play the video for a few seconds, then open the menu again', 'error');
        }
        return;
      }

      if (openMenu && openMenu.banner === banner) {
        refreshOpenMenu(merged);
      } else if (!openMenu) {
        showResolutionMenu(video, banner, merged);
      }
    } catch (err) {
      showToast(err.message || 'Failed to detect resolutions', 'error');
    } finally {
      btn.classList.remove('dm-vb-loading');
      btn.querySelector('.dm-vb-text').textContent = 'Download this video';
    }
  }

  function finalizeVariants(list) {
    let variants = sortVariants(list.filter((v) => v.url && !String(v.url).startsWith('blob:')));
    const videoVariants = variants.filter((v) => !v.audio);
    const audioVariants = variants.filter((v) => v.audio).slice(0, 3);
    return [...videoVariants, ...audioVariants];
  }

  function dimLabel(video) {
    const h = video.videoHeight || 0;
    if (h >= 2000) return '2160p';
    if (h >= 1400) return '1440p';
    if (h >= 1000) return '1080p';
    if (h >= 700) return '720p';
    if (h >= 450) return '480p';
    if (h >= 300) return '360p';
    if (h > 0) return `${h}p`;
    return '';
  }

  function sortVariants(list) {
    const score = (v) => {
      /* Prefer real CDN / player streams (correct itag + clen) over yt-dlp processor rows */
      if (v.stream || (v.url && /googlevideo\.com/i.test(v.url))) {
        const h = Number(v.height) || 0;
        if (h) return 20_000 + h;
        const m = String(v.label || '').match(/(\d{3,4})/);
        return 20_000 + (m ? Number(m[1]) : 0);
      }
      if (v.processor) {
        return (
          10_000 +
          (Number(String(v.quality).replace(/\D/g, '')) || (v.quality === 'best' ? 9000 : 0))
        );
      }
      const h = Number(v.height) || 0;
      if (h) return h;
      const m = String(v.label || '').match(/(\d{3,4})/);
      return m ? Number(m[1]) : 0;
    };
    const seen = new Set();
    return [...list]
      .filter((v) => {
        const key = v.processor
          ? `proc:${v.quality}:${v.container || v.ext || ''}`
          : `${v.url}|${v.container || v.ext || ''}`;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      })
      .sort((a, b) => {
        /* CDN / player streams first — they match online-tool sizes (itag + clen) */
        const aStream = !!(a.stream || (a.url && /googlevideo\.com/i.test(a.url)));
        const bStream = !!(b.stream || (b.url && /googlevideo\.com/i.test(b.url)));
        if (aStream !== bStream) return aStream ? -1 : 1;
        if (!!a.processor !== !!b.processor) return a.processor ? 1 : -1;
        if (!!a.audio !== !!b.audio) return a.audio ? 1 : -1;
        const d = score(b) - score(a);
        if (d) return d;
        const ca = String(a.container || '');
        const cb = String(b.container || '');
        if (ca === 'TS' && cb !== 'TS') return -1;
        if (cb === 'TS' && ca !== 'TS') return 1;
        return 0;
      });
  }

  function showResolutionMenu(video, banner, variants) {
    const menu = document.createElement('div');
    menu.className = 'dm-res-menu';
    menu.innerHTML = `
      <div class="dm-res-head">Download this video</div>
      <ul class="dm-res-list"></ul>
    `;
    const ul = menu.querySelector('.dm-res-list');
    fillMenuList(ul, variants);

    banner.appendChild(menu);
    openMenu = { menu, banner, video, variants };

    /* Keep menu on-screen */
    requestAnimationFrame(() => {
      const r = menu.getBoundingClientRect();
      if (r.bottom > window.innerHeight - 8) {
        menu.classList.add('dm-res-up');
      }
    });
  }

  function fillMenuList(ul, variants) {
    const pageTitle =
      variants.find((v) => v.title)?.title ||
      document.querySelector('meta[property="og:title"]')?.content ||
      document.title ||
      'Video';

    variants.slice(0, 20).forEach((v, idx) => {
      const li = document.createElement('li');
      const title = (v.title || pageTitle || 'Video').trim();
      /* IDM-style: resolution is the primary row label, not the video title */
      const resLabel =
        v.audio || v.quality === 'audio'
          ? 'Audio only'
          : v.quality === 'best' || v.label === 'Best'
            ? 'Best'
            : v.label && /\d{3,4}\s*p/i.test(String(v.label))
              ? String(v.label)
              : v.height
                ? `${v.height}p`
                : String(v.label || 'Video');
      const detail =
        v.detail ||
        [
          v.container ? `${v.container} file` : '',
          v.sizeLabel || (v.size ? formatSize(v.size) : ''),
          title && title.length < 80 ? title : '',
        ]
          .filter(Boolean)
          .join(', ') ||
        shortHost(v.url);
      li.innerHTML = `
        <button type="button" class="dm-res-item${idx === 0 ? ' dm-res-best' : ''}">
          <span class="dm-res-label">${escapeHtml(resLabel)}</span>
          <span class="dm-res-meta">${escapeHtml(detail)}</span>
        </button>
      `;
      li.querySelector('button').addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        closeOpenMenu();
        const pageUrl = location.href;
        const quality =
          v.quality ||
          (v.audio ? 'audio' : String(v.label || '').match(/(\d{3,4})/)?.[1]) ||
          (v.processor ? 'best' : undefined);
        const ext = v.ext || (v.container === 'TS' ? 'ts' : v.audio ? 'm4a' : 'mp4');
        const safeTitle = title
          .replace(/[<>:"/\\|?*]+/g, '')
          .replace(/\s+/g, ' ')
          .trim()
          .slice(0, 80);
        const qTag = String(quality || resLabel || '')
          .replace(/[^\w.\-]+/g, '')
          .slice(0, 24);
        const filename = safeTitle
          ? qTag && !/^best$/i.test(qTag)
            ? `${safeTitle} [${qTag}].${ext}`
            : `${safeTitle}.${ext}`
          : undefined;
        const isDirectStream =
          v.stream ||
          (v.url && /googlevideo\.com/i.test(v.url) && /videoplayback/i.test(v.url));
        const itag = String(v.formatId || '').replace(/\D/g, '');
        /* Pin adaptive itag so daemon merges the same 4K/2K/1080 stream the menu showed */
        const pinnedFormat =
          itag && !v.progressive && !v.audio ? `${itag}+bestaudio/ba` : undefined;
        runtimeSend({
          type: settings?.captureMode === 'auto' ? 'CAPTURE_URL' : 'OPEN_CAPTURE',
          url: v.url,
          label: resLabel,
          quality: isDirectStream && v.progressive ? undefined : quality,
          title: safeTitle || title,
          size: Number(v.size) || 0,
          totalBytes: Number(v.size) || 0,
          format: pinnedFormat || (isDirectStream || v.processor ? '' : v.formatId || ''),
          filename,
          pageUrl,
          referer: pageUrl,
        }).then((res) => {
          if (res?.ok && settings?.captureMode === 'auto') {
            showToast(
              isDirectStream
                ? `Queued ${resLabel} (fast segmented)`
                : `Queued ${resLabel || title}`,
              'success'
            );
          } else if (res && res.ok === false) {
            showToast(res.error || 'Capture failed', 'error');
          }
        });
      });
      ul.appendChild(li);
    });
  }

  function closeOpenMenu() {
    if (!openMenu) return;
    openMenu.menu.remove();
    openMenu = null;
  }

  /* ─── Corner media grabber panel (secondary) ────────────────── */

  function scheduleMediaSniff() {
    clearTimeout(sniffTimer);
    sniffTimer = setTimeout(sniffMedia, 600);
  }

  function sniffMedia() {
    if (!settings?.mediaGrabber) return;
    if (isYouTubeHere()) return; /* banner + CDN sniff handle YouTube */

    const urls = new Set();

    document.querySelectorAll('video, audio').forEach((el) => {
      if (el.currentSrc && !el.currentSrc.startsWith('blob:')) urls.add(el.currentSrc);
      if (el.src && !String(el.src).startsWith('blob:')) urls.add(el.src);
      el.querySelectorAll('source[src]').forEach((s) => {
        if (s.src && !s.src.startsWith('blob:')) urls.add(s.src);
      });
    });

    document.querySelectorAll('a[href]').forEach((a) => {
      if (DOWNLOADABLE_HINT.test(a.href)) urls.add(a.href);
    });

    const list = [...urls].filter(
      (u) =>
        u.startsWith('http') &&
        !u.startsWith('blob:') &&
        !/favicon|sprite|tracking|analytics/i.test(u)
    );

    runtimeSend({ type: 'GET_TAB_MEDIA' }).then((res) => {
      if (!res) return;
      const sniffed = (res?.items || []).map((m) => ({
        url: m.url,
        label: m.label || '',
        size: m.size || 0,
        detail: m.detail || '',
        container: m.container || '',
      }));
      const byUrl = new Map();
      for (const u of list) byUrl.set(u, { url: u, label: '', size: 0, detail: '' });
      for (const m of sniffed) {
        const key = m.container ? `${m.url}|${m.container}` : m.url;
        const prev = byUrl.get(key) || { url: m.url, label: '', size: 0, detail: '' };
        byUrl.set(key, {
          url: m.url,
          label: m.label || prev.label,
          size: m.size || prev.size || 0,
          detail: m.detail || prev.detail || '',
          container: m.container || prev.container || '',
        });
      }
      const items = [...byUrl.values()].slice(0, 12);
      if (!items.length) {
        removeMediaPanel();
        return;
      }
      renderMediaPanel(items);
    }).catch(() => {
      if (!list.length) {
        removeMediaPanel();
        return;
      }
      renderMediaPanel(list.slice(0, 12).map((url) => ({ url, label: '', size: 0 })));
    });
  }

  function removeMediaPanel() {
    mediaPanel?.remove();
    mediaPanel = null;
  }

  function renderMediaPanel(items) {
    if (!mediaPanel) {
      mediaPanel = document.createElement('div');
      mediaPanel.id = 'dm-media-grabber';
      mediaPanel.innerHTML = `
        <div class="dm-mg-head">
          <span class="dm-mg-title">DM Media Grabber</span>
          <button type="button" class="dm-mg-close" title="Close">×</button>
        </div>
        <ul class="dm-mg-list"></ul>
      `;
      document.documentElement.appendChild(mediaPanel);
      mediaPanel.querySelector('.dm-mg-close').addEventListener('click', () => {
        removeMediaPanel();
        settings = { ...settings, mediaGrabber: false };
      });
    }

    const ul = mediaPanel.querySelector('.dm-mg-list');
    ul.innerHTML = '';
    items.forEach((item) => {
      const url = typeof item === 'string' ? item : item.url;
      const size = typeof item === 'object' ? item.size || 0 : 0;
      const label = typeof item === 'object' ? item.label || '' : '';
      const detail = typeof item === 'object' ? item.detail || '' : '';
      const li = document.createElement('li');
      const name = filename(url);
      const sizeText = formatSize(size);
      const meta = detail || [label, sizeText].filter(Boolean).join(' · ');
      li.innerHTML = `
        <span class="dm-mg-name" title="${escapeAttr(url)}">${escapeHtml(name)}</span>
        <span class="dm-mg-meta">${escapeHtml(meta)}</span>
        <button type="button" class="dm-mg-dl">Download</button>
      `;
      li.querySelector('button').addEventListener('click', () => {
        runtimeSend({ type: 'OPEN_CAPTURE', url });
      });
      ul.appendChild(li);
    });
  }

  function showToast(message, level) {
    let el = document.getElementById('dm-toast');
    if (!el) {
      el = document.createElement('div');
      el.id = 'dm-toast';
      document.documentElement.appendChild(el);
    }
    el.className = `dm-toast dm-toast-${level}`;
    el.textContent = message;
    el.classList.add('dm-toast-show');
    clearTimeout(el._hide);
    el._hide = setTimeout(() => el.classList.remove('dm-toast-show'), 3200);
  }

  function filename(url) {
    try {
      const u = new URL(url);
      return decodeURIComponent(u.pathname.split('/').filter(Boolean).pop() || u.hostname);
    } catch {
      return url.slice(0, 48);
    }
  }

  function shortHost(url) {
    try {
      return new URL(url).hostname.replace(/^www\./, '');
    } catch {
      return '';
    }
  }

  function formatSize(n) {
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

  function escapeHtml(s) {
    return String(s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;');
  }

  function escapeAttr(s) {
    return escapeHtml(s).replace(/"/g, '&quot;');
  }

  init();
})();
