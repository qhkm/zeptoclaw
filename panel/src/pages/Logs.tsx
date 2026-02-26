import { useState, useRef, useEffect, useCallback } from 'react'
import { useWebSocket, PanelEvent } from '../hooks/useWebSocket'

// Filter category definitions
type FilterCategory = 'all' | 'tool' | 'agent' | 'channel' | 'cron' | 'error'

const FILTER_CATEGORIES: { label: string; value: FilterCategory }[] = [
  { label: 'All', value: 'all' },
  { label: 'Tool', value: 'tool' },
  { label: 'Agent', value: 'agent' },
  { label: 'Channel', value: 'channel' },
  { label: 'Cron', value: 'cron' },
  { label: 'Error', value: 'error' },
]

// Map event type strings to badge color classes
function getTypeBadgeClass(type: string): string {
  switch (type) {
    case 'tool_started':
      return 'bg-blue-500/20 text-blue-300 border border-blue-500/30'
    case 'tool_done':
      return 'bg-green-500/20 text-green-300 border border-green-500/30'
    case 'tool_failed':
      return 'bg-red-500/20 text-red-300 border border-red-500/30'
    case 'agent_started':
    case 'agent_done':
      return 'bg-violet-500/20 text-violet-300 border border-violet-500/30'
    case 'message_received':
      return 'bg-yellow-500/20 text-yellow-300 border border-yellow-500/30'
    case 'cron_fired':
      return 'bg-orange-500/20 text-orange-300 border border-orange-500/30'
    case 'channel_status':
      return 'bg-cyan-500/20 text-cyan-300 border border-cyan-500/30'
    default:
      return 'bg-zinc-700/60 text-zinc-400 border border-zinc-600/40'
  }
}

// Map event type to its filter category
function getEventCategory(type: string): FilterCategory {
  if (type.startsWith('tool_')) return 'tool'
  if (type.startsWith('agent_')) return 'agent'
  if (type === 'channel_status') return 'channel'
  if (type === 'cron_fired') return 'cron'
  if (type === 'tool_failed' || type.includes('error') || type.includes('fail')) return 'error'
  if (type === 'message_received') return 'channel'
  return 'all'
}

// Format ISO timestamp → HH:MM:SS.mmm
function formatTimestamp(ts: string | undefined): string {
  if (!ts) return '--:--:--.---'
  try {
    const d = new Date(ts)
    const hh = String(d.getHours()).padStart(2, '0')
    const mm = String(d.getMinutes()).padStart(2, '0')
    const ss = String(d.getSeconds()).padStart(2, '0')
    const ms = String(d.getMilliseconds()).padStart(3, '0')
    return `${hh}:${mm}:${ss}.${ms}`
  } catch {
    return '--:--:--.---'
  }
}

// Extract a human-readable summary from an event
function getEventSummary(event: PanelEvent): string {
  const { type, ...rest } = event
  // Common fields to surface
  const tool = rest.tool as string | undefined
  const message = rest.message as string | undefined
  const channel = rest.channel as string | undefined
  const name = rest.name as string | undefined
  const schedule = rest.schedule as string | undefined
  const error = rest.error as string | undefined

  if (type === 'tool_started' || type === 'tool_done' || type === 'tool_failed') {
    const toolName = tool || name || 'unknown tool'
    if (error) return `${toolName} — ${error}`
    return toolName
  }
  if (type === 'agent_started') return message || 'Agent started'
  if (type === 'agent_done') return message || 'Agent done'
  if (type === 'message_received') {
    const ch = channel ? `[${channel}] ` : ''
    return `${ch}${message || 'Message received'}`
  }
  if (type === 'cron_fired') return schedule ? `Schedule: ${schedule}` : 'Cron fired'
  if (type === 'channel_status') {
    const ch = channel ? `${channel}: ` : ''
    return `${ch}${message || 'Status update'}`
  }
  // Fallback: show the first string value we find
  const firstStr = Object.values(rest).find((v) => typeof v === 'string') as string | undefined
  return firstStr || type
}

// Extract error details for expandable panel
function getErrorDetails(event: PanelEvent): string | null {
  if (event.type !== 'tool_failed') return null
  const error = event.error as string | undefined
  const details = event.details as string | undefined
  const stack = event.stack as string | undefined
  return details || stack || error || null
}

