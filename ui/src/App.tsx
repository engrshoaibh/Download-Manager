import { useState } from 'react'
import { useDownloads } from './hooks/useDownloads'
import { DownloadCard } from './components/DownloadCard'
import { AddDownloadModal } from './components/AddDownloadModal'
import './index.css'

type FilterTab = 'all' | 'active' | 'completed' | 'failed'

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return `${(bytes / Math.pow(k, i)).toFixed(1)} ${sizes[i]}`
}

function formatSpeed(bps: number): string {
  return `${formatBytes(bps)}/s`
}

export default function App() {
  const { jobs, connected, addDownload, cancelDownload } = useDownloads()
  const [showModal, setShowModal] = useState(false)
  const [filter, setFilter] = useState<FilterTab>('all')

  // Counts
  const activeJobs    = jobs.filter(j => j.status === 'downloading' || j.status === 'probing')
  const completedJobs = jobs.filter(j => j.status === 'completed')
  const failedJobs    = jobs.filter(j => j.status === 'failed')

  // Aggregate stats
  const totalSpeed    = activeJobs.reduce((acc, j) => acc + j.speed_bps, 0)
  const totalReceived = jobs.reduce((acc, j) => acc + j.downloaded_bytes, 0)

  // Filtered view
  const filteredJobs = jobs.filter(j => {
    switch (filter) {
      case 'active':    return j.status === 'downloading' || j.status === 'probing' || j.status === 'queued'
      case 'completed': return j.status === 'completed'
      case 'failed':    return j.status === 'failed' || j.status === 'cancelled'
      default:          return true
    }
  })

  // Sort: active first, then by created_at desc
  const sortedJobs = [...filteredJobs].sort((a, b) => {
    const aPrio = a.status === 'downloading' ? 0 : a.status === 'probing' ? 1 : a.status === 'queued' ? 2 : 3
    const bPrio = b.status === 'downloading' ? 0 : b.status === 'probing' ? 1 : b.status === 'queued' ? 2 : 3
    if (aPrio !== bPrio) return aPrio - bPrio
    return new Date(b.created_at).getTime() - new Date(a.created_at).getTime()
  })

  async function handleAdd(url: string, segments: number, saveDir?: string) {
    await addDownload(url, segments, saveDir)
  }

  return (
    <div className="app">
      {/* Topbar */}
      <header className="topbar">
        <div className="topbar-brand">
          <div className="logo-icon">⚡</div>
          <h1>Download Manager</h1>
        </div>

        <div className="topbar-stats">
          <div>
            <span className="conn-dot" style={!connected ? { background: 'var(--color-danger)' } : {}} />
            {' '}
            <span className="stat-value">{connected ? 'Connected' : 'Connecting…'}</span>
          </div>
          <div>
            Speed: <span className="stat-value">{formatSpeed(totalSpeed)}</span>
          </div>
          <div>
            Downloaded: <span className="stat-value">{formatBytes(totalReceived)}</span>
          </div>
          <div>
            Active: <span className="stat-value">{activeJobs.length}</span>
          </div>
        </div>

        <button
          className="btn btn-primary"
          id="add-download-btn"
          onClick={() => setShowModal(true)}
        >
          ＋ New Download
        </button>
      </header>

      {/* Main */}
      <div className="main-content">
        {/* Sidebar */}
        <nav className="sidebar" aria-label="Filter downloads">
          <div className="sidebar-label">Downloads</div>

          {([ ['all', 'All', '📋'],
               ['active', 'Active', '⬇️'],
               ['completed', 'Completed', '✅'],
               ['failed', 'Failed', '❌'] ] as [FilterTab, string, string][]).map(([key, label, icon]) => (
            <button
              key={key}
              className={`sidebar-item ${filter === key ? 'active' : ''}`}
              onClick={() => setFilter(key)}
              id={`filter-${key}`}
            >
              <span className="icon">{icon}</span>
              {label}
              <span className="badge">
                {key === 'all'       ? jobs.length
                : key === 'active'   ? activeJobs.length
                : key === 'completed'? completedJobs.length
                :                     failedJobs.length}
              </span>
            </button>
          ))}

          {/* Stats cards below nav */}
          <div style={{ marginTop: '2rem', display: 'flex', flexDirection: 'column', gap: '0.75rem' }}>
            <div className="sidebar-label">Performance</div>

            <div className="stat-card">
              <div className="stat-card-label">Live Speed</div>
              <div className="stat-card-value" style={{ fontSize: '1.1rem' }}>
                {formatSpeed(totalSpeed)}
              </div>
              <div className="stat-card-sub">across {activeJobs.length} file(s)</div>
            </div>

            <div className="stat-card">
              <div className="stat-card-label">Total Received</div>
              <div className="stat-card-value" style={{ fontSize: '1.1rem' }}>
                {formatBytes(totalReceived)}
              </div>
              <div className="stat-card-sub">{completedJobs.length} completed</div>
            </div>
          </div>
        </nav>

        {/* Download Panel */}
        <main className="download-panel" aria-label="Download list">
          <div className="panel-header">
            <h2 style={{ fontSize: '1rem', fontWeight: 600, color: 'var(--color-text-muted)' }}>
              {sortedJobs.length} {filter === 'all' ? 'total' : filter} download{sortedJobs.length !== 1 ? 's' : ''}
            </h2>
          </div>

          {sortedJobs.length === 0 ? (
            <div className="empty-state">
              <div className="empty-icon">📥</div>
              <h3>No downloads yet</h3>
              <p>
                {filter === 'all'
                  ? 'Click "＋ New Download" to add your first download.'
                  : `No ${filter} downloads.`}
              </p>
              {filter === 'all' && (
                <button className="btn btn-primary" onClick={() => setShowModal(true)}>
                  ＋ New Download
                </button>
              )}
            </div>
          ) : (
            sortedJobs.map(job => (
              <DownloadCard
                key={job.id}
                job={job}
                onCancel={cancelDownload}
              />
            ))
          )}
        </main>
      </div>

      {/* Modal */}
      {showModal && (
        <AddDownloadModal
          onAdd={handleAdd}
          onClose={() => setShowModal(false)}
        />
      )}
    </div>
  )
}
