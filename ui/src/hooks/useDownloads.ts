import { useState, useEffect, useRef, useCallback } from 'react'

export type DownloadStatus =
  | 'queued' | 'probing' | 'downloading' | 'paused'
  | 'completed' | 'failed' | 'cancelled'

export type ConnectionState = 'connecting' | 'connected' | 'reconnecting' | 'disconnected'

export interface Segment {
  id: number
  start: number
  end: number
  downloaded: number
  speed_bps: number
}

export interface DownloadJob {
  id: string
  url: string
  filename: string
  save_path: string
  total_bytes: number
  downloaded_bytes: number
  segments: Segment[]
  status: DownloadStatus
  phase?: string | null
  speed_bps: number
  eta_seconds: number | null
  created_at: string
  completed_at: string | null
  supports_ranges: boolean
  segment_count: number
  use_http3: boolean
  engine?: string
  quality?: string | null
}

type DmEvent =
  | { seq?: number; type: 'snapshot'; jobs: DownloadJob[] }
  | { seq?: number; type: 'job_added'; job: DownloadJob }
  | { seq?: number; type: 'job_updated'; job: DownloadJob }
  | { seq?: number; type: 'job_removed'; id: string }
  | { seq?: number; type: 'segment_progress'; job_id: string; segment_id: number; downloaded: number; speed_bps: number }
  | { seq?: number; type: 'error'; id: string | null; message: string }

function apiErrorMessage(err: unknown, fallback: string): string {
  if (!err || typeof err !== 'object') return fallback
  const e = err as { error?: unknown }
  if (typeof e.error === 'string') return e.error
  if (e.error && typeof e.error === 'object') {
    const body = e.error as { message?: unknown; code?: unknown }
    if (typeof body.message === 'string') return body.message
  }
  return fallback
}

const DAEMON_URL = 'http://127.0.0.1:7373'
const WS_URL = 'ws://127.0.0.1:7373/ws'
const RECONNECT_MS = 3000

/** Pure reducer so buffered WS events can be replayed in seq order. */
export function applyJobEvent(
  prev: Map<string, DownloadJob>,
  event: DmEvent,
): Map<string, DownloadJob> {
  const next = new Map(prev)
  switch (event.type) {
    case 'snapshot':
      return new Map(event.jobs.map(j => [j.id, j]))
    case 'job_added':
      next.set(event.job.id, event.job)
      break
    case 'job_updated': {
      const existing = next.get(event.job.id)
      const job = { ...event.job }
      if (
        existing &&
        (job.status === 'downloading' || job.status === 'probing')
      ) {
        // Keep last good speed when a tick reports 0 / Unknown.
        if (job.status === 'downloading' && (!job.speed_bps || job.speed_bps <= 0) && existing.speed_bps > 0) {
          job.speed_bps = existing.speed_bps
          if ((!job.eta_seconds || job.eta_seconds <= 0) && job.total_bytes > job.downloaded_bytes) {
            job.eta_seconds = (job.total_bytes - job.downloaded_bytes) / existing.speed_bps
          }
        }
        // Never let a stale update rewind bytes during an active download.
        if (existing.downloaded_bytes > job.downloaded_bytes) {
          job.downloaded_bytes = existing.downloaded_bytes
        }
      }
      next.set(job.id, job)
      break
    }
    case 'job_removed':
      next.delete(event.id)
      break
    case 'segment_progress': {
      const job = next.get(event.job_id)
      if (job) {
        const known = job.segments.some(s => s.id === event.segment_id)
        const speedIn = event.speed_bps
        const segs = known
          ? job.segments.map(s =>
              s.id === event.segment_id
                ? { ...s, downloaded: event.downloaded, speed_bps: speedIn }
                : s,
            )
          : [...job.segments, {
              id: event.segment_id,
              start: 0,
              end: Math.max(event.downloaded, job.total_bytes || event.downloaded),
              downloaded: event.downloaded,
              speed_bps: speedIn,
            }]
        const downloaded_bytes =
          job.engine === 'youtube'
            ? Math.max(event.downloaded, job.downloaded_bytes)
            : segs.reduce((a, s) => a + s.downloaded, 0)
        const speed_bps =
          job.engine === 'youtube'
            ? speedIn
            : segs.reduce((a, s) => a + s.speed_bps, 0)
        const remaining = Math.max(0, (job.total_bytes || 0) - downloaded_bytes)
        const eta_seconds = speed_bps > 0 ? remaining / speed_bps : job.eta_seconds
        next.set(event.job_id, { ...job, segments: segs, downloaded_bytes, speed_bps, eta_seconds })
      }
      break
    }
    case 'error': {
      if (event.id) {
        const job = next.get(event.id)
        if (job) next.set(event.id, { ...job, status: 'failed' })
      }
      break
    }
  }
  return next
}

