// Connects to /ws/events and maintains a bounded in-memory event log.
// Auto-reconnects every 3 seconds on close/error.

import { useEffect, useRef, useState } from 'react'
import { apiFetch } from '../lib/api'

export interface PanelEvent {
  type: string
  ts?: string
  [key: string]: unknown
}

export function useWebSocket(maxEvents = 50) {
  const [events, setEvents] = useState<PanelEvent[]>([])
  const [connected, setConnected] = useState(false)
  const wsRef = useRef<WebSocket | null>(null)
  const reconnectTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  // Track mount state to prevent reconnect after unmount
  const mounted = useRef(true)

  useEffect(() => {
    mounted.current = true

    async function connect() {
      if (!mounted.current) return
      try {
        const { ticket } = await apiFetch<{ ticket: string }>('/api/auth/ws-ticket', {
          method: 'POST',
        })
        if (!mounted.current) return

        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
        const url = `${protocol}//${window.location.host}/ws/events?ticket=${encodeURIComponent(ticket)}`
        const ws = new WebSocket(url)
        wsRef.current = ws

        ws.onopen = () => {
          if (mounted.current) setConnected(true)
        }

        ws.onclose = () => {
          if (!mounted.current) return
          setConnected(false)
          reconnectTimer.current = setTimeout(() => void connect(), 3_000)
        }

        ws.onerror = () => {
          ws.close()
        }

        ws.onmessage = (msg) => {
          if (!mounted.current) return
          try {
            const event = JSON.parse(msg.data as string) as PanelEvent
            // Inject a client-side timestamp if the server omits one
            if (!event.ts) event.ts = new Date().toISOString()
            setEvents((prev) => [event, ...prev].slice(0, maxEvents))
          } catch {
            // Ignore malformed frames
          }
        }
      } catch {
        if (!mounted.current) return
        setConnected(false)
        reconnectTimer.current = setTimeout(() => void connect(), 3_000)
      }
    }

    void connect()
    return () => {
      mounted.current = false
      if (reconnectTimer.current) clearTimeout(reconnectTimer.current)
      wsRef.current?.close()
    }
  }, [maxEvents])

  return { events, connected }
}
