import { memo, useCallback, useMemo, useRef, useState, useEffect } from 'react'
import { List, type RowComponentProps } from 'react-window'
import type { DownloadJob, DownloadStatus } from '../hooks/useDownloads'
import { formatBytes, formatDate, formatEta, formatSpeed, getFileKind } from '../lib/format'

export const STATUS_LABELS: Record<DownloadStatus, string> = {
  queued: 'Queued',
  probing: 'Connecting…',
  downloading: 'Downloading',
  paused: 'Paused',
  completed: 'Complete',
  failed: 'Failed',
  cancelled: 'Stopped',
}

const ROW_HEIGHT = 42
/** Virtualize once the list is large enough that DOM cost matters. */
const VIRTUALIZE_THRESHOLD = 40

interface Props {
  jobs: DownloadJob[]
  selectedIds: Set<string>
  onSelect: (id: string, e: React.MouseEvent) => void
  onContextMenu: (id: string, e: React.MouseEvent) => void
  onShowDetails: (id: string) => void
  onAdd: () => void
  emptyHint: string
}

type RowData = {
  jobs: DownloadJob[]
  selectedIds: Set<string>
  onSelect: (id: string, e: React.MouseEvent) => void
  onContextMenu: (id: string, e: React.MouseEvent) => void
  onShowDetails: (id: string) => void
}

export function DownloadList({ jobs, selectedIds, onSelect, onContextMenu, onShowDetails, onAdd, emptyHint }: Props) {
  if (jobs.length === 0) {
    return (
      <div className="empty-state">
        <div className="empty-icon" aria-hidden>
          <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.4">
            <path d="M12 3v12m0 0 4-4m-4 4-4-4M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" />
          </svg>
        </div>
        <h3>No downloads in this category</h3>
        <p>{emptyHint}</p>
        <button className="btn btn-primary" onClick={onAdd}>Add URL</button>
      </div>
    )
  }

  const useVirtual = jobs.length >= VIRTUALIZE_THRESHOLD

  return (
    <div className="table-wrap">
      <div className="dl-table-header" role="row">
        <span className="col-name">Name</span>
        <span className="col-size">Size</span>
        <span className="col-status">Status</span>
        <span className="col-eta">Time Left</span>
        <span className="col-speed">Transfer Rate</span>
        <span className="col-date">Added</span>
        <span className="col-path">Save To</span>
      </div>
      {useVirtual ? (
        <VirtualDownloadRows jobs={jobs} selectedIds={selectedIds} onSelect={onSelect} onContextMenu={onContextMenu} onShowDetails={onShowDetails} />
      ) : (
        <div className="dl-table-body" role="rowgroup">
          {jobs.map(job => (
            <DownloadRow
              key={job.id}
              job={job}
              selected={selectedIds.has(job.id)}
              onSelect={onSelect}
              onContextMenu={onContextMenu}
              onShowDetails={onShowDetails}
            />
          ))}
        </div>
      )}
    </div>
  )
}

function VirtualDownloadRows({
  jobs,
  selectedIds,
  onSelect,
  onContextMenu,
  onShowDetails,
}: {
  jobs: DownloadJob[]
  selectedIds: Set<string>
  onSelect: (id: string, e: React.MouseEvent) => void
  onContextMenu: (id: string, e: React.MouseEvent) => void
  onShowDetails: (id: string) => void
}) {
  const wrapRef = useRef<HTMLDivElement>(null)
  const [height, setHeight] = useState(400)

  useEffect(() => {
    const el = wrapRef.current
    if (!el) return
    const ro = new ResizeObserver(entries => {
      const h = entries[0]?.contentRect.height
      if (h && h > 0) setHeight(h)
    })
    ro.observe(el)
    setHeight(el.clientHeight || 400)
    return () => ro.disconnect()
  }, [])

  const rowProps: RowData = useMemo(
    () => ({ jobs, selectedIds, onSelect, onContextMenu, onShowDetails }),
    [jobs, selectedIds, onSelect, onContextMenu, onShowDetails],
  )

  const rowKey = useCallback(
    (index: number, data: RowData) => data.jobs[index]?.id ?? index,
    [],
  )

  return (
    <div className="dl-virtual-host" ref={wrapRef}>
      <List
        rowComponent={VirtualRow}
        rowCount={jobs.length}
        rowHeight={ROW_HEIGHT}
        rowProps={rowProps}
        rowKey={rowKey}
        overscanCount={8}
        style={{ height, width: '100%' }}
        className="dl-virtual-list"
      />
    </div>
  )
}

function VirtualRow({
  index,
  style,
  jobs,
  selectedIds,
  onSelect,
  onContextMenu,
  onShowDetails,
}: RowComponentProps<RowData>) {
  const job = jobs[index]
  if (!job) return null
  return (
    <div style={style}>
      <DownloadRow
        job={job}
        selected={selectedIds.has(job.id)}
        onSelect={onSelect}
        onContextMenu={onContextMenu}
        onShowDetails={onShowDetails}
      />
    </div>
  )
}

const DownloadRow = memo(function DownloadRow({
  job,
  selected,
  onSelect,
  onContextMenu,
  onShowDetails,
}: {
  job: DownloadJob
  selected: boolean
  onSelect: (id: string, e: React.MouseEvent) => void
  onContextMenu: (id: string, e: React.MouseEvent) => void
  onShowDetails: (id: string) => void
}) {
  const pct = job.total_bytes > 0 ? (job.downloaded_bytes / job.total_bytes) * 100 : 0
  const kind = getFileKind(job.filename)
  const isActive = job.status === 'downloading' || job.status === 'probing'

  async function handleDoubleClick() {
    onShowDetails(job.id)
  }

  return (
    <div
      className={`dl-row ${selected ? 'selected' : ''} ${job.status}`}
      role="row"
      onClick={e => onSelect(job.id, e)}
      onDoubleClick={handleDoubleClick}
      onContextMenu={e => {
        e.preventDefault()
        onContextMenu(job.id, e)
      }}
    >
      <span className="col-name">
        <span className={`file-icon kind-${kind}`} aria-hidden />
        <span className="file-name" title={job.filename}>{job.filename}</span>
      </span>
      <span className="col-size mono">
        {job.total_bytes > 0 
          ? `${formatBytes(job.downloaded_bytes)} / ${formatBytes(job.total_bytes)}` 
          : formatBytes(job.downloaded_bytes)}
      </span>
      <span className="col-status">
        <div className="status-cell">
          {(isActive || job.status === 'paused') && (
            <div className="mini-track" title={`${pct.toFixed(1)}%`}>
              <div
                className={`mini-fill ${job.status}`}
                style={{ width: `${Math.min(pct, 100)}%` }}
              />
            </div>
          )}
          <span
            className={`status-text ${job.status}`}
            title={job.status === 'failed' && job.error ? job.error : undefined}
          >
            {job.status === 'downloading' 
              ? (job.phase === 'merging' ? 'Merging...' : (pct >= 100 ? 'Finishing...' : `${pct.toFixed(1)}%`))
              : STATUS_LABELS[job.status]}
          </span>
        </div>
      </span>
      <span className="col-eta mono">{isActive ? formatEta(job.eta_seconds) : '—'}</span>
      <span className="col-speed mono">{isActive ? formatSpeed(job.speed_bps) : '—'}</span>
      <span className="col-date">{formatDate(job.created_at)}</span>
      <span className="col-path" title={job.save_path}>{job.save_path}</span>
    </div>
  )
})
