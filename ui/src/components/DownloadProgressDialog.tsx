import { useDownloads, type DownloadJob } from '../hooks/useDownloads'
import { formatBytes, formatSpeed, formatEta } from '../lib/format'
import { STATUS_LABELS } from './DownloadList'

export function DownloadProgressDialog({
  job,
  jobId,
  onClose,
  onPause,
  onResume,
}: {
  job?: DownloadJob
  jobId?: string
  onClose?: () => void
  onPause?: (id: string) => void
  onResume?: (id: string) => void
}) {
  if (job) {
    return (
      <ProgressBody
        job={job}
        onClose={onClose}
        onPause={() => onPause?.(job.id)}
        onResume={() => onResume?.(job.id)}
      />
    )
  }
  return <StandaloneProgress jobId={jobId} onClose={onClose} />
}

/** Separate window (?dialog=progress) — loads its own daemon connection. */
function StandaloneProgress({ jobId, onClose }: { jobId?: string; onClose?: () => void }) {
  const searchParams = new URLSearchParams(window.location.search)
  const id = jobId || searchParams.get('id')
  const { jobs, pauseDownload, resumeDownload } = useDownloads()
  const job = jobs.find(j => j.id === id)

  if (!job) {
    return (
      <div style={{ padding: '20px', fontFamily: 'sans-serif' }}>
        Loading…
        {onClose && <button className="btn btn-ghost" onClick={onClose} style={{ marginLeft: '10px' }}>Close</button>}
      </div>
    )
  }

  return (
    <ProgressBody
      job={job}
      onClose={onClose}
      onPause={() => { void pauseDownload(job.id) }}
      onResume={() => { void resumeDownload(job.id) }}
    />
  )
}

function ProgressBody({
  job,
  onClose,
  onPause,
  onResume,
}: {
  job: DownloadJob
  onClose?: () => void
  onPause: () => void
  onResume: () => void
}) {

  const pct = job.total_bytes > 0 ? (job.downloaded_bytes / job.total_bytes) * 100 : 0
  const isActive = job.status === 'downloading' || job.status === 'probing'
  const isPaused = job.status === 'paused' || job.status === 'failed'

  const handlePauseResume = () => {
    if (isActive) onPause()
    else if (isPaused) onResume()
  }

  const handleClose = () => {
    if (onClose) onClose()
    else window.close()
  }

  const pctLabel = job.phase === 'merging'
    ? 'Merging'
    : isActive && pct >= 100
      ? 'Finishing'
      : `${Math.min(pct, 100).toFixed(1)}%`

  return (
    <div className="progress-dialog">
      {onClose && (
        <div className="progress-dialog-head">
          <h2 className="progress-dialog-title">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
              <polyline points="7 10 12 15 17 10" />
              <line x1="12" y1="15" x2="12" y2="3" />
            </svg>
            Download progress
          </h2>
          <button className="btn btn-ghost btn-sm btn-icon" onClick={onClose} aria-label="Close">✕</button>
        </div>
      )}

      <div className="progress-dialog-body">
        <div className="progress-cols">
          <div>
            <div className="progress-kicker">File</div>
            <div className="progress-line"><span>Name</span><span title={job.filename}>{job.filename}</span></div>
            <div className="progress-line"><span>URL</span><span title={job.url}>{job.url}</span></div>
            <div className="progress-line"><span>Status</span><span className={isActive ? 'active' : ''}>{STATUS_LABELS[job.status]}</span></div>
          </div>
          <div>
            <div className="progress-kicker">Transfer</div>
            <div className="progress-line"><span>Size</span><span>{job.total_bytes > 0 ? formatBytes(job.total_bytes) : 'Unknown'}</span></div>
            <div className="progress-line"><span>Done</span><span>{formatBytes(job.downloaded_bytes)}</span></div>
            <div className="progress-line"><span>Speed</span><span className="mono">{isActive ? formatSpeed(job.speed_bps) : '—'}</span></div>
            <div className="progress-line"><span>Time left</span><span className="mono">{isActive ? formatEta(job.eta_seconds) : '—'}</span></div>
          </div>
        </div>

        <div>
          <div className="progress-bar-head">
            <span>
              Progress
              {job.segments.length > 0 && <span className="seg-chip">{job.segments.length} segments</span>}
            </span>
            <span className={isActive ? 'active' : ''}>{pctLabel}</span>
          </div>
          <div className="progress-track-lg">
            {job.total_bytes > 0 && job.segments.length > 0 ? (
              job.segments.map(seg => {
                const segStart = seg.start
                const segEnd = seg.end > 0 ? seg.end : job.total_bytes - 1
                const segTotal = segEnd - segStart + 1
                const startPct = (segStart / job.total_bytes) * 100
                const widthPct = (segTotal / job.total_bytes) * 100
                const downloadedPct = segTotal > 0 ? (seg.downloaded / segTotal) * 100 : 0
                return (
                  <div key={seg.id} className="progress-seg" style={{ left: `${startPct}%`, width: `${widthPct}%` }}>
                    <div className={`progress-seg-fill ${isActive ? '' : 'idle'}`} style={{ width: `${Math.min(downloadedPct, 100)}%` }} />
                  </div>
                )
              })
            ) : (
              <div className={`progress-fill-lg ${isActive ? '' : 'idle'}`} style={{ width: `${Math.min(pct, 100)}%` }} />
            )}
          </div>
        </div>
      </div>

      <div className="progress-dialog-foot">
        <button className="btn btn-ghost" onClick={handleClose}>Close</button>
        <button className="btn btn-primary" onClick={handlePauseResume} disabled={job.status === 'completed'}>
          {isActive ? 'Pause' : 'Resume'}
        </button>
      </div>
    </div>
  )
}