export default function Logs() {
  const { events: liveEvents, connected } = useWebSocket(200)

  // Paused state: when true, freeze the displayed list
  const [paused, setPaused] = useState(false)
  const [displayedEvents, setDisplayedEvents] = useState<PanelEvent[]>([])
  const [filter, setFilter] = useState<FilterCategory>('all')
  const [expandedIndex, setExpandedIndex] = useState<number | null>(null)
  const [isAtBottom, setIsAtBottom] = useState(true)

  // Scroll container ref for auto-scroll
  const scrollRef = useRef<HTMLDivElement>(null)
  // Track previous event count to detect new arrivals
  const prevCountRef = useRef(0)

  // Sync live events to displayed list only when not paused
  useEffect(() => {
    if (!paused) {
      setDisplayedEvents(liveEvents)
    }
  }, [liveEvents, paused])

  // Auto-scroll to bottom when new events arrive and user is at bottom
  useEffect(() => {
    const newCount = liveEvents.length
    if (newCount !== prevCountRef.current && isAtBottom && !paused) {
      const el = scrollRef.current
      if (el) {
        el.scrollTop = el.scrollHeight
      }
    }
    prevCountRef.current = newCount
  }, [liveEvents, isAtBottom, paused])

  // Detect scroll position to toggle "jump to latest" button
  const handleScroll = useCallback(() => {
    const el = scrollRef.current
    if (!el) return
    const threshold = 48
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight <= threshold
    setIsAtBottom(atBottom)
  }, [])

  const scrollToBottom = () => {
    const el = scrollRef.current
    if (el) {
      el.scrollTop = el.scrollHeight
      setIsAtBottom(true)
    }
  }

  const handleClear = () => {
    setDisplayedEvents([])
    setExpandedIndex(null)
  }

  const togglePause = () => {
    setPaused((prev) => {
      // When resuming, sync immediately
      if (prev) setDisplayedEvents(liveEvents)
      return !prev
    })
  }

  // Filter displayed events by category
  const filteredEvents = displayedEvents.filter((event) => {
    if (filter === 'all') return true
    if (filter === 'error') {
      return (
        event.type === 'tool_failed' ||
        event.type.includes('error') ||
        event.type.includes('fail')
      )
    }
    return getEventCategory(event.type) === filter
  })

  return (
    <div className="flex flex-col h-full gap-4">
      {/* Header */}
      <div className="flex items-center justify-between flex-wrap gap-3">
        <div className="flex items-center gap-3">
          <h1 className="text-2xl font-bold text-zinc-100">Logs</h1>
          {/* WebSocket status dot */}
          <span
            className={`inline-flex items-center gap-1.5 text-xs px-2 py-0.5 rounded-full border ${
              connected
                ? 'bg-green-500/10 text-green-400 border-green-500/30'
                : 'bg-red-500/10 text-red-400 border-red-500/30'
            }`}
          >
            <span
              className={`inline-block w-1.5 h-1.5 rounded-full ${
                connected ? 'bg-green-400 animate-pulse' : 'bg-red-400'
              }`}
            />
            {connected ? 'Live' : 'Disconnected'}
          </span>
        </div>

        <div className="flex items-center gap-2">
          {/* Pause / Resume */}
          <button
            onClick={togglePause}
            className={`text-xs px-3 py-1.5 rounded-md border font-medium transition-colors ${
              paused
                ? 'bg-violet-500/20 text-violet-300 border-violet-500/40 hover:bg-violet-500/30'
                : 'bg-zinc-800 text-zinc-300 border-zinc-700 hover:bg-zinc-700'
            }`}
          >
            {paused ? 'Resume' : 'Pause'}
          </button>

          {/* Clear */}
          <button
            onClick={handleClear}
            className="text-xs px-3 py-1.5 rounded-md border bg-zinc-800 text-zinc-400 border-zinc-700 hover:bg-zinc-700 hover:text-zinc-200 transition-colors font-medium"
          >
            Clear
          </button>
        </div>
      </div>

      {/* Filter chips */}
      <div className="flex items-center gap-2 flex-wrap">
        {FILTER_CATEGORIES.map(({ label, value }) => (
          <button
            key={value}
            onClick={() => setFilter(value)}
            className={`text-xs px-3 py-1 rounded-full border font-medium transition-colors ${
              filter === value
                ? 'bg-violet-500/20 text-violet-300 border-violet-500/40'
                : 'bg-zinc-800 text-zinc-400 border-zinc-700 hover:bg-zinc-700 hover:text-zinc-300'
            }`}
          >
            {label}
          </button>
        ))}
        <span className="ml-auto text-xs text-zinc-600">
          {filteredEvents.length} event{filteredEvents.length !== 1 ? 's' : ''}
          {paused && <span className="ml-2 text-yellow-500/80">(paused)</span>}
        </span>
      </div>

      {/* Log list — scrollable, relative for floating button */}
      <div className="relative flex-1 min-h-0">
        <div
          ref={scrollRef}
          onScroll={handleScroll}
          className="h-full overflow-y-auto bg-zinc-900 rounded-lg border border-zinc-800 font-mono text-xs"
        >
          {filteredEvents.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-full gap-2 text-zinc-600 py-16">
              <svg
                className="w-8 h-8 opacity-40"
                fill="none"
                stroke="currentColor"
                viewBox="0 0 24 24"
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  strokeWidth={1.5}
                  d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
                />
              </svg>
              <span>No events yet</span>
              {!connected && (
                <span className="text-red-400/60 text-xs">WebSocket disconnected — reconnecting…</span>
              )}
            </div>
          ) : (
            // Events are stored newest-first (index 0 = newest), so reverse for
            // chronological display (oldest at top, newest at bottom)
            [...filteredEvents].reverse().map((event, idx) => {
              const isExpanded = expandedIndex === idx
              const errorDetails = getErrorDetails(event)
              const isError = event.type === 'tool_failed'

              return (
                <div
                  key={idx}
                  className={`group border-b border-zinc-800/60 last:border-0 ${
                    isError ? 'cursor-pointer hover:bg-zinc-800/40' : ''
                  } ${isExpanded ? 'bg-zinc-800/30' : ''}`}
                  onClick={() => {
                    if (isError) {
                      setExpandedIndex(isExpanded ? null : idx)
                    }
                  }}
                >
                  {/* Main row */}
                  <div className="flex items-start gap-3 px-4 py-2.5">
                    {/* Timestamp */}
                    <span className="shrink-0 text-zinc-600 tabular-nums w-[88px]">
                      {formatTimestamp(event.ts)}
                    </span>

                    {/* Type badge */}
                    <span
                      className={`shrink-0 inline-block px-1.5 py-0.5 rounded text-[10px] leading-tight font-semibold tracking-wide ${getTypeBadgeClass(event.type)}`}
                    >
                      {event.type}
                    </span>

                    {/* Summary */}
                    <span className="text-zinc-300 min-w-0 break-all">
                      {getEventSummary(event)}
                    </span>

                    {/* Expand chevron for errors */}
                    {isError && errorDetails && (
                      <span className="ml-auto shrink-0 text-zinc-600 group-hover:text-zinc-400 transition-colors">
                        <svg
                          className={`w-3 h-3 transition-transform ${isExpanded ? 'rotate-90' : ''}`}
                          fill="none"
                          stroke="currentColor"
                          viewBox="0 0 24 24"
                        >
                          <path
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            strokeWidth={2}
                            d="M9 5l7 7-7 7"
                          />
                        </svg>
                      </span>
                    )}
                  </div>

                  {/* Expanded error details */}
                  {isExpanded && errorDetails && (
                    <div className="px-4 pb-3 pt-0 ml-[88px]">
                      <pre className="bg-zinc-950 border border-red-500/20 rounded p-3 text-red-300/80 text-[11px] whitespace-pre-wrap break-all overflow-auto max-h-40">
                        {errorDetails}
                      </pre>
                    </div>
                  )}
                </div>
              )
            })
          )}
        </div>

        {/* Jump to latest floating button */}
        {!isAtBottom && filteredEvents.length > 0 && (
          <button
            onClick={scrollToBottom}
            className="absolute bottom-4 right-4 flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-full bg-violet-600 hover:bg-violet-500 text-white shadow-lg shadow-violet-900/40 transition-colors border border-violet-500/40"
          >
            <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
            </svg>
            Jump to latest
          </button>
        )}
      </div>
    </div>
  )
}
