/**
 * Page-world hooks for hls.js / fetch of manifests (blob players).
 * Loaded declaratively as a "world": "MAIN" content script (see manifest.json)
 * instead of being injected as an inline <script>, so it never runs afoul of
 * a page's own Content-Security-Policy (script-src).
 */
(() => {
  if (window.__dmPlayerHooked) return;
  window.__dmPlayerHooked = true;

  const post = (url) => {
    try {
      if (!url || typeof url !== 'string') return;
      if (!/^https?:/i.test(url)) return;
      window.postMessage({ source: 'dm-player-hook', url }, '*');
    } catch (_) {
      /* ignore */
    }
  };

  const looksMedia = (u) =>
    /\.(m3u8|mpd|mp4|m4v|webm)(\?|$)/i.test(u) ||
    /[?&](m3u8|mpd|playlist|hls)/i.test(u) ||
    /\/(hls|dash|playlist|master|manifest|stream|litro)\b/i.test(u) ||
    /master\.m3u8|index\.m3u8/i.test(u);

  const origFetch = window.fetch;
  if (typeof origFetch === 'function') {
    window.fetch = function (...args) {
      try {
        const u = typeof args[0] === 'string' ? args[0] : args[0]?.url;
        if (u && looksMedia(u)) post(u);
      } catch (_) {
        /* ignore */
      }
      return origFetch.apply(this, args);
    };
  }

  const XO = window.XMLHttpRequest;
  if (XO && XO.prototype) {
    const open = XO.prototype.open;
    XO.prototype.open = function (method, url, ...rest) {
      try {
        if (url && looksMedia(String(url))) post(String(url));
      } catch (_) {
        /* ignore */
      }
      return open.call(this, method, url, ...rest);
    };
  }

  const wrapHls = () => {
    const H = window.Hls;
    if (!H || H.__dmWrapped) return;
    H.__dmWrapped = true;
    const load = H.prototype.loadSource;
    if (typeof load === 'function') {
      H.prototype.loadSource = function (src) {
        post(src);
        return load.call(this, src);
      };
    }
  };
  wrapHls();
  setInterval(wrapHls, 1500);
})();
