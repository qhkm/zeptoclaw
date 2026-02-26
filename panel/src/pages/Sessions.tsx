// Sessions — two-panel layout for browsing conversation sessions.
//
// Left panel  (~300px): searchable session list — key, message count, last active
// Right panel (flex-1): selected session detail — stats bar, chat bubbles, tool calls
//
// Data:
//   GET /api/sessions          → SessionSummary[]
//   GET /api/sessions/{key}    → SessionDetail

import { useState, useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { apiFetch } from '../lib/api'
import ChatBubble from '../components/ChatBubble'
import ToolCallBlock from '../components/ToolCallBlock'

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface SessionSummary {
  key: string
  message_count: number
  last_active: string
}

interface ToolCall {
  name: string
  args: string
  result?: string
  duration_ms?: number
}

interface SessionMessage {
  role: 'user' | 'assistant' | 'tool'
  content: string
  timestamp?: string
  tool_calls?: ToolCall[]
}

interface SessionDetail {
  key: string
  messages: SessionMessage[]
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

function formatRelativeTime(iso: string): string {
  try {
    const diff = Date.now() - new Date(iso).getTime()
    const secs = Math.floor(diff / 1000)
    if (secs < 60) return 'just now'
    const mins = Math.floor(secs / 60)
    if (mins < 60) return `${mins}m ago`
    const hours = Math.floor(mins / 60)
    if (hours < 24) return `${hours}h ago`
    const days = Math.floor(hours / 24)
    return `${days}d ago`
  } catch {
    return iso
  }
}

function formatDuration(messages: SessionMessage[]): string {
  const timestamps = messages
    .map((m) => m.timestamp)
    .filter((t): t is string => Boolean(t))
    .map((t) => new Date(t).getTime())
    .filter((n) => !isNaN(n))
    .sort((a, b) => a - b)

  if (timestamps.length < 2) return '—'

  const ms = timestamps[timestamps.length - 1] - timestamps[0]
  const secs = Math.floor(ms / 1000)
  if (secs < 60) return `${secs}s`
  const mins = Math.floor(secs / 60)
  if (mins < 60) return `${mins}m ${secs % 60}s`
  return `${Math.floor(mins / 60)}h ${mins % 60}m`
}

function countToolCalls(messages: SessionMessage[]): number {
  return messages.reduce((sum, m) => sum + (m.tool_calls?.length ?? 0), 0)
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

function SessionListItem({
  session,
  selected,
  onClick,
}: {
  session: SessionSummary
  selected: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`w-full text-left px-3 py-3 rounded-lg border transition-colors ${
        selected
          ? 'bg-violet-600/15 border-violet-500/40 text-zinc-100'
          : 'bg-transparent border-transparent hover:bg-zinc-800/60 hover:border-zinc-700/60 text-zinc-300'
      }`}
    >
      {/* Key */}
      <p className="text-xs font-mono truncate leading-tight mb-1">
        {session.key}
      </p>

      {/* Meta row */}
      <div className="flex items-center justify-between gap-2">
        <span className="text-[11px] text-zinc-500">
          {session.message_count} msg{session.message_count !== 1 ? 's' : ''}
        </span>
        <span className="text-[11px] text-zinc-600 tabular-nums">
          {formatRelativeTime(session.last_active)}
        </span>
      </div>
    </button>
  )
}

function StatPill({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="flex flex-col items-center gap-0.5 px-4 py-2 bg-zinc-800/50 rounded-lg border border-zinc-700/50">
      <span className="text-base font-bold text-zinc-100 tabular-nums">{value}</span>
      <span className="text-[11px] text-zinc-500 uppercase tracking-wider">{label}</span>
    </div>
  )
}

function EmptyState({ message }: { message: string }) {
  return (
    <div className="flex-1 flex items-center justify-center py-16 text-zinc-600 text-sm">
      {message}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Session detail panel
// ---------------------------------------------------------------------------

function SessionDetailPanel({ sessionKey }: { sessionKey: string }) {
  const { data, isLoading, isError } = useQuery<SessionDetail>({
    queryKey: ['session', sessionKey],
    queryFn: () => apiFetch<SessionDetail>(`/api/sessions/${encodeURIComponent(sessionKey)}`),
    staleTime: 30_000,
  })

  if (isLoading) {
    return (
      <div className="flex-1 flex items-center justify-center text-zinc-600 text-sm">
        Loading session…
      </div>
    )
  }

  if (isError || !data) {
    return (
      <div className="flex-1 flex items-center justify-center text-red-400 text-sm">
        Failed to load session.
      </div>
    )
  }

  const toolCallCount = countToolCalls(data.messages)
  const duration = formatDuration(data.messages)
  const chatMessages = data.messages.filter((m) => m.role !== 'tool')

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* Header */}
      <div className="shrink-0 border-b border-zinc-800 px-5 py-3">
        <p className="text-xs font-mono text-zinc-400 truncate mb-2">{data.key}</p>

        {/* Stats bar */}
        <div className="flex flex-wrap gap-2">
          <StatPill label="Messages" value={data.messages.length} />
          <StatPill label="Tool Calls" value={toolCallCount} />
          <StatPill label="Duration" value={duration} />
        </div>
      </div>

      {/* Chat message list */}
      <div className="flex-1 overflow-y-auto px-5 py-4 space-y-4">
        {chatMessages.length === 0 ? (
          <EmptyState message="No messages in this session." />
        ) : (
          chatMessages.map((msg, idx) => (
            <div key={idx} className="space-y-2">
              {/* Chat bubble for user/assistant */}
              {(msg.role === 'user' || msg.role === 'assistant') && (
                <ChatBubble
                  role={msg.role}
                  content={msg.content}
                  timestamp={msg.timestamp}
                />
              )}

              {/* Inline tool calls */}
              {msg.tool_calls && msg.tool_calls.length > 0 && (
                <div
                  className={`space-y-1.5 ${msg.role === 'assistant' ? 'ml-0 mr-auto max-w-[85%]' : 'ml-auto mr-0 max-w-[85%]'}`}
                >
                  {msg.tool_calls.map((tc, tcIdx) => (
                    <ToolCallBlock
                      key={tcIdx}
                      name={tc.name}
                      args={tc.args}
                      result={tc.result}
                      duration_ms={tc.duration_ms}
                    />
                  ))}
                </div>
              )}
            </div>
          ))
        )}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Sessions page
// ---------------------------------------------------------------------------

export default function Sessions() {
  const [selectedKey, setSelectedKey] = useState<string | null>(null)
  const [search, setSearch] = useState('')

  const { data: sessions, isLoading, isError } = useQuery<SessionSummary[]>({
    queryKey: ['sessions'],
    queryFn: () => apiFetch<SessionSummary[]>('/api/sessions'),
    refetchInterval: 30_000,
    staleTime: 15_000,
  })

  const filtered = useMemo(() => {
    if (!sessions) return []
    const q = search.trim().toLowerCase()
    if (!q) return sessions
    return sessions.filter((s) => s.key.toLowerCase().includes(q))
  }, [sessions, search])

  return (
    <div className="flex flex-col h-full">
      {/* Page header */}
      <div className="shrink-0 mb-4">
        <h1 className="text-2xl font-bold text-zinc-100 mb-1">Sessions</h1>
        <p className="text-zinc-400 text-sm">
          Browse and search past conversation sessions with message history and metadata.
        </p>
      </div>

      {/* Two-panel layout */}
      <div className="flex flex-1 min-h-0 gap-3 overflow-hidden">
        {/* Left panel — session list */}
        <div
          className="shrink-0 flex flex-col bg-zinc-900 rounded-lg border border-zinc-800 overflow-hidden"
          style={{ width: 300 }}
        >
          {/* Search */}
          <div className="shrink-0 p-3 border-b border-zinc-800">
            <div className="relative">
              <svg
                className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-zinc-500 pointer-events-none"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
                aria-hidden="true"
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M21 21l-5.197-5.197m0 0A7.5 7.5 0 105.196 5.196a7.5 7.5 0 0010.607 10.607z"
                />
              </svg>
              <input
                type="search"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Search sessions…"
                className="w-full bg-zinc-800 border border-zinc-700 rounded-md pl-8 pr-3 py-1.5 text-sm text-zinc-200 placeholder-zinc-600 focus:outline-none focus:border-violet-500/60 focus:ring-1 focus:ring-violet-500/30"
                aria-label="Search sessions by key"
              />
            </div>
          </div>

          {/* List body */}
          <div className="flex-1 overflow-y-auto p-2 space-y-1">
            {isLoading && (
              <p className="text-zinc-600 text-sm px-2 py-4">Loading sessions…</p>
            )}
            {isError && (
              <p className="text-red-400 text-sm px-2 py-4">Failed to load sessions.</p>
            )}
            {!isLoading && !isError && filtered.length === 0 && (
              <p className="text-zinc-600 text-sm px-2 py-4 italic">
                {search ? 'No sessions match your search.' : 'No sessions found.'}
              </p>
            )}
            {filtered.map((session) => (
              <SessionListItem
                key={session.key}
                session={session}
                selected={selectedKey === session.key}
                onClick={() => setSelectedKey(session.key)}
              />
            ))}
          </div>

          {/* Footer count */}
          {sessions && (
            <div className="shrink-0 border-t border-zinc-800 px-3 py-2 text-[11px] text-zinc-600">
              {filtered.length} of {sessions.length} session{sessions.length !== 1 ? 's' : ''}
            </div>
          )}
        </div>

        {/* Right panel — session detail */}
        <div className="flex-1 bg-zinc-900 rounded-lg border border-zinc-800 overflow-hidden flex flex-col min-w-0">
          {selectedKey ? (
            <SessionDetailPanel sessionKey={selectedKey} />
          ) : (
            <div className="flex-1 flex flex-col items-center justify-center gap-3 text-zinc-600">
              <svg
                className="w-10 h-10 opacity-40"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={1.5}
                aria-hidden="true"
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M8.625 12a.375.375 0 11-.75 0 .375.375 0 01.75 0zm0 0H8.25m4.125 0a.375.375 0 11-.75 0 .375.375 0 01.75 0zm0 0H12m4.125 0a.375.375 0 11-.75 0 .375.375 0 01.75 0zm0 0h-.375M21 12c0 4.556-4.03 8.25-9 8.25a9.764 9.764 0 01-2.555-.337A5.972 5.972 0 015.41 20.97a5.969 5.969 0 01-.474-.065 4.48 4.48 0 00.978-2.025c.09-.457-.133-.901-.467-1.226C3.93 16.178 3 14.189 3 12c0-4.556 4.03-8.25 9-8.25s9 3.694 9 8.25z"
                />
              </svg>
              <p className="text-sm">Select a session to view its messages</p>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
