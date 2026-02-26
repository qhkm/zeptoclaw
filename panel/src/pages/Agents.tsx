// Agents — Live Agent Office.
//
// Derives agent state from WebSocket events:
//   agent_started { session_key }          → add desk (active)
//   tool_started  { session_key, tool }    → show tool animation on desk
//   tool_done     { session_key, tool, duration_ms } → clear tool animation
//   agent_done    { session_key, tokens }  → mark desk as done with final tokens
//
// Desks that have been "done" for >60s fade to dimmed opacity (idle state).

import { useMemo, useEffect, useReducer, useRef } from 'react'
import { useNavigate } from 'react-router'
import { useWebSocket } from '../hooks/useWebSocket'
import AgentDesk, { type AgentState } from '../components/AgentDesk'

// ---------------------------------------------------------------------------
// State management
// ---------------------------------------------------------------------------

type AgentMap = Map<string, AgentState>

type Action =
  | { type: 'AGENT_STARTED'; sessionKey: string; channel: string; ts: string }
  | { type: 'TOOL_STARTED'; sessionKey: string; tool: string; ts: string }
  | { type: 'TOOL_DONE'; sessionKey: string; ts: string }
  | { type: 'AGENT_DONE'; sessionKey: string; tokens: number; ts: string }
  | { type: 'TICK' }   // periodic tick to re-evaluate idle fade

function extractChannel(sessionKey: string): string {
  // Session keys are typically "channel:chatid" or just "channel"
  // e.g. "telegram:123456789" → "telegram"
  //      "webhook"            → "webhook"
  const colonIdx = sessionKey.indexOf(':')
  if (colonIdx > 0) return sessionKey.slice(0, colonIdx).toLowerCase()
  return sessionKey.toLowerCase()
}

function makeDesk(sessionKey: string, ts: string): AgentState {
  return {
    sessionKey,
    channel: extractChannel(sessionKey),
    status: 'active',
    currentTool: undefined,
    totalTokens: 0,
    startedAt: ts,
    lastActivity: ts,
  }
}

function reducer(state: AgentMap, action: Action): AgentMap {
  switch (action.type) {
    case 'AGENT_STARTED': {
      const next = new Map(state)
      // If the desk already exists, reset it to active (re-started session)
      const existing = next.get(action.sessionKey)
      next.set(action.sessionKey, {
        ...(existing ?? makeDesk(action.sessionKey, action.ts)),
        status: 'active',
        currentTool: undefined,
        lastActivity: action.ts,
        startedAt: existing?.startedAt ?? action.ts,
        channel: action.channel || extractChannel(action.sessionKey),
      })
      return next
    }

    case 'TOOL_STARTED': {
      const next = new Map(state)
      const desk = next.get(action.sessionKey) ?? makeDesk(action.sessionKey, action.ts)
      next.set(action.sessionKey, {
        ...desk,
        status: 'active',
        currentTool: action.tool,
        lastActivity: action.ts,
      })
      return next
    }

    case 'TOOL_DONE': {
      const next = new Map(state)
      const desk = next.get(action.sessionKey)
      if (desk) {
        next.set(action.sessionKey, {
          ...desk,
          currentTool: undefined,
          lastActivity: action.ts,
        })
      }
      return next
    }

    case 'AGENT_DONE': {
      const next = new Map(state)
      const desk = next.get(action.sessionKey) ?? makeDesk(action.sessionKey, action.ts)
      next.set(action.sessionKey, {
        ...desk,
        status: 'done',
        currentTool: undefined,
        totalTokens: action.tokens,
        lastActivity: action.ts,
      })
      return next
    }

    case 'TICK': {
      // Transition desks that have been "done" for >60s to "idle" (dimmed)
      const now = Date.now()
      let changed = false
      const next = new Map(state)
      for (const [key, desk] of next) {
        if (desk.status === 'done') {
          const age = now - new Date(desk.lastActivity).getTime()
          if (age > 60_000) {
            next.set(key, { ...desk, status: 'idle' })
            changed = true
          }
        }
      }
      return changed ? next : state
    }

    default:
      return state
  }
}

// ---------------------------------------------------------------------------
// Agents page
// ---------------------------------------------------------------------------

