import type { DownloadJob, DownloadStatus } from '../hooks/useDownloads'

// ─── Helpers ──────────────────────────────────────────────────────────────────

function formatBytes(bytes: number, decimals = 1): string {
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return `${(bytes / Math.pow(k, i)).toFixed(decimals)} ${sizes[i]}`
}

function formatSpeed(bps: number): string {
  return `${formatBytes(bps)}/s`
}

function formatEta(secs: number | null): string {
  if (secs === null || secs <= 0) return '—'
  if (secs < 60) return `${Math.round(secs)}s`
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${Math.round(secs % 60)}s`
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`
}

const STATUS_LABELS: Record<DownloadStatus, string> = {
  queued:      'Queued',
  probing:     'Probing',
  downloading: 'Downloading',
  paused:      'Paused',
  completed:   'Complete',
  failed:      'Failed',
  cancelled:   'Cancelled',
}

const STATUS_ICONS: Record<DownloadStatus, string> = {
  queued:      '⏳',
  probing:     '🔍',
  downloading: '⬇️',
  paused:      '⏸',
  completed:   '✅',
  failed:      '❌',
  cancelled:   '🚫',
}

// ─── Component ────────────────────────────────────────────────────────────────

interface Props {
  job: DownloadJob
  onCancel: (id: string) => void
}

export function DownloadCard({ job, onCancel }: Props) {
  const pct = job.total_bytes > 0
    ? (job.downloaded_bytes / job.total_bytes) * 100
    : 0

  const isActive = job.status === 'downloading'
  const isDone   = job.status === 'completed'

  return (
    <div className="download-card">
      {/* Header */}
      <div className="card-header">
        <div style={{ minWidth: 0 }}>
          <div className="card-title" title={job.filename}>{job.filename}</div>
          <div className="card-url" title={job.url}>{job.url}</div>
        </div>
        <div className="card-actions">
          <span className={`status-badge ${job.status}`}>
            <span className="status-dot" />
            {STATUS_LABELS[job.status]}
          </span>
          {!isDone && job.status !== 'cancelled' && (
            <button
              className="btn btn-ghost btn-sm btn-icon"
              onClick={() => onCancel(job.id)}
              title="Cancel"
            >
              ✕
            </button>
          )}
        </div>
      </div>

      {/* Progress */}
      <div className="progress-row">
        <div className="progress-track">
          <div
            className={`progress-fill${isDone ? ' completed' : ''}`}
            style={{ width: `${Math.min(pct, 100)}%` }}
          />
        </div>
        <span className="progress-pct">
          {pct < 100 ? `${pct.toFixed(1)}%` : '100%'}
        </span>
      </div>

      {/* Segment mini-bars */}
      {job.segments.length > 1 && (
        <div className="segments-vis" title="Segment progress">
          {job.segments.map(seg => {
            const segSize = seg.end - seg.start + 1
            const segPct  = segSize > 0 ? (seg.downloaded / segSize) * 100 : 0
            return (
              <div key={seg.id} className="seg-bar">
                <div className="seg-fill" style={{ width: `${Math.min(segPct, 100)}%` }} />
              </div>
            )
          })}
        </div>
      )}

      {/* Meta */}
      <div className="card-meta">
        <div className="meta-item">
          <span>📦</span>
          <span className="meta-value">
            {job.total_bytes > 0
              ? `${formatBytes(job.downloaded_bytes)} / ${formatBytes(job.total_bytes)}`
              : '—'}
          </span>
        </div>
        {isActive && (
          <>
            <div className="meta-item">
              <span>⚡</span>
              <span className="meta-value">{formatSpeed(job.speed_bps)}</span>
            </div>
            <div className="meta-item">
              <span>⏱</span>
              <span className="meta-value">{formatEta(job.eta_seconds)}</span>
            </div>
          </>
        )}
        <div className="meta-item">
          <span>🔀</span>
          <span className="meta-value">{job.segment_count} segs</span>
        </div>
        {job.use_http3 && (
          <div className="meta-item">
            <span>⚡</span>
            <span className="meta-value" style={{ color: 'var(--color-accent)' }}>HTTP/3</span>
          </div>
        )}
        {isDone && job.completed_at && (
          <div className="meta-item">
            <span>✅</span>
            <span className="meta-value">
              {new Date(job.completed_at).toLocaleTimeString()}
            </span>
          </div>
        )}
      </div>
    </div>
  )
}
