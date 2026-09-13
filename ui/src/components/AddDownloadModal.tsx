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
          <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
            <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ color: '#ff9800' }}>
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
              <polyline points="7 10 12 15 17 10" />
              <line x1="12" y1="15" x2="12" y2="3" />
            </svg>
            <h2 className="modal-title" id="modal-title" style={{ fontSize: '1.1rem', fontWeight: 500, margin: 0, opacity: 0.9 }}>Send to Download Manager</h2>
          </div>
          <button className="btn btn-ghost btn-sm btn-icon" onClick={onClose} aria-label="Close">✕</button>
        </div>

        <form onSubmit={handleSubmit} style={{ padding: '0 20px 20px 20px', display: 'flex', flexDirection: 'column', gap: '16px' }}>
          
          <div className="form-group" style={{ marginBottom: 0 }}>
            <label className="form-label" htmlFor="dl-url" style={{ textTransform: 'uppercase', fontSize: '0.75rem', opacity: 0.7, letterSpacing: '0.5px' }}>Address</label>
            <input
              id="dl-url"
              ref={inputRef}
              className="form-input"
              type="url"
              placeholder="https://example.com/file.zip"
              value={url}
              onChange={e => setUrl(e.target.value)}
              required
              style={{ backgroundColor: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.1)', padding: '10px 12px' }}
            />
          </div>

          <div className="form-group" style={{ marginBottom: 0 }}>
            <label className="form-label" htmlFor="dl-filename" style={{ textTransform: 'uppercase', fontSize: '0.75rem', opacity: 0.7, letterSpacing: '0.5px' }}>File Name</label>
            <input
              id="dl-filename"
              className="form-input"
              type="text"
              placeholder="video.mp4"
              value={filename}
              onChange={e => setFilename(e.target.value)}
              style={{ backgroundColor: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.1)', padding: '10px 12px' }}
            />
          </div>

          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1.5fr', gap: '16px' }}>
            <div className="form-group" style={{ marginBottom: 0 }}>
              <label className="form-label" htmlFor="dl-segments" style={{ textTransform: 'uppercase', fontSize: '0.75rem', opacity: 0.7, letterSpacing: '0.5px' }}>Segments</label>
              <input
                id="dl-segments"
                className="form-input"
                type="number"
                min={1}
                max={32}
                value={segments}
                onChange={e => setSegments(Number(e.target.value))}
                style={{ backgroundColor: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.1)', padding: '10px 12px' }}
              />
            </div>

            <div className="form-group" style={{ marginBottom: 0 }}>
              <label className="form-label" htmlFor="dl-savedir" style={{ textTransform: 'uppercase', fontSize: '0.75rem', opacity: 0.7, letterSpacing: '0.5px' }}>Save Folder (Optional)</label>
              <div style={{ display: 'flex', gap: '8px' }}>
                <input
                  id="dl-savedir"
                  className="form-input"
                  type="text"
                  placeholder="Default Downloads"
                  value={saveDir}
                  onChange={e => setSaveDir(e.target.value)}
                  style={{ flex: 1, backgroundColor: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.1)', padding: '10px 12px' }}
                />
                <button type="button" className="btn" onClick={handleBrowseFolder} style={{ backgroundColor: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.1)', padding: '0 12px' }}>
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ color: '#ffb300' }}>
                    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
                  </svg>
                </button>
              </div>
            </div>
          </div>

          {(initialSize || youtube) && (
            <div style={{ 
              marginTop: '8px',
              padding: '12px 16px', 
              backgroundColor: 'rgba(0, 200, 120, 0.05)', 
              border: '1px solid rgba(0, 200, 120, 0.4)', 
              borderRadius: '6px',
              color: 'rgb(0, 220, 140)',
              fontSize: '0.9rem',
              display: 'flex',
              alignItems: 'center',
              gap: '8px'
            }}>
              {youtube ? 'YouTube processor' : 'File processor'}
              <span style={{ opacity: 0.5 }}>·</span>
              {quality !== 'best' ? quality : (youtube ? '1080' : 'Direct')}
              {initialSize ? (
                <>
                  <span style={{ opacity: 0.5 }}>·</span>
                  ~{formatBytes(initialSize)}
                </>
              ) : null}
            </div>
          )}

          {error && (
            <div className="form-error" role="alert" style={{ marginTop: '8px' }}>
              {error}
            </div>
          )}

          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginTop: '16px', paddingTop: '16px', borderTop: '1px solid rgba(255,255,255,0.05)' }}>
            <button type="button" className="btn" onClick={handleDownloadBrowser} style={{ backgroundColor: 'rgba(255,255,255,0.05)', border: '1px solid rgba(255,255,255,0.1)' }}>
              Download with browser
            </button>
            <div style={{ display: 'flex', gap: '12px' }}>
              <button type="button" className="btn" onClick={onClose} style={{ backgroundColor: 'rgba(255,255,255,0.05)', border: '1px solid rgba(255,255,255,0.1)' }}>
                Cancel
              </button>
              <button
                type="submit"
                className="btn btn-primary"
                disabled={loading || !url.trim()}
                id="start-download-btn"
                style={{ backgroundColor: '#f57c00', color: '#fff', border: 'none', padding: '8px 24px', fontWeight: 500 }}
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
