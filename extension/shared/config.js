/** Shared defaults & helpers for the DM Chrome extension (IDM-style). */

export const DAEMON_URL = 'http://127.0.0.1:7373';

/** File extensions IDM typically captures by default. */
export const DEFAULT_EXTENSIONS = [
  'zip', 'rar', '7z', 'tar', 'gz', 'bz2', 'xz', 'iso',
  'exe', 'msi', 'dmg', 'pkg', 'deb', 'rpm', 'apk',
  'pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx',
  'mp4', 'mkv', 'avi', 'mov', 'wmv', 'flv', 'webm', 'm4v',
  'mp3', 'flac', 'wav', 'aac', 'ogg', 'm4a', 'wma',
  'jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'tiff', 'svg',
  'torrent', 'bin', 'img', 'cab', 'nupkg',
];

export const DEFAULT_SETTINGS = {
  enabled: true,
  /** ask | auto | off — ask shows IDM-style confirm dialog */
  captureMode: 'ask',
  extensions: DEFAULT_EXTENSIONS,
  /** Skip tiny assets (favicon etc.) when sniffing — bytes; 0 = no min */
  minSizeHint: 0,
  /** Alt+click always captures even if extension not in list */
  altClickCapture: true,
  /** Catch link clicks and open the download dialog (IDM Integration style) */
  linkClickCapture: true,
  /** Also catch YouTube / video page links on click (off — use player banner instead) */
  catchVideoPageLinks: false,
  /** Show floating media grabber for video/audio on page */
  mediaGrabber: true,
  /** IDM-style "Download this video" banner with resolution menu */
  videoBanner: true,
  /** Intercept chrome.downloads and redirect to DM */
  interceptBrowserDownloads: true,
  segments: 16,
  saveDir: '',
};

export function filenameFromUrl(url) {
  try {
    const u = new URL(url);
    const last = u.pathname.split('/').filter(Boolean).pop() || 'download';
    return decodeURIComponent(last.split('?')[0]) || 'download';
  } catch {
    return 'download';
  }
}

export function extensionOf(urlOrName) {
  const name = filenameFromUrl(urlOrName);
  const i = name.lastIndexOf('.');
  if (i < 0) return '';
  return name.slice(i + 1).toLowerCase();
}

export function shouldCapture(url, settings, { force = false } = {}) {
  if (force) return true;
  if (!settings?.enabled) return false;
  if (settings.captureMode === 'off') return false;
  const ext = extensionOf(url);
  if (!ext) return false;
  const list = settings.extensions || DEFAULT_EXTENSIONS;
  return list.includes(ext);
}

export async function loadSettings() {
  const stored = await chrome.storage.sync.get('settings');
  return { ...DEFAULT_SETTINGS, ...(stored.settings || {}) };
}

export async function saveSettings(partial) {
  const current = await loadSettings();
  const next = { ...current, ...partial };
  await chrome.storage.sync.set({ settings: next });
  return next;
}
