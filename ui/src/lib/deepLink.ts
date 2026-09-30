/**
 * `dmpro://download?url=<https link>&title=<name>` — lets the website start a
 * download in the app with one click, even when the app was closed.
 *
 * Only links pointing at the allowed website hosts are accepted; anything else is
 * ignored, so other sites cannot use the scheme to push downloads into the app.
 */
import { getCurrent, onOpenUrl } from '@tauri-apps/plugin-deep-link'

const DAEMON_URL = 'http://127.0.0.1:7373'

/** Origins the app accepts download links from. */
const ALLOWED_ORIGINS = new Set([
  'https://hdmoviesshow.com',
  'https://www.hdmoviesshow.com',
  'http://localhost:3000',
])

/** True for downloads the website handed over (links on the allowed hosts). */
export function isWebsiteUrl(url: string): boolean {
  try {
    return ALLOWED_ORIGINS.has(new URL(url).origin)
  } catch {
    return false
  }
}

const DAEMON_WAIT_MS = 20_000

/** A link can arrive twice (startup + event, or React StrictMode) — handle it once. */
const handled = new Set<string>()

interface DeepLinkDownload {
  url: string
  title?: string
}

export function parseDeepLink(raw: string): DeepLinkDownload | null {
  let link: URL
  try {
    link = new URL(raw)
  } catch {
    return null
  }
  if (link.protocol !== 'dmpro:') return null
  // dmpro://download?... → host "download"; dmpro:download?... → pathname "download"
  const action = link.hostname || link.pathname.replace(/^\/+/, '')
  if (action !== 'download') return null

  const target = link.searchParams.get('url')
  if (!target) return null
  let targetUrl: URL
  try {
    targetUrl = new URL(target)
  } catch {
    return null
  }
  if (!ALLOWED_ORIGINS.has(targetUrl.origin)) return null

  const title = link.searchParams.get('title')?.trim()
  return { url: targetUrl.toString(), title: title || undefined }
}

/** The daemon starts with the app — wait for it when the link launched the app. */
async function waitForDaemon(): Promise<boolean> {
  const deadline = Date.now() + DAEMON_WAIT_MS
  while (Date.now() < deadline) {
    try {
      await fetch(`${DAEMON_URL}/health`)
      return true
    } catch {
      await new Promise(resolve => setTimeout(resolve, 500))
    }
  }
  return false
}

async function startDownload(raw: string): Promise<void> {
  if (handled.has(raw)) return
  handled.add(raw)

  const download = parseDeepLink(raw)
  if (!download) {
    console.warn('Ignored deep link (not an allowed download link)')
    return
  }
  if (!(await waitForDaemon())) {
    console.error('Deep link: daemon did not start in time')
    return
  }
  const resp = await fetch(`${DAEMON_URL}/downloads`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      url: download.url,
      title: download.title,
      user_agent: navigator.userAgent,
    }),
  })
  if (!resp.ok) console.error('Deep link: daemon rejected the download', resp.status)
}

function startAll(urls: string[] | null | undefined): void {
  for (const url of urls ?? []) {
    startDownload(url).catch(console.error)
  }
}

/** Call once from the main window. Returns a cleanup function. */
export function initDeepLinks(): () => void {
  let disposed = false
  let unlisten: (() => void) | undefined

  // Link that launched the app.
  getCurrent().then(startAll).catch(console.error)

  // Links opened while the app is running (forwarded by the single-instance plugin).
  onOpenUrl(startAll)
    .then(fn => {
      if (disposed) fn()
      else unlisten = fn
    })
    .catch(console.error)

  return () => {
    disposed = true
    unlisten?.()
  }
}
