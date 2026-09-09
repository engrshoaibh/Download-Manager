import { useState, useRef, useEffect } from 'react'

interface Props {
  onAdd: (url: string, segments: number, saveDir?: string) => Promise<void>
  onClose: () => void
}

const DEFAULT_SEGMENTS = 16

export function AddDownloadModal({ onAdd, onClose }: Props) {
  const [url, setUrl]           = useState('')
  const [segments, setSegments] = useState(DEFAULT_SEGMENTS)
  const [saveDir, setSaveDir]   = useState('')
  const [loading, setLoading]   = useState(false)
  const [error, setError]       = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    inputRef.current?.focus()

    // Auto-fill from clipboard if it looks like a URL
    navigator.clipboard.readText().then(text => {
      if (text.startsWith('http://') || text.startsWith('https://')) {
        setUrl(text)
      }
    }).catch(() => {})
  }, [])

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (!url.trim()) return

    setError('')
    setLoading(true)
    try {
      await onAdd(url.trim(), segments, saveDir.trim() || undefined)
      onClose()
    } catch (err: any) {
      setError(err.message ?? 'Failed to start download')
    } finally {
      setLoading(false)
    }
  }

  function handleOverlayClick(e: React.MouseEvent) {
    if (e.target === e.currentTarget) onClose()
  }

  return (
    <div className="modal-overlay" onClick={handleOverlayClick}>
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby="modal-title">
        <div className="modal-header">
          <h2 className="modal-title" id="modal-title">
            ⬇️ New Download
          </h2>
          <button className="btn btn-ghost btn-sm btn-icon" onClick={onClose} aria-label="Close">✕</button>
        </div>

        <form onSubmit={handleSubmit}>
          {/* URL */}
          <div className="form-group">
            <label className="form-label" htmlFor="dl-url">Download URL</label>
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

          {/* Segments */}
          <div className="form-group">
            <label className="form-label" htmlFor="dl-segments">
              Parallel Segments
              <span style={{ marginLeft: '0.5rem', color: 'var(--color-accent)', fontFamily: 'JetBrains Mono' }}>
                {segments}
              </span>
            </label>
            <input
              id="dl-segments"
              className="form-input"
              type="range"
              min={1} max={32} step={1}
              value={segments}
              onChange={e => setSegments(Number(e.target.value))}
              style={{ padding: '0.3rem 0', cursor: 'pointer' }}
            />
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span className="form-hint">1 (single stream)</span>
              <span className="form-hint">32 (maximum)</span>
            </div>
          </div>

          {/* Save directory (optional) */}
          <div className="form-group">
            <label className="form-label" htmlFor="dl-savedir">Save to (optional)</label>
            <input
              id="dl-savedir"
              className="form-input"
              type="text"
              placeholder="Default: Downloads folder"
              value={saveDir}
              onChange={e => setSaveDir(e.target.value)}
            />
            <span className="form-hint">Leave blank to use your system Downloads folder</span>
          </div>

          {error && (
            <div style={{
              background: 'var(--color-danger-dim)',
              border: '1px solid rgba(248,113,113,0.3)',
              borderRadius: 'var(--radius-md)',
              padding: '0.65rem 1rem',
              color: 'var(--color-danger)',
              fontSize: '0.85rem',
              marginBottom: '1.25rem',
            }}>
              ⚠️ {error}
            </div>
          )}

          <div style={{ display: 'flex', gap: '0.75rem', justifyContent: 'flex-end' }}>
            <button type="button" className="btn btn-ghost" onClick={onClose}>
              Cancel
            </button>
            <button
              type="submit"
              className="btn btn-primary"
              disabled={loading || !url.trim()}
              id="start-download-btn"
            >
              {loading ? '⏳ Starting…' : '⬇️ Start Download'}
            </button>
          </div>
        </form>
      </div>
    </div>
  )
}
