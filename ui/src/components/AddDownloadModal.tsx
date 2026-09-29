import { useState, useRef, useEffect } from 'react'
import { formatBytes } from '../lib/format'

interface Props {
  onAdd: (url: string, segments: number, saveDir?: string, quality?: string, filename?: string, totalBytesHint?: number) => Promise<void>
  onClose: () => void
  defaultSegments?: number
  initialUrl?: string
  initialFilename?: string
  initialSize?: number
  initialQuality?: string
}

function isYouTubeUrl(url: string): boolean {
  try {
    const u = new URL(url)
    const host = u.hostname.toLowerCase()
    if (host === 'youtu.be') return true
    if (host.includes('youtube.com')) {
      return (
        u.pathname.startsWith('/watch') ||
        u.pathname.startsWith('/shorts/') ||
        u.pathname.startsWith('/embed/') ||
        u.pathname.startsWith('/live/') ||
        u.searchParams.has('v')
      )
    }
  } catch {
    /* ignore */
  }
  return false
}

export function AddDownloadModal({
  onAdd,
  onClose,
  defaultSegments = 16,
  initialUrl = '',
  initialFilename = '',
  initialSize,
  initialQuality = 'best',
}: Props) {
  const [url, setUrl] = useState(initialUrl)
  const [filename, setFilename] = useState(initialFilename)
  const [segments, setSegments] = useState(defaultSegments)
  const [saveDir, setSaveDir] = useState('')
  const [quality] = useState(initialQuality)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)
  const youtube = isYouTubeUrl(url.trim())

  useEffect(() => {
    inputRef.current?.focus()
    if (!initialUrl) {
      navigator.clipboard.readText().then(text => {
        if (text.startsWith('http://') || text.startsWith('https://')) {
          setUrl(text)
        }
      }).catch(() => {})
    }
  }, [initialUrl])

  async function handleBrowseFolder() {
    try {
      // Calling the backend folder selector API we defined
      const resp = await fetch('http://127.0.0.1:7373/dialog/folder')
      if (resp.ok) {
        const { path } = await resp.json()
        if (path) {
          setSaveDir(path)
        }
      }
    } catch (e) {
      console.error(e)
    }
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (!url.trim()) return

    setError('')
    setLoading(true)
    try {
      await onAdd(
        url.trim(),
        segments,
        saveDir.trim() || undefined,
        youtube ? quality : undefined,
        filename.trim() || undefined,
        initialSize
      )
      onClose()
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : 'Failed to start download')
    } finally {
      setLoading(false)
    }
  }

  function handleOverlayClick(e: React.MouseEvent) {
    if (e.target === e.currentTarget) onClose()
  }

  function handleDownloadBrowser() {
    if (url.trim()) {
      window.open(url.trim(), '_blank')
      onClose()
    }
  }

  return (
    <div className="modal-overlay" onClick={handleOverlayClick}>
      <div className="modal modal-add" role="dialog" aria-modal="true" aria-labelledby="modal-title">
        <div className="modal-header">
          <div className="modal-title-row">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
              <polyline points="7 10 12 15 17 10" />
              <line x1="12" y1="15" x2="12" y2="3" />
            </svg>
            <h2 className="modal-title" id="modal-title">Add download</h2>
          </div>
          <button className="btn btn-ghost btn-sm btn-icon" onClick={onClose} aria-label="Close">✕</button>
        </div>

        <form onSubmit={handleSubmit}>
          <div className="form-group">
            <label className="form-label" htmlFor="dl-url">Address</label>
            <input
              id="dl-url"
              ref={inputRef}
              className="form-input"
              type="url"
              placeholder="https://example.com/file.zip"
              value={url}
              onChange={e => setUrl(e.target.value)}
              required
            />
          </div>

          <div className="form-group">
            <label className="form-label" htmlFor="dl-filename">File name</label>
            <input
              id="dl-filename"
              className="form-input"
              type="text"
              placeholder="video.mp4"
              value={filename}
              onChange={e => setFilename(e.target.value)}
            />
          </div>

          <div className="form-row">
            <div className="form-group">
              <label className="form-label" htmlFor="dl-segments">Segments</label>
              <input
                id="dl-segments"
                className="form-input"
                type="number"
                min={1}
                max={32}
                value={segments}
                onChange={e => setSegments(Number(e.target.value))}
              />
            </div>

            <div className="form-group">
              <label className="form-label" htmlFor="dl-savedir">Save folder</label>
              <div className="input-with-btn">
                <input
                  id="dl-savedir"
                  className="form-input"
                  type="text"
                  placeholder="Default downloads"
                  value={saveDir}
                  onChange={e => setSaveDir(e.target.value)}
                />
                <button type="button" className="icon-btn" onClick={handleBrowseFolder} aria-label="Browse folder">
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
                  </svg>
                </button>
              </div>
            </div>
          </div>

          {(initialSize || youtube) && (
            <div className="processor-note">
              {youtube ? 'YouTube' : 'Direct file'}
              <span>·</span>
              {quality !== 'best' ? quality : (youtube ? '1080' : 'Direct')}
              {initialSize ? (
                <>
                  <span>·</span>
                  ~{formatBytes(initialSize)}
                </>
              ) : null}
            </div>
          )}

          {error && (
            <div className="form-error" role="alert">{error}</div>
          )}

          <div className="modal-actions">
            <button type="button" className="btn btn-ghost" onClick={handleDownloadBrowser}>
              Open in browser
            </button>
            <div className="modal-actions-end">
              <button type="button" className="btn btn-ghost" onClick={onClose}>Cancel</button>
              <button
                type="submit"
                className="btn btn-primary"
                disabled={loading || !url.trim()}
                id="start-download-btn"
              >
                {loading ? 'Starting…' : 'Start download'}
              </button>
            </div>
          </div>
        </form>
      </div>
    </div>
  )
}
