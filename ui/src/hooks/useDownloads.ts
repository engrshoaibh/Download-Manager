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
  /** Why the job failed, when the daemon knows. */
  error?: string | null
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

/** Client-only fields used to derive speed from how fast Done grows. */
type SpeedSample = {
  __speedAt?: number
  __speedBytes?: number
  __speedEma?: number
}

type JobState = DownloadJob & SpeedSample

/**
 * Speed must match Done. Checkpoints wipe per-segment speeds to 0, so summing
 * them often shows one connection (~50 KB/s) while all 16 are writing.
 * Measure from downloaded_bytes over wall time instead.
 */
function withMeasuredSpeed(prev: JobState | undefined, job: JobState, now = performance.now()): JobState {
  if (job.status !== 'downloading') {
    return {
      ...job,
      speed_bps: job.status === 'probing' ? job.speed_bps : 0,
      __speedAt: now,
      __speedBytes: job.downloaded_bytes,
      __speedEma: 0,
    }
  }

  const prevAt = prev?.__speedAt
  const prevBytes = prev?.__speedBytes ?? prev?.downloaded_bytes ?? job.downloaded_bytes
  let ema = prev?.__speedEma ?? 0

  if (prevAt != null) {
    const dt = (now - prevAt) / 1000
    if (dt >= 0.2) {
      const delta = Math.max(0, job.downloaded_bytes - prevBytes)
      const instant = delta / dt
      ema = ema > 0 ? ema * 0.55 + instant * 0.45 : instant
      const speed_bps = ema > 1 ? ema : job.speed_bps
      const eta_seconds =
        speed_bps > 1 && job.total_bytes > job.downloaded_bytes
          ? (job.total_bytes - job.downloaded_bytes) / speed_bps
          : job.eta_seconds
      return {
        ...job,
        speed_bps,
        eta_seconds,
        __speedAt: now,
        __speedBytes: job.downloaded_bytes,
        __speedEma: ema,
      }
    }
    /* Same animation frame / burst — keep the EMA, don't reset the byte anchor. */
    const speed_bps = ema > 1 ? ema : job.speed_bps || prev?.speed_bps || 0
    const eta_seconds =
      speed_bps > 1 && job.total_bytes > job.downloaded_bytes
        ? (job.total_bytes - job.downloaded_bytes) / speed_bps
        : job.eta_seconds
    return {
      ...job,
      speed_bps,
      eta_seconds,
      __speedAt: prevAt,
      __speedBytes: prevBytes,
      __speedEma: ema,
    }
  }

  return {
    ...job,
    __speedAt: now,
    __speedBytes: job.downloaded_bytes,
    __speedEma: job.speed_bps > 0 ? job.speed_bps : 0,
  }
}

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
      const existing = next.get(event.job.id) as JobState | undefined
      const job = { ...event.job } as JobState
      if (
        existing &&
        (job.status === 'downloading' || job.status === 'probing')
      ) {
        // Never let a stale update rewind bytes during an active download.
        if (existing.downloaded_bytes > job.downloaded_bytes) {
          job.downloaded_bytes = existing.downloaded_bytes
        }
        // Keep segment speeds the UI already measured when the daemon snapshot
        // sends zeros (scheduler.snapshot always writes speed_bps: 0).
        if (job.segments?.length && existing.segments?.length) {
          const byId = new Map(existing.segments.map(s => [s.id, s]))
          job.segments = job.segments.map(s => {
            const prevSeg = byId.get(s.id)
            if (prevSeg && (!s.speed_bps || s.speed_bps <= 0) && prevSeg.speed_bps > 0) {
              return { ...s, speed_bps: prevSeg.speed_bps }
            }
            return s
          })
        }
      }
      next.set(job.id, withMeasuredSpeed(existing, job))
      break
    }
    case 'job_removed':
      next.delete(event.id)
      break
    case 'segment_progress': {
      const job = next.get(event.job_id) as JobState | undefined
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
        next.set(
          event.job_id,
          withMeasuredSpeed(job, { ...job, segments: segs, downloaded_bytes }),
        )
      }
      break
    }
    case 'error': {
      if (event.id) {
        const job = next.get(event.id)
        if (job) next.set(event.id, { ...job, status: 'failed', speed_bps: 0 })
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

  const pendingEventsRef = useRef<DmEvent[]>([])
  const flushFrameRef = useRef(0)
  /** Timer used instead of a paint callback while the window is hidden. */
  const flushTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)

  const flushPendingEvents = useCallback(() => {
    flushFrameRef.current = 0
    clearTimeout(flushTimerRef.current)
    flushTimerRef.current = undefined
    const batch = pendingEventsRef.current
    if (batch.length === 0) return
    pendingEventsRef.current = []
    setJobs(prev => batch.reduce((state, event) => applyJobEvent(state, event), prev))
  }, [])

  const handleLiveEvent = useCallback((ws: WebSocket, event: DmEvent) => {
    if (typeof event.seq === 'number') {
      const last = lastSeqRef.current
      if (event.type !== 'snapshot' && last !== null && event.seq > last + 1) {
        // Missed events. Reload the list over HTTP: a snapshot sent over the socket
        // can itself be dropped when this window is behind (hidden in the tray).
        void fetchRestSnapshot(ws)
        return
      }
      if (event.type !== 'snapshot' && last !== null && event.seq <= last) {
        return
      }
      lastSeqRef.current = event.seq
    }
    /* Paint at most once per frame so fragment ticks don't re-render the list. */
    const immediate = event.type === 'job_added' || event.type === 'job_removed' || event.type === 'error'
    if (immediate) {
      if (flushFrameRef.current) {
        cancelAnimationFrame(flushFrameRef.current)
        flushFrameRef.current = 0
      }
      clearTimeout(flushTimerRef.current)
      flushTimerRef.current = undefined
      const batch = pendingEventsRef.current
      pendingEventsRef.current = []
      setJobs(prev => [...batch, event].reduce((state, ev) => applyJobEvent(state, ev), prev))
      return
    }
    pendingEventsRef.current.push(event)
    // Paint callbacks never fire while the window is hidden — use a timer then,
    // so the list keeps moving and does not pile up thousands of updates.
    if (document.hidden) {
      if (!flushTimerRef.current) {
        flushTimerRef.current = setTimeout(flushPendingEvents, 500)
      }
    } else if (!flushFrameRef.current) {
      flushFrameRef.current = requestAnimationFrame(flushPendingEvents)
    }
  }, [flushPendingEvents, fetchRestSnapshot])

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
      if (flushFrameRef.current) cancelAnimationFrame(flushFrameRef.current)
      clearTimeout(flushTimerRef.current)
      fetchGenRef.current += 1
      wsRef.current?.close()
    }
  }, [connect])

  // Opening the window from the tray (or focusing it) reloads the list, so it is
  // right even if live updates were missed while it was hidden.
  const lastRefreshRef = useRef(0)
  useEffect(() => {
    const refresh = () => {
      if (document.hidden) return
      const now = Date.now()
      if (now - lastRefreshRef.current < 2000) return
      lastRefreshRef.current = now
      flushPendingEvents()
      const ws = wsRef.current
      if (ws && ws.readyState === WebSocket.OPEN) void fetchRestSnapshot(ws)
      else connectRef.current()
    }
    document.addEventListener('visibilitychange', refresh)
    window.addEventListener('focus', refresh)
    return () => {
      document.removeEventListener('visibilitychange', refresh)
      window.removeEventListener('focus', refresh)
    }
  }, [fetchRestSnapshot, flushPendingEvents])

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
