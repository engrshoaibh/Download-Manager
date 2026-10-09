import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import packageJson from '../package.json'
import { useDownloads } from './hooks/useDownloads'
import { AddDownloadModal } from './components/AddDownloadModal'
import { DownloadList } from './components/DownloadList'
import { DeleteConfirmationModal } from './components/DeleteConfirmationModal'
import { DownloadProgressDialog } from './components/DownloadProgressDialog'
import { formatBytes, formatSpeed, matchesCategory, type CategoryId } from './lib/format'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { initDeepLinks, isWebsiteUrl } from './lib/deepLink'
import './index.css'

type MenuId = 'file' | 'downloads' | 'view' | 'help' | null

const CATEGORIES: { id: CategoryId; label: string }[] = [
  { id: 'all', label: 'All Downloads' },
  { id: 'unfinished', label: 'Unfinished' },
  { id: 'finished', label: 'Finished' },
]

const DEFAULT_SEGMENTS_KEY = 'dm.defaultSegments'

function readDefaultSegments(): number {
  const raw = localStorage.getItem(DEFAULT_SEGMENTS_KEY)
  const n = raw ? Number(raw) : 16
  return Number.isFinite(n) ? Math.min(32, Math.max(1, n)) : 16
}

export default function App() {
  const { jobs, connected, connection, addDownload, cancelDownload, pauseDownload, resumeDownload, stopAll, triggerUpdate, getUpdateStatus } = useDownloads()
  const [showAdd, setShowAdd] = useState(false)
  const [showSettings, setShowSettings] = useState(false)
  const [showAbout, setShowAbout] = useState(false)
  const [category, setCategory] = useState<CategoryId>('all')
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null)
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set())
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false)
  const [openMenu, setOpenMenu] = useState<MenuId>(null)
  const [defaultSegments, setDefaultSegments] = useState(readDefaultSegments)
  const [showProgressId, setShowProgressId] = useState<string | null>(null)
  const [updateStatus, setUpdateStatus] = useState<{ state: string; version?: string; message?: string } | null>(null)
  const menuRef = useRef<HTMLElement>(null)
  const lastClicked = useRef<string | null>(null)

  useEffect(() => {
    invoke('setup_native_messaging').catch(console.error)
  }, [])

  const checkForUpdates = async () => {
    try {
      setUpdateStatus({ state: 'checking', message: 'Checking for updates...' })
      const update = await check()
      if (update) {
        setUpdateStatus({ state: 'available', version: update.version, message: `Version ${update.version} is available!` })
      } else {
        setUpdateStatus({ state: 'latest', message: 'You are on the latest version.' })
      }
    } catch (error: any) {
      setUpdateStatus({ state: 'error', message: `Failed to check for updates: ${error.message || error}` })
    }
  }

  const installUpdate = async () => {
    try {
      setUpdateStatus({ state: 'downloading', message: 'Downloading update...' })
      const update = await check()
      if (update) {
        await update.downloadAndInstall((event) => {
          if (event.event === 'Progress') {
            setUpdateStatus({ state: 'downloading', message: `Downloading... ${event.data.chunkLength} bytes received` })
          }
        })
        setUpdateStatus({ state: 'done', message: 'Update installed. Restarting...' })
        await relaunch()
      }
    } catch (error: any) {
      setUpdateStatus({ state: 'error', message: `Update failed: ${error.message || error}` })
    }
  }

  // dmpro:// links from the website
  useEffect(() => initDeepLinks(), [])

  // A download sent by the website brings the window forward (it may be hidden in
  // the tray). Jobs from the extension or added by hand behave as before.
  const surfacedJobs = useRef<Set<string>>(new Set())
  useEffect(() => {
    for (const job of jobs) {
      if (surfacedJobs.current.has(job.id)) continue
      surfacedJobs.current.add(job.id)
      const ageMs = Date.now() - new Date(job.created_at).getTime()
      if (!isWebsiteUrl(job.url) || !(ageMs >= 0 && ageMs < 15_000)) continue
      const win = getCurrentWindow()
      win.show()
        .then(() => win.unminimize())
        .then(() => win.setFocus())
        .catch(console.error)
    }
  }, [jobs])

  const activeJobs = jobs.filter(j => j.status === 'downloading' || j.status === 'probing')
  const completedJobs = jobs.filter(j => j.status === 'completed')
  const totalSpeed = activeJobs.reduce((acc, j) => acc + j.speed_bps, 0)

  const filteredJobs = useMemo(() => {
    const list = jobs.filter(j => matchesCategory(j.filename, j.status, category))
    return list.sort((a, b) => {
      const rank = (s: string) =>
        s === 'downloading' ? 0 : s === 'probing' ? 1 : s === 'queued' || s === 'paused' ? 2 : 3
      const d = rank(a.status) - rank(b.status)
      if (d !== 0) return d
      return new Date(b.created_at).getTime() - new Date(a.created_at).getTime()
    })
  }, [jobs, category])

  const categoryCounts = useMemo(() => {
    const counts = {} as Record<CategoryId, number>
    for (const cat of CATEGORIES) {
      counts[cat.id] = jobs.filter(j => matchesCategory(j.filename, j.status, cat.id)).length
    }
    return counts
  }, [jobs])

  const selectedJobs = jobs.filter(j => selectedIds.has(j.id))
  const primaryJob = selectedJobs[0]
  const selectedActive = selectedJobs.filter(j =>
    j.status === 'downloading' || j.status === 'probing' || j.status === 'queued',
  )
  const selectedPaused = selectedJobs.filter(j => j.status === 'paused' || j.status === 'failed')

  const canStop = selectedActive.length > 0
  const canResume = selectedPaused.length > 0
  const canDelete = selectedJobs.length > 0
  const canStopAll = activeJobs.length > 0

  useEffect(() => {
    setSelectedIds(prev => {
      const next = new Set([...prev].filter(id => jobs.some(j => j.id === id)))
      return next.size === prev.size ? prev : next
    })
  }, [jobs])

  useEffect(() => {
    function onDocClick(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpenMenu(null)
      }
      setContextMenu(null)
    }
    document.addEventListener('click', onDocClick)
    return () => document.removeEventListener('click', onDocClick)
  }, [])

  const handleSelect = useCallback((id: string, e: ReactMouseEvent) => {
    setSelectedIds(prev => {
      const next = new Set(prev)
      if (e.shiftKey && lastClicked.current) {
        const ids = filteredJobs.map(j => j.id)
        const a = ids.indexOf(lastClicked.current)
        const b = ids.indexOf(id)
        if (a >= 0 && b >= 0) {
          const [from, to] = a < b ? [a, b] : [b, a]
          for (let i = from; i <= to; i++) next.add(ids[i])
          return next
        }
      }
      if (e.ctrlKey || e.metaKey) {
        if (next.has(id)) next.delete(id)
        else next.add(id)
        lastClicked.current = id
        return next
      }
      lastClicked.current = id
      return new Set([id])
    })
  }, [filteredJobs])

  async function handleAdd(url: string, segments: number, saveDir?: string, quality?: string, filename?: string, totalBytesHint?: number) {
    const result = await addDownload(url, segments, saveDir, quality, filename, totalBytesHint)
    if (result?.id) {
      setSelectedIds(new Set([result.id]))
      setShowProgressId(result.id)
    }
  }

  function stopSelected() {
    selectedActive.forEach(j => { void pauseDownload(j.id) })
  }

  function resumeSelected() {
    selectedPaused.forEach(j => { void resumeDownload(j.id) })
  }

  function requestDeleteSelected() {
    if (selectedIds.size > 0) {
      setShowDeleteConfirm(true)
    }
  }

  function confirmDelete(deleteFile: boolean) {
    selectedJobs.forEach(j => cancelDownload(j.id, deleteFile))
    setSelectedIds(new Set())
    setShowDeleteConfirm(false)
  }

  function handleStopAll() {
    const ids = jobs
      .filter(j => j.status === 'downloading' || j.status === 'probing' || j.status === 'queued')
      .map(j => j.id)
    void stopAll(ids)
  }

  async function handleProperties() {
    if (!primaryJob) return
    setShowProgressId(primaryJob.id)
  }

  /** `save_path` from the daemon is already the full file path (dir + filename). */
  function jobFilePath(job: { save_path: string; filename: string }): string {
    const p = (job.save_path || '').trim()
    if (p) return p.replace(/\//g, '\\')
    return (job.filename || '').trim()
  }

  async function handleOpenFile() {
    if (!primaryJob) return
    try {
      await invoke('open_file', { path: jobFilePath(primaryJob) })
    } catch (e) {
      console.error(e)
    }
  }

  async function handleOpenFolder() {
    if (!primaryJob) return
    try {
      await invoke('open_folder', { path: jobFilePath(primaryJob) })
    } catch (e) {
      console.error(e)
    }
  }

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const tag = (e.target as HTMLElement)?.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA') return
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'n') {
        e.preventDefault()
        setShowAdd(true)
      }
      if (e.key === 'Delete' && selectedIds.size > 0) {
        e.preventDefault()
        requestDeleteSelected()
      }
      if (e.key === 'Escape') {
        if (showProgressId) {
          setShowProgressId(null)
          return
        }
        setShowAdd(false)
        setShowSettings(false)
        setShowAbout(false)
        setOpenMenu(null)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [selectedIds, selectedJobs])

  useEffect(() => {
    let timer: any
    async function check() {
      try {
        const status = await getUpdateStatus()
        setUpdateStatus(status)
        if (status.state === 'checking' || status.state === 'downloading') {
          timer = setTimeout(check, 2000)
        } else {
          timer = setTimeout(check, 60000)
        }
      } catch {
        timer = setTimeout(check, 60000)
      }
    }
    check()
    return () => clearTimeout(timer)
  }, [getUpdateStatus])

  function saveDefaultSegments(n: number) {
    setDefaultSegments(n)
    localStorage.setItem(DEFAULT_SEGMENTS_KEY, String(n))
  }

  const emptyHint =
    category === 'all'
      ? 'Add a URL from the toolbar, or press Ctrl+N.'
      : `Nothing matches “${CATEGORIES.find(c => c.id === category)?.label}”.`

  return (
    <div className="app">
      {connection !== 'connected' && (
        <div
          className={`daemon-banner ${connection === 'reconnecting' ? 'reconnecting' : 'disconnected'}`}
          role="status"
          aria-live="polite"
        >
          <span className="daemon-banner-dot" />
          {connection === 'connecting'
            ? 'Connecting to download daemon…'
            : 'Daemon disconnected — reconnecting…'}
          <span className="daemon-banner-hint">
            REST + WebSocket will re-sync automatically when the daemon is back.
          </span>
        </div>
      )}

      {connection === 'connected' && updateStatus && updateStatus.state !== 'up_to_date' && (
        <div
          className={`daemon-banner ${updateStatus.state === 'error' ? 'disconnected' : 'reconnecting'}`}
          role="status"
          style={{ backgroundColor: updateStatus.state === 'ready_to_restart' ? '#2e7d32' : undefined }}
        >
          <span className="daemon-banner-dot" />
          {updateStatus.state === 'checking' && 'Checking for updates...'}
          {updateStatus.state === 'downloading' && `Downloading update ${updateStatus.version || ''}...`}
          {updateStatus.state === 'ready_to_restart' && 'Update ready! Please restart the daemon to apply.'}
          {updateStatus.state === 'error' && `Update failed: ${updateStatus.message || 'Unknown error'}`}
          {updateStatus.state === 'error' && (
            <button className="btn btn-ghost btn-sm" style={{ marginLeft: '12px', padding: '2px 8px', fontSize: '11px', height: 'auto', minHeight: '20px' }} onClick={() => void triggerUpdate()}>Retry</button>
          )}
        </div>
      )}

      <nav className="menubar" ref={menuRef} aria-label="Application menu">
        {([
          ['file', 'File', [
            ['Add URL…', () => setShowAdd(true)],
            ['Options…', () => setShowSettings(true)],
          ]],
          ['downloads', 'Downloads', [
            ['Open File', () => { void handleOpenFile() }, !primaryJob || primaryJob.status !== 'completed'],
            ['Open Folder', () => { void handleOpenFolder() }, !primaryJob],
            ['Properties', () => { void handleProperties() }, !primaryJob],
            ['Resume', resumeSelected, !canResume],
            ['Stop', stopSelected, !canStop],
            ['Stop All', handleStopAll, !canStopAll],
            ['Delete', requestDeleteSelected, !canDelete],
          ]],
          ['view', 'View', []],
          ['help', 'Help', [
            ['Check for Updates', () => { void triggerUpdate() }],
            ['About Download Manager Pro', () => setShowAbout(true)],
          ]],
        ] as [MenuId, string, [string, () => void, boolean?][]][]).map(([id, label, items]) => (
          <div key={id} className="menu">
            <button
              className={`menu-btn ${openMenu === id ? 'open' : ''}`}
              onClick={() => setOpenMenu(openMenu === id ? null : id)}
              onMouseEnter={() => { if (openMenu) setOpenMenu(id) }}
            >
              {label}
            </button>
            {openMenu === id && (
              <div className="menu-dropdown" role="menu">
                {items.map(([itemLabel, action, disabled]) => (
                  <button
                    key={itemLabel}
                    role="menuitem"
                    disabled={disabled}
                    onClick={() => {
                      action()
                      setOpenMenu(null)
                    }}
                  >
                    {itemLabel}
                  </button>
                ))}
              </div>
            )}
          </div>
        ))}
      </nav>

      <div className="toolbar" role="toolbar" aria-label="Download actions">
        <ToolbarButton label="Add URL" title="Add URL (Ctrl+N)" onClick={() => setShowAdd(true)} accent>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M12 5v14M5 12h14" /></svg>
        </ToolbarButton>
        <ToolbarButton label="Resume" title="Resume selected" disabled={!canResume} onClick={resumeSelected}>
          <svg viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z" /></svg>
        </ToolbarButton>
        <ToolbarButton label="Stop" title="Stop selected" disabled={!canStop} onClick={stopSelected}>
          <svg viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="1" /></svg>
        </ToolbarButton>
        <ToolbarButton label="Stop All" title="Stop every active download" disabled={!canStopAll} onClick={handleStopAll}>
          <svg viewBox="0 0 24 24" fill="currentColor"><rect x="4" y="4" width="7" height="7" rx="1" /><rect x="13" y="13" width="7" height="7" rx="1" /></svg>
        </ToolbarButton>
        <ToolbarButton label="Delete" title="Delete selected (Del)" disabled={!canDelete} onClick={requestDeleteSelected}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" /></svg>
        </ToolbarButton>
        <ToolbarButton label="Properties" title="Properties (Alt+Enter)" disabled={!primaryJob} onClick={() => { void handleProperties() }}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>
        </ToolbarButton>
        <div className="toolbar-sep" />
        <ToolbarButton label="Options" title="Options" onClick={() => setShowSettings(true)}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H8a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V8c.3.6.9 1 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" /></svg>
        </ToolbarButton>
      </div>

      <div className="workspace">
        <aside className="category-tree" aria-label="Categories">
          <div className="tree-heading">Categories</div>
          {CATEGORIES.map(cat => (
            <button
              key={cat.id}
              className={`tree-item ${category === cat.id ? 'active' : ''}`}
              onClick={() => setCategory(cat.id)}
            >
              <span className={`tree-dot ${cat.id}`} />
              <span className="tree-label">{cat.label}</span>
              <span className="tree-count">{categoryCounts[cat.id]}</span>
            </button>
          ))}
        </aside>

        <section className="list-pane" aria-label="Download list">
          <DownloadList
            jobs={filteredJobs}
            selectedIds={selectedIds}
            onSelect={handleSelect}
            onContextMenu={(id, e) => {
              if (!selectedIds.has(id)) {
                handleSelect(id, { ...e, shiftKey: false, ctrlKey: false, metaKey: false } as any)
              }
              setContextMenu({ x: e.pageX, y: e.pageY })
            }}
            onShowDetails={setShowProgressId}
            onAdd={() => setShowAdd(true)}
            emptyHint={emptyHint}
          />
        </section>
      </div>



      <footer className="statusbar">
        <span className="status-conn">
          <span className={`conn-dot ${connected ? '' : 'disconnected'}`} />
          {connection === 'connected'
            ? 'Connected to daemon'
            : connection === 'connecting'
              ? 'Connecting to daemon…'
              : 'Reconnecting to daemon…'}
        </span>
        <span>Speed: <strong className="mono">{formatSpeed(totalSpeed)}</strong></span>
        <span>Active: <strong>{activeJobs.length}</strong></span>
        <span>Complete: <strong>{completedJobs.length}</strong></span>
        <span>Downloaded: <strong className="mono">{formatBytes(jobs.reduce((a, j) => a + j.downloaded_bytes, 0))}</strong></span>
        <span className="status-end">{filteredJobs.length} shown</span>
      </footer>

      {showDeleteConfirm && (
        <DeleteConfirmationModal
          jobs={selectedJobs}
          onConfirm={confirmDelete}
          onCancel={() => setShowDeleteConfirm(false)}
        />
      )}

      {showAdd && (
        <AddDownloadModal
          defaultSegments={defaultSegments}
          onAdd={handleAdd}
          onClose={() => setShowAdd(false)}
        />
      )}

      {showSettings && (
        <div className="modal-overlay" onClick={e => e.target === e.currentTarget && setShowSettings(false)}>
          <div className="modal" role="dialog" aria-modal="true" aria-labelledby="opt-title">
            <div className="modal-header">
              <h2 className="modal-title" id="opt-title">Options</h2>
              <button className="btn btn-ghost btn-sm btn-icon" onClick={() => setShowSettings(false)} aria-label="Close">✕</button>
            </div>
            <div className="form-group">
              <label className="form-label" htmlFor="opt-segments">
                Default connections
                <span className="range-value">{defaultSegments}</span>
              </label>
              <input
                id="opt-segments"
                className="form-input range"
                type="range"
                min={1}
                max={32}
                value={defaultSegments}
                onChange={e => saveDefaultSegments(Number(e.target.value))}
              />
              <span className="form-hint">Used when you add a new URL. Individual downloads can still override this.</span>
            </div>
            <div className="modal-footer">
              <button className="btn btn-primary" onClick={() => setShowSettings(false)}>OK</button>
            </div>
          </div>
        </div>
      )}

      {showAbout && (
        <div className="modal-overlay" onClick={e => e.target === e.currentTarget && setShowAbout(false)}>
          <div className="modal modal-sm" role="dialog" aria-modal="true" aria-labelledby="about-title">
            <div className="modal-header">
              <h2 className="modal-title" id="about-title">Download Manager Pro</h2>
              <button className="btn btn-ghost btn-sm btn-icon" onClick={() => setShowAbout(false)} aria-label="Close">✕</button>
            </div>
            <p className="about-copy">
              A fast and reliable download manager.
              Double-click a download (or use Properties) for detailed progress.
            </p>
            <div className="about-info" style={{ marginTop: '1rem', fontSize: '0.85em', color: 'var(--text-muted)' }}>
              <p>Version: {packageJson.version}</p>
              <p>Publisher: Shoaib Hassan</p>
            </div>
            
            <div style={{ marginTop: '1rem', padding: '0.5rem', backgroundColor: 'var(--bg-surface)', borderRadius: '4px' }}>
              <p style={{ fontSize: '0.85em', marginBottom: '0.5rem' }}>{updateStatus?.message || "Keep your app up to date."}</p>
              
              {updateStatus?.state === 'available' ? (
                <button className="btn btn-primary btn-sm" onClick={installUpdate}>Download & Install</button>
              ) : (
                <button 
                  className="btn btn-secondary btn-sm" 
                  onClick={checkForUpdates}
                  disabled={updateStatus?.state === 'checking' || updateStatus?.state === 'downloading'}
                >
                  {updateStatus?.state === 'checking' ? 'Checking...' : 'Check for Updates'}
                </button>
              )}
            </div>

            <div className="modal-footer">
              <button className="btn btn-primary" onClick={() => setShowAbout(false)}>OK</button>
            </div>
          </div>
        </div>
      )}

      {showProgressId && (
        <div className="modal-overlay" onClick={e => e.target === e.currentTarget && setShowProgressId(null)}>
          <div className="modal modal-progress" role="dialog" aria-modal="true">
            <DownloadProgressDialog
              job={jobs.find(j => j.id === showProgressId)}
              jobId={showProgressId}
              onClose={() => setShowProgressId(null)}
              onPause={(id) => { void pauseDownload(id) }}
              onResume={(id) => { void resumeDownload(id) }}
            />
          </div>
        </div>
      )}

      {contextMenu && (
        <div
          className="context-menu menu-dropdown"
          style={{
            position: 'fixed',
            top: contextMenu.y,
            left: contextMenu.x,
            zIndex: 1000,
            display: 'flex',
            flexDirection: 'column',
          }}
          onClick={(e) => e.stopPropagation()}
        >
          <button className="menu-item" onClick={() => { setContextMenu(null); void handleOpenFile(); }} disabled={!primaryJob || primaryJob.status !== 'completed'}>Open File</button>
          <button className="menu-item" onClick={() => { setContextMenu(null); void handleOpenFolder(); }} disabled={!primaryJob}>Open Folder</button>
          <button className="menu-item" onClick={() => { setContextMenu(null); void handleProperties(); }} disabled={!primaryJob}>Properties</button>
          <button className="menu-item" onClick={() => { setContextMenu(null); resumeSelected(); }} disabled={!canResume}>Resume</button>
          <button className="menu-item" onClick={() => { setContextMenu(null); stopSelected(); }} disabled={!canStop}>Stop</button>
          <button className="menu-item" onClick={() => { setContextMenu(null); requestDeleteSelected(); }} disabled={!canDelete}>Delete</button>
        </div>
      )}
    </div>
  )
}

function ToolbarButton({
  label,
  title,
  onClick,
  disabled,
  accent,
  children,
}: {
  label: string
  title?: string
  onClick?: () => void
  disabled?: boolean
  accent?: boolean
  children: ReactNode
}) {
  return (
    <button
      type="button"
      className={`toolbar-btn ${accent ? 'accent' : ''}`}
      title={title ?? label}
      disabled={disabled}
      onClick={onClick}
    >
      <span className="toolbar-icon">{children}</span>
      <span>{label}</span>
    </button>
  )
}
