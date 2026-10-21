import { DEFAULT_SETTINGS, loadSettings, saveSettings } from '../shared/config.js';

const els = {
  enabled: document.getElementById('enabled'),
  captureMode: document.getElementById('captureMode'),
  interceptBrowserDownloads: document.getElementById('interceptBrowserDownloads'),
  linkClickCapture: document.getElementById('linkClickCapture'),
  catchVideoPageLinks: document.getElementById('catchVideoPageLinks'),
  videoBanner: document.getElementById('videoBanner'),
  mediaGrabber: document.getElementById('mediaGrabber'),
  altSkipsCapture: document.getElementById('altSkipsCapture'),
  insertForcesCapture: document.getElementById('insertForcesCapture'),
  segments: document.getElementById('segments'),
  extensions: document.getElementById('extensions'),
  daemon: document.getElementById('daemon'),
  daemonText: document.getElementById('daemonText'),
  save: document.getElementById('save'),
  saved: document.getElementById('saved'),
  features: document.getElementById('features'),
  offNotice: document.getElementById('offNotice'),
};

/** Sub-settings that only make sense while master capture is on. */
const dependentFields = [
  els.captureMode,
  els.interceptBrowserDownloads,
  els.linkClickCapture,
  els.altSkipsCapture,
  els.insertForcesCapture,
  els.catchVideoPageLinks,
  els.videoBanner,
  els.mediaGrabber,
  els.segments,
  els.extensions,
];

function applyMasterState() {
  const on = els.enabled.checked;
  els.features.classList.toggle('is-disabled', !on);
  els.offNotice.hidden = on;
  for (const field of dependentFields) {
    field.disabled = !on;
  }
}

els.enabled.addEventListener('change', applyMasterState);

async function refreshDaemon() {
  const res = await chrome.runtime.sendMessage({ type: 'CHECK_DAEMON' });
  if (res?.ok) {
    els.daemonText.textContent = `App connected · v${res.version || '?'}`;
    els.daemon.className = 'daemon ok';
  } else {
    els.daemonText.textContent = res?.error || 'App not running — start Download Manager Pro';
    els.daemon.className = 'daemon bad';
  }
}

async function hydrate() {
  const s = await loadSettings();
  els.enabled.checked = s.enabled;
  els.captureMode.value = s.captureMode;
  els.interceptBrowserDownloads.checked = s.interceptBrowserDownloads;
  els.linkClickCapture.checked = s.linkClickCapture !== false;
  els.catchVideoPageLinks.checked = s.catchVideoPageLinks === true;
  els.videoBanner.checked = s.videoBanner !== false;
  els.mediaGrabber.checked = s.mediaGrabber;
  els.altSkipsCapture.checked = s.altSkipsCapture !== false;
  els.insertForcesCapture.checked = s.insertForcesCapture !== false;
  els.segments.value = String(s.segments || 16);
  els.extensions.value = (s.extensions || DEFAULT_SETTINGS.extensions).join(', ');
  applyMasterState();
  await refreshDaemon();
}

els.save.addEventListener('click', async () => {
  const extensions = els.extensions.value
    .split(/[\s,;]+/)
    .map((x) => x.replace(/^\./, '').toLowerCase())
    .filter(Boolean);

  await saveSettings({
    enabled: els.enabled.checked,
    captureMode: els.captureMode.value,
    interceptBrowserDownloads: els.interceptBrowserDownloads.checked,
    linkClickCapture: els.linkClickCapture.checked,
    catchVideoPageLinks: els.catchVideoPageLinks.checked,
    videoBanner: els.videoBanner.checked,
    mediaGrabber: els.mediaGrabber.checked,
    altSkipsCapture: els.altSkipsCapture.checked,
    insertForcesCapture: els.insertForcesCapture.checked,
    segments: Number(els.segments.value) || 16,
    extensions: extensions.length ? extensions : DEFAULT_SETTINGS.extensions,
  });

  els.saved.hidden = false;
  setTimeout(() => { els.saved.hidden = true; }, 1200);
});

hydrate();
