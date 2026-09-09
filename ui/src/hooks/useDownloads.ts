import { useState, useEffect, useRef, useCallback } from 'react'

export type DownloadStatus =
  | 'queued' | 'probing' | 'downloading' | 'paused'
  | 'completed' | 'failed' | 'cancelled'

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
  speed_bps: number
  eta_seconds: number | null
  created_at: string
  completed_at: string | null
  supports_ranges: boolean
  segment_count: number
  use_http3: boolean
}

type DmEvent =
  | { type: 'snapshot'; jobs: DownloadJob[] }
  | { type: 'job_added'; job: DownloadJob }
  | { type: 'job_updated'; job: DownloadJob }
  | { type: 'job_removed'; id: string }
  | { type: 'segment_progress'; job_id: string; segment_id: number; downloaded: number; speed_bps: number }
  | { type: 'error'; id: string | null; message: string }

const DAEMON_URL = 'http://127.0.0.1:7373'
const WS_URL     = 'ws://127.0.0.1:7373/ws'

export function useDownloads() {
  const [jobs, setJobs] = useState<Map<string, DownloadJob>>(new Map())
  const [connected, setConnected] = useState(false)
  const wsRef = useRef<WebSocket | null>(null)
  const reconnectTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  // Ref to always hold the latest connect fn, avoiding stale closure in ws.onclose
  const connectRef = useRef<() => void>(() => {})
  // Guards against scheduling reconnects after the component unmounts (StrictMode safe)
  const isMountedRef = useRef(false)

  const connect = useCallback(() => {
    // Already open or mid-handshake — do nothing
    const state = wsRef.current?.readyState
    if (state === WebSocket.OPEN || state === WebSocket.CONNECTING) return

    const ws = new WebSocket(WS_URL)
    wsRef.current = ws

    ws.onopen = () => {
      setConnected(true)
      clearTimeout(reconnectTimer.current)
    }

    ws.onmessage = (ev) => {
      try {
        const event: DmEvent = JSON.parse(ev.data)
        handleEvent(event)
      } catch {}
    }

    ws.onclose = () => {
      setConnected(false)
      // Only schedule reconnect if the component is still mounted
      if (isMountedRef.current) {
        reconnectTimer.current = setTimeout(() => connectRef.current(), 3000)
      }
    }

    ws.onerror = () => ws.close()
  }, [])

  // Keep ref in sync with the latest connect fn
  connectRef.current = connect

  function handleEvent(event: DmEvent) {
    setJobs(prev => {
      const next = new Map(prev)
      switch (event.type) {
        case 'snapshot':
          event.jobs.forEach(j => next.set(j.id, j))
          break
        case 'job_added':
          next.set(event.job.id, event.job)
          break
        case 'job_updated':
          next.set(event.job.id, event.job)
          break
        case 'job_removed':
          next.delete(event.id)
          break
        case 'segment_progress': {
          const job = next.get(event.job_id)
          if (job) {
            const segs = job.segments.map(s =>
              s.id === event.segment_id
                ? { ...s, downloaded: event.downloaded, speed_bps: event.speed_bps }
                : s
            )
            const downloaded_bytes = segs.reduce((a, s) => a + s.downloaded, 0)
            const speed_bps = segs.reduce((a, s) => a + s.speed_bps, 0)
            const remaining = job.total_bytes - downloaded_bytes
            const eta_seconds = speed_bps > 0 ? remaining / speed_bps : null
            next.set(event.job_id, { ...job, segments: segs, downloaded_bytes, speed_bps, eta_seconds })
          }
          break
        }
        case 'error': {
          if (event.id) {
            const job = next.get(event.id)
            if (job) {
              next.set(event.id, { ...job, status: 'failed' })
            }
          }
          break
        }
      }
      return next
    })
  }

  useEffect(() => {
    isMountedRef.current = true
    connect()
    return () => {
      isMountedRef.current = false  // block any pending onclose from rescheduling
      clearTimeout(reconnectTimer.current)
      wsRef.current?.close()
    }
  }, [connect])

  async function addDownload(url: string, segments?: number, saveDir?: string) {
    const resp = await fetch(`${DAEMON_URL}/downloads`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ url, segments, save_dir: saveDir }),
    })
    if (!resp.ok) {
      const err = await resp.json()
      throw new Error(err.error ?? 'Failed to add download')
    }
    return await resp.json() as { id: string }
  }

  async function cancelDownload(id: string) {
    await fetch(`${DAEMON_URL}/downloads/${id}`, { method: 'DELETE' })
  }

  return {
    jobs: Array.from(jobs.values()),
    connected,
    addDownload,
    cancelDownload,
  }
}