function sortBySeq(events: DmEvent[]): DmEvent[] {
  return [...events].sort((a, b) => (a.seq ?? 0) - (b.seq ?? 0))
}

export function useDownloads() {
  const [jobs, setJobs] = useState<Map<string, DownloadJob>>(new Map())
  const [connection, setConnection] = useState<ConnectionState>('connecting')
  const wsRef = useRef<WebSocket | null>(null)
  const reconnectTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const connectRef = useRef<() => void>(() => {})
  const isMountedRef = useRef(false)
  const lastSeqRef = useRef<number | null>(null)
  /** WS messages held until the REST snapshot resolves (merge-race fix). */
  const wsBufferRef = useRef<DmEvent[]>([])
  const restReadyRef = useRef(false)
  const fetchGenRef = useRef(0)
  const everConnectedRef = useRef(false)

  const flushBuffer = useCallback((baseline: Map<string, DownloadJob>) => {
    const buffered = sortBySeq(wsBufferRef.current)
    wsBufferRef.current = []
    let state = baseline
    let lastSeq = lastSeqRef.current
    for (const event of buffered) {
      if (typeof event.seq === 'number') {
        if (lastSeq !== null && event.type !== 'snapshot' && event.seq <= lastSeq) {
          continue
        }
        if (lastSeq !== null && event.type !== 'snapshot' && event.seq > lastSeq + 1) {
          // Gap inside the buffer — skip to next snapshot if any, else keep applying.
          // Server will resync on next live gap; for buffered data apply in order after gap.
        }
        lastSeq = event.seq
      }
      state = applyJobEvent(state, event)
    }
    lastSeqRef.current = lastSeq
    return state
  }, [])

  const fetchRestSnapshot = useCallback(async (ws: WebSocket) => {
    const gen = ++fetchGenRef.current
    restReadyRef.current = false
    wsBufferRef.current = []
    lastSeqRef.current = null

    try {
      const resp = await fetch(`${DAEMON_URL}/downloads`)
      if (gen !== fetchGenRef.current || wsRef.current !== ws) return
      if (!resp.ok) throw new Error(`REST snapshot failed (${resp.status})`)
      const list = (await resp.json()) as DownloadJob[]
      if (gen !== fetchGenRef.current || wsRef.current !== ws) return

      const baseline = new Map(list.map(j => [j.id, j]))
      restReadyRef.current = true
      const merged = flushBuffer(baseline)
      setJobs(merged)
    } catch {
      if (gen !== fetchGenRef.current || wsRef.current !== ws) return
      // REST failed (daemon restarting) — still release the buffer if WS sent a snapshot.
      restReadyRef.current = true
      const hasSnap = wsBufferRef.current.some(e => e.type === 'snapshot')
      if (hasSnap) {
        setJobs(flushBuffer(new Map()))
      }
    }
  }, [flushBuffer])

  const handleLiveEvent = useCallback((ws: WebSocket, event: DmEvent) => {
    if (typeof event.seq === 'number') {
      const last = lastSeqRef.current
      if (event.type !== 'snapshot' && last !== null && event.seq > last + 1) {
        ws.send(JSON.stringify({ type: 'resync' }))
        return
      }
      if (event.type !== 'snapshot' && last !== null && event.seq <= last) {
        return
      }
      lastSeqRef.current = event.seq
    }
    setJobs(prev => applyJobEvent(prev, event))
  }, [])

  const connect = useCallback(() => {
    const state = wsRef.current?.readyState
    if (state === WebSocket.OPEN || state === WebSocket.CONNECTING) return

    setConnection(everConnectedRef.current ? 'reconnecting' : 'connecting')

    const ws = new WebSocket(WS_URL)
    wsRef.current = ws

    ws.onopen = () => {
      if (!isMountedRef.current || wsRef.current !== ws) return
      everConnectedRef.current = true
      setConnection('connected')
      clearTimeout(reconnectTimer.current)
      // Re-snapshot via REST; buffer WS until it lands (daemon restart safe).
      void fetchRestSnapshot(ws)
    }

    ws.onmessage = (ev) => {
      if (wsRef.current !== ws) return
      try {
        const event: DmEvent = JSON.parse(ev.data)
        if (!restReadyRef.current) {
          wsBufferRef.current.push(event)
          return
        }
        handleLiveEvent(ws, event)
      } catch {
        /* ignore malformed */
      }
    }

    ws.onclose = () => {
      if (wsRef.current === ws) wsRef.current = null
      restReadyRef.current = false
      wsBufferRef.current = []
      if (!isMountedRef.current) return
      setConnection('reconnecting')
      reconnectTimer.current = setTimeout(() => connectRef.current(), RECONNECT_MS)
    }

    ws.onerror = () => ws.close()
  }, [fetchRestSnapshot, handleLiveEvent])

  connectRef.current = connect

  useEffect(() => {
    isMountedRef.current = true
    connect()
    return () => {
      isMountedRef.current = false
      clearTimeout(reconnectTimer.current)
      fetchGenRef.current += 1
      wsRef.current?.close()
    }
  }, [connect])

  async function addDownload(url: string, segments?: number, saveDir?: string, quality?: string, filename?: string, totalBytesHint?: number) {
    const resp = await fetch(`${DAEMON_URL}/downloads`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ url, segments, save_dir: saveDir, quality, filename, total_bytes: totalBytesHint }),
    })
    if (!resp.ok) {
      const err = await resp.json().catch(() => null)
      throw new Error(apiErrorMessage(err, 'Failed to add download'))
    }
    return await resp.json() as { id: string }
  }

  async function cancelDownload(id: string, deleteFile = false) {
    let snapshot: DownloadJob | undefined
    setJobs(prev => {
      const next = new Map(prev)
      snapshot = next.get(id)
      next.delete(id)
      return next
    })
    try {
      const url = `${DAEMON_URL}/downloads/${id}${deleteFile ? '?delete_file=true' : ''}`
      const resp = await fetch(url, { method: 'DELETE' })
      if (!resp.ok && resp.status !== 204) {
        throw new Error('Failed to delete download')
      }
    } catch (e) {
      if (snapshot) {
        setJobs(prev => {
          const next = new Map(prev)
          next.set(id, snapshot!)
          return next
        })
      }
      throw e
    }
  }

  async function pauseDownload(id: string) {
    let snapshot: DownloadJob | undefined
    setJobs(prev => {
      const next = new Map(prev)
      const j = next.get(id)
      if (j) {
        snapshot = j
        next.set(id, { ...j, status: 'paused', speed_bps: 0, eta_seconds: null })
      }
      return next
    })
    try {
      const resp = await fetch(`${DAEMON_URL}/downloads/${id}/pause`, { method: 'POST' })
      if (!resp.ok && resp.status !== 204) {
        throw new Error('Failed to stop download')
      }
    } catch (e) {
      if (snapshot) {
        setJobs(prev => new Map(prev).set(id, snapshot!))
      }
      throw e
    }
  }

  async function resumeDownload(id: string) {
    let snapshot: DownloadJob | undefined
    setJobs(prev => {
      const next = new Map(prev)
      const j = next.get(id)
      if (j) {
        snapshot = j
        next.set(id, { ...j, status: 'queued', speed_bps: 0 })
      }
      return next
    })
    try {
      const resp = await fetch(`${DAEMON_URL}/downloads/${id}/resume`, { method: 'POST' })
      if (!resp.ok && resp.status !== 204) {
        const err = await resp.json().catch(() => null)
        throw new Error(apiErrorMessage(err, 'Failed to resume download'))
      }
    } catch (e) {
      if (snapshot) {
        setJobs(prev => new Map(prev).set(id, snapshot!))
      }
      throw e
    }
  }

  async function stopAll(ids: string[]) {
    await Promise.all(ids.map(id => pauseDownload(id).catch(() => undefined)))
  }

  async function triggerUpdate() {
    await fetch(`${DAEMON_URL}/update`, { method: 'POST' })
  }

  async function getUpdateStatus(): Promise<{ state: string; version?: string; message?: string }> {
    const resp = await fetch(`${DAEMON_URL}/update/status`)
    if (!resp.ok) throw new Error('Failed to fetch update status')
    return await resp.json()
  }

  return {
    jobs: Array.from(jobs.values()),
    connected: connection === 'connected',
    connection,
    addDownload,
    cancelDownload,
    pauseDownload,
    resumeDownload,
    stopAll,
    triggerUpdate,
    getUpdateStatus,
  }
}