export default function Agents() {
  const navigate = useNavigate()
  const { events, connected } = useWebSocket(200)
  const [agentMap, dispatch] = useReducer(reducer, new Map<string, AgentState>())

  // Track which event indices we've already processed so we don't re-process
  // on re-renders. useWebSocket prepends new events; track by stable reference.
  const processedRef = useRef(new Set<object>())

  // Process incoming WebSocket events
  useEffect(() => {
    for (const event of events) {
      if (processedRef.current.has(event)) continue
      processedRef.current.add(event)

      const ts = (event.ts as string | undefined) ?? new Date().toISOString()

      if (event.type === 'agent_started') {
        const sessionKey = (event.session_key as string | undefined) ?? ''
        const channel = (event.channel as string | undefined) ?? extractChannel(sessionKey)
        if (sessionKey) {
          dispatch({ type: 'AGENT_STARTED', sessionKey, channel, ts })
        }
      } else if (event.type === 'tool_started') {
        const sessionKey = (event.session_key as string | undefined) ?? ''
        const tool = (event.tool as string | undefined) ?? ''
        if (sessionKey) {
          dispatch({ type: 'TOOL_STARTED', sessionKey, tool, ts })
        }
      } else if (event.type === 'tool_done') {
        const sessionKey = (event.session_key as string | undefined) ?? ''
        if (sessionKey) {
          dispatch({ type: 'TOOL_DONE', sessionKey, ts })
        }
      } else if (event.type === 'agent_done') {
        const sessionKey = (event.session_key as string | undefined) ?? ''
        const tokens = typeof event.tokens === 'number' ? event.tokens : 0
        if (sessionKey) {
          dispatch({ type: 'AGENT_DONE', sessionKey, tokens, ts })
        }
      }
    }
  }, [events])

  // Periodic tick to transition "done" → "idle" after 60s
  useEffect(() => {
    const id = setInterval(() => dispatch({ type: 'TICK' }), 10_000)
    return () => clearInterval(id)
  }, [])

  const agents = useMemo(() => Array.from(agentMap.values()), [agentMap])

  const activeCount = useMemo(
    () => agents.filter((a) => a.status === 'active').length,
    [agents]
  )

  function handleDeskClick(sessionKey: string) {
    navigate(`/sessions?key=${encodeURIComponent(sessionKey)}`)
  }

  return (
    <div className="flex flex-col h-full">
      {/* Page header */}
      <div className="shrink-0 mb-6">
        <div className="flex items-start justify-between gap-4 flex-wrap">
          <div>
            <h1 className="text-2xl font-bold text-zinc-100 mb-1">Agent Office</h1>
            <p className="text-zinc-400 text-sm">
              Live view of all agent sessions — watch tools execute in real time.
            </p>
          </div>

          {/* Status bar */}
          <div className="flex items-center gap-3 flex-wrap">
            {/* Active count badge */}
            {activeCount > 0 && (
              <span className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-emerald-500/10 border border-emerald-500/20 text-emerald-400 text-sm font-medium">
                <span className="w-2 h-2 rounded-full bg-emerald-400 animate-pulse" />
                {activeCount} active
              </span>
            )}

            {/* WebSocket connection status */}
            <span
              className={`inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border text-sm ${
                connected
                  ? 'bg-zinc-800/60 border-zinc-700/60 text-zinc-400'
                  : 'bg-zinc-900 border-zinc-800 text-zinc-600'
              }`}
            >
              <span
                className={`w-2 h-2 rounded-full ${
                  connected ? 'bg-emerald-400' : 'bg-zinc-600'
                }`}
              />
              {connected ? 'Live' : 'Reconnecting\u2026'}
            </span>
          </div>
        </div>
      </div>

      {/* Agent grid or empty state */}
      {agents.length === 0 ? (
        <EmptyState connected={connected} />
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3 content-start">
          {agents.map((agent) => (
            <AgentDesk
              key={agent.sessionKey}
              agent={agent}
              onClick={() => handleDeskClick(agent.sessionKey)}
            />
          ))}
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Empty state
// ---------------------------------------------------------------------------

function EmptyState({ connected }: { connected: boolean }) {
  return (
    <div className="flex-1 flex flex-col items-center justify-center gap-4 py-20 text-zinc-600">
      {/* Monitor icon */}
      <svg
        className="w-12 h-12 opacity-30"
        fill="none"
        viewBox="0 0 24 24"
        stroke="currentColor"
        strokeWidth={1.25}
        aria-hidden="true"
      >
        <path
          strokeLinecap="round"
          strokeLinejoin="round"
          d="M9 17.25v1.007a3 3 0 01-.879 2.122L7.5 21h9l-.621-.621A3 3 0 0115 18.257V17.25m6-12V15a2.25 2.25 0 01-2.25 2.25H5.25A2.25 2.25 0 013 15V5.25m18 0A2.25 2.25 0 0018.75 3H5.25A2.25 2.25 0 003 5.25m18 0H3"
        />
      </svg>

      <div className="text-center space-y-1">
        <p className="text-sm font-medium text-zinc-500">No active agents</p>
        <p className="text-xs text-zinc-700">
          {connected
            ? 'Waiting for agent sessions to start\u2026'
            : 'Connecting to event stream\u2026'}
        </p>
      </div>
    </div>
  )
}
