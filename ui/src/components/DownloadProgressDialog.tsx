import { useDownloads } from '../hooks/useDownloads'
import { formatBytes, formatSpeed, formatEta } from '../lib/format'
import { STATUS_LABELS } from './DownloadList'

export function DownloadProgressDialog({ jobId, onClose }: { jobId?: string, onClose?: () => void }) {
  const searchParams = new URLSearchParams(window.location.search)
  const id = jobId || searchParams.get('id')
  
  const { jobs, pauseDownload, resumeDownload, cancelDownload } = useDownloads()
  const job = jobs.find(j => j.id === id)

  if (!job) {
    return (
      <div style={{ padding: '20px', fontFamily: 'sans-serif' }}>
        Loading or Download not found...
        {onClose && <button className="btn btn-ghost" onClick={onClose} style={{ marginLeft: '10px' }}>Close</button>}
      </div>
    )
  }

  const pct = job.total_bytes > 0 ? (job.downloaded_bytes / job.total_bytes) * 100 : 0
  const isActive = job.status === 'downloading' || job.status === 'probing'
  const isPaused = job.status === 'paused' || job.status === 'failed'

  const handlePauseResume = () => {
    if (isActive) {
      void pauseDownload(job.id)
    } else if (isPaused) {
      void resumeDownload(job.id)
    }
  }

  const handleCancel = () => {
    void cancelDownload(job.id, false)
    if (onClose) onClose()
    else window.close()
  }

  return (
    <div className="progress-dialog" style={{ display: 'flex', flexDirection: 'column', height: '100%', boxSizing: 'border-box' }}>
      {onClose && (
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', padding: '12px 16px', borderBottom: '1px solid rgba(255, 255, 255, 0.05)' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#f57c00" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
              <polyline points="7 10 12 15 17 10" />
              <line x1="12" y1="15" x2="12" y2="3" />
            </svg>
            <h2 style={{ margin: 0, fontSize: '15px', fontWeight: 600 }}>Download Progress</h2>
          </div>
          <button className="btn btn-ghost btn-sm btn-icon" onClick={onClose} aria-label="Close">✕</button>
        </div>
      )}

      <div style={{ padding: '16px', flexGrow: 1, display: 'flex', flexDirection: 'column', gap: '20px' }}>
        
        <div style={{ display: 'grid', gridTemplateColumns: 'minmax(0, 1.4fr) minmax(0, 1fr)', gap: '16px' }}>
          <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
            <div style={{ fontSize: '11px', opacity: 0.5, textTransform: 'uppercase', letterSpacing: '0.5px' }}>File Info</div>
            <div style={{ fontSize: '13px', display: 'flex', flexDirection: 'column', gap: '6px' }}>
              <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
                <span style={{ width: '50px', opacity: 0.6, flexShrink: 0 }}>Name:</span>
                <span style={{ fontWeight: 500, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', minWidth: 0, flex: 1 }} title={job.filename}>{job.filename}</span>
              </div>
              <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
                <span style={{ width: '50px', opacity: 0.6, flexShrink: 0 }}>URL:</span>
                <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', opacity: 0.8, minWidth: 0, flex: 1 }} title={job.url}>{job.url}</span>
              </div>
              <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
                <span style={{ width: '50px', opacity: 0.6, flexShrink: 0 }}>Status:</span>
                <span style={{ fontWeight: 500, color: isActive ? '#f57c00' : 'inherit', minWidth: 0, flex: 1 }}>{STATUS_LABELS[job.status]}</span>
              </div>
            </div>
          </div>

          <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
            <div style={{ fontSize: '11px', opacity: 0.5, textTransform: 'uppercase', letterSpacing: '0.5px' }}>Transfer Stats</div>
            <div style={{ fontSize: '13px', display: 'flex', flexDirection: 'column', gap: '6px' }}>
              <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
                <span style={{ width: '85px', opacity: 0.6, flexShrink: 0 }}>Size:</span>
                <span style={{ whiteSpace: 'nowrap' }}>{job.total_bytes > 0 ? formatBytes(job.total_bytes) : 'Unknown'}</span>
              </div>
              <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
                <span style={{ width: '85px', opacity: 0.6, flexShrink: 0 }}>Downloaded:</span>
                <span style={{ fontWeight: 500, whiteSpace: 'nowrap' }}>{formatBytes(job.downloaded_bytes)}</span>
              </div>
              <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
                <span style={{ width: '85px', opacity: 0.6, flexShrink: 0 }}>Speed:</span>
                <span className="mono" style={{ whiteSpace: 'nowrap' }}>{isActive ? formatSpeed(job.speed_bps) : '—'}</span>
              </div>
              <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
                <span style={{ width: '85px', opacity: 0.6, flexShrink: 0 }}>Time Left:</span>
                <span className="mono" style={{ whiteSpace: 'nowrap' }}>{isActive ? formatEta(job.eta_seconds) : '—'}</span>
              </div>
            </div>
          </div>
        </div>

        <div style={{ flexGrow: 1, display: 'flex', flexDirection: 'column', justifyContent: 'flex-end', paddingBottom: '8px' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: '8px', fontSize: '13px', fontWeight: 500 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
              <span>Progress</span>
              {job.segments.length > 0 && (
                <span style={{ fontSize: '11px', opacity: 0.6, fontWeight: 400, backgroundColor: 'rgba(255,255,255,0.05)', padding: '2px 6px', borderRadius: '4px' }}>
                  {job.segments.length} segments
                </span>
              )}
            </div>
            <span style={{ color: isActive ? '#f57c00' : 'inherit' }}>{Math.min(pct, 100).toFixed(2)}%</span>
          </div>
          
          <div style={{ width: '100%', height: '24px', backgroundColor: 'rgba(255,255,255,0.05)', borderRadius: '4px', overflow: 'hidden', position: 'relative', boxShadow: 'inset 0 1px 3px rgba(0,0,0,0.2)' }}>
            {job.total_bytes > 0 && job.segments.length > 0 ? (
              job.segments.map(seg => {
                const segStart = seg.start
                const segEnd = seg.end > 0 ? seg.end : job.total_bytes - 1
                const segTotal = segEnd - segStart + 1
                const startPct = (segStart / job.total_bytes) * 100
                const widthPct = (segTotal / job.total_bytes) * 100
                const downloadedPct = segTotal > 0 ? (seg.downloaded / segTotal) * 100 : 0
                
                return (
                  <div key={seg.id} style={{ position: 'absolute', left: `${startPct}%`, width: `${widthPct}%`, height: '100%', borderRight: '1px solid rgba(255,255,255,0.15)', boxSizing: 'border-box' }}>
                    <div style={{ width: `${Math.min(downloadedPct, 100)}%`, height: '100%', backgroundColor: isActive ? '#f57c00' : '#78909c', transition: 'width 0.2s', backgroundImage: 'linear-gradient(180deg, rgba(255,255,255,0.15) 0%, rgba(255,255,255,0) 100%)' }} />
                  </div>
                )
              })
            ) : (
              <div style={{ width: `${Math.min(pct, 100)}%`, height: '100%', backgroundColor: isActive ? '#f57c00' : '#78909c', transition: 'width 0.2s', backgroundImage: 'linear-gradient(180deg, rgba(255,255,255,0.15) 0%, rgba(255,255,255,0) 100%)' }}></div>
            )}
          </div>
        </div>

      </div>

      <div style={{ padding: '16px', backgroundColor: 'rgba(255,255,255,0.02)', borderTop: '1px solid rgba(255,255,255,0.05)', display: 'flex', justifyContent: 'flex-end', gap: '12px' }}>
        <button className="btn btn-primary" onClick={handlePauseResume} disabled={job.status === 'completed'} style={{ minWidth: '80px', backgroundColor: '#f57c00', color: 'white', border: 'none' }}>
          {isActive ? 'Pause' : 'Resume'}
        </button>
        <button className="btn btn-ghost" onClick={handleCancel} style={{ minWidth: '80px' }}>Cancel</button>
      </div>
    </div>
  )
}
