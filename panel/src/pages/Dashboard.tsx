// Dashboard — health overview, usage stats, live activity feed.
//
// Sections:
//   1. Status bar  — health pill, version, uptime, RSS memory
//   2. Stats row   — 4 KPI cards from health.usage
//   3. Activity    — last 10 WebSocket events, scrollable
//   4. Components  — health check table from health.components

import { useHealth } from '../hooks/useHealth'
import { useWebSocket, type PanelEvent } from '../hooks/useWebSocket'

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

function formatUptime(secs: number): string {
  const d = Math.floor(secs / 86_400)
  const h = Math.floor((secs % 86_400) / 3_600)
  const m = Math.floor((secs % 3_600) / 60)
  const parts: string[] = []
  if (d > 0) parts.push(`${d}d`)
  if (h > 0 || d > 0) parts.push(`${h}h`)
  parts.push(`${m}m`)
  return parts.join(' ')
}

function formatBytes(bytes: number): string {
  if (bytes >= 1_048_576) return `${(bytes / 1_048_576).toFixed(1)} MB`
  if (bytes >= 1_024) return `${(bytes / 1_024).toFixed(1)} KB`
  return `${bytes} B`
}

function formatNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`
  return String(n)
}

function formatTs(iso: string): string {
  try {
    return new Date(iso).toLocaleTimeString([], {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    })
  } catch {
    return iso
  }
}

// ---------------------------------------------------------------------------
// Event type → label + colour
// ---------------------------------------------------------------------------

const EVENT_LABELS: Record<string, { label: string; color: string }> = {
  tool_call:      { label: 'Tool Call',      color: 'bg-violet-500/20 text-violet-300' },
  tool_result:    { label: 'Tool Result',    color: 'bg-blue-500/20 text-blue-300' },
  agent_start:    { label: 'Agent Start',    color: 'bg-emerald-500/20 text-emerald-300' },
  agent_stop:     { label: 'Agent Stop',     color: 'bg-zinc-500/20 text-zinc-300' },
  message:        { label: 'Message',        color: 'bg-sky-500/20 text-sky-300' },
  error:          { label: 'Error',          color: 'bg-red-500/20 text-red-300' },
  memory_set:     { label: 'Memory Set',     color: 'bg-amber-500/20 text-amber-300' },
  memory_get:     { label: 'Memory Get',     color: 'bg-amber-500/20 text-amber-300' },
  compaction:     { label: 'Compaction',     color: 'bg-orange-500/20 text-orange-300' },
  routine_run:    { label: 'Routine Run',    color: 'bg-teal-500/20 text-teal-300' },
  heartbeat:      { label: 'Heartbeat',      color: 'bg-pink-500/20 text-pink-300' },
}

function eventMeta(type: string) {
  return EVENT_LABELS[type] ?? { label: type, color: 'bg-zinc-700/40 text-zinc-400' }
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

function StatCard({
  label,
  value,
  accent,
}: {
  label: string
  value: string | number
  accent?: string
}) {
  return (
    <div className="bg-zinc-900 rounded-lg border border-zinc-800 p-4 flex flex-col gap-1">
      <span className={`text-2xl font-bold tracking-tight ${accent ?? 'text-zinc-100'}`}>
        {typeof value === 'number' ? formatNumber(value) : value}
      </span>
      <span className="text-xs text-zinc-500 uppercase tracking-wider">{label}</span>
    </div>
  )
}

function EventRow({ event }: { event: PanelEvent }) {
  const { label, color } = eventMeta(event.type)
  const ts = typeof event.ts === 'string' ? formatTs(event.ts) : ''

  // Build a short detail string from remaining keys
  const detail = Object.entries(event)
    .filter(([k]) => k !== 'type' && k !== 'ts')
    .map(([k, v]) => `${k}: ${typeof v === 'object' ? JSON.stringify(v) : String(v)}`)
    .slice(0, 3)
    .join('  |  ')

  return (
    <li className="flex items-start gap-3 py-2.5 border-b border-zinc-800/60 last:border-0">
      <span className={`mt-0.5 shrink-0 inline-flex items-center px-2 py-0.5 rounded text-[11px] font-medium ${color}`}>
        {label}
      </span>
      <span className="flex-1 text-sm text-zinc-300 truncate">{detail || <span className="text-zinc-600 italic">no details</span>}</span>
      {ts && (
        <span className="shrink-0 text-xs text-zinc-600 tabular-nums">{ts}</span>
      )}
    </li>
  )
}

function ComponentRow({ name, status, error }: { name: string; status: string; error?: string }) {
  const isUp = status === 'Up' || status === 'up' || status === 'ok'
  return (
    <tr className="border-b border-zinc-800/60 last:border-0">
      <td className="py-2 pr-4 text-sm text-zinc-300 font-mono">{name}</td>
      <td className="py-2 pr-4">
        <span
          className={`inline-flex items-center gap-1.5 text-xs font-medium px-2 py-0.5 rounded ${
            isUp
              ? 'bg-emerald-500/15 text-emerald-400'
              : 'bg-red-500/15 text-red-400'
          }`}
        >
          <span
            className={`w-1.5 h-1.5 rounded-full ${isUp ? 'bg-emerald-400' : 'bg-red-400'}`}
          />
          {status}
        </span>
      </td>
      <td className="py-2 text-xs text-zinc-500 truncate max-w-xs">{error ?? ''}</td>
    </tr>
  )
}

// ---------------------------------------------------------------------------
// Dashboard
// ---------------------------------------------------------------------------

export default function Dashboard() {
  const { data: health, isLoading: healthLoading, isError: healthError } = useHealth()
  const { events, connected } = useWebSocket(50)

  const isHealthy =
    health?.status === 'ok' || health?.status === 'healthy' || health?.status === 'Up'

  return (
    <div className="space-y-6">
      {/* Page header */}
      <div>
        <h1 className="text-2xl font-bold text-zinc-100 mb-1">Dashboard</h1>
        <p className="text-zinc-400 text-sm">
          Overview of your ZeptoClaw agent — activity, health, and recent events.
        </p>
      </div>

      {/* Status bar */}
      <div className="bg-zinc-900 rounded-lg border border-zinc-800 p-4 flex flex-wrap items-center gap-4">
        {/* Health pill */}
        {healthLoading && (
          <span className="inline-flex items-center gap-1.5 px-3 py-1 rounded-full text-sm bg-zinc-700/40 text-zinc-400">
            <span className="w-2 h-2 rounded-full bg-zinc-500 animate-pulse" />
            Connecting…
          </span>
        )}
        {healthError && (
          <span className="inline-flex items-center gap-1.5 px-3 py-1 rounded-full text-sm bg-red-500/20 text-red-400">
            <span className="w-2 h-2 rounded-full bg-red-400" />
            Unreachable
          </span>
        )}
        {health && (
          <span
            className={`inline-flex items-center gap-1.5 px-3 py-1 rounded-full text-sm font-medium ${
              isHealthy
                ? 'bg-emerald-500/20 text-emerald-400'
                : 'bg-red-500/20 text-red-400'
            }`}
          >
            <span
              className={`w-2 h-2 rounded-full ${isHealthy ? 'bg-emerald-400' : 'bg-red-400'}`}
            />
            {isHealthy ? 'Healthy' : health.status}
          </span>
        )}

        {/* Meta row */}
        {health && (
          <>
            <div className="flex items-center gap-1 text-sm text-zinc-400">
              <span className="text-zinc-600">v</span>
              <span className="text-zinc-300 font-mono">{health.version}</span>
            </div>
            <div className="flex items-center gap-1 text-sm text-zinc-400">
              <span className="text-zinc-600">Uptime</span>
              <span className="text-zinc-300 font-mono">{formatUptime(health.uptime_secs)}</span>
            </div>
            <div className="flex items-center gap-1 text-sm text-zinc-400">
              <span className="text-zinc-600">RSS</span>
              <span className="text-zinc-300 font-mono">{formatBytes(health.rss_bytes)}</span>
            </div>
          </>
        )}

        {/* WebSocket status — right-aligned */}
        <div className="ml-auto flex items-center gap-2 text-xs text-zinc-500">
          <span
            className={`w-2 h-2 rounded-full ${connected ? 'bg-emerald-400' : 'bg-zinc-600'}`}
          />
          {connected ? 'Live' : 'Reconnecting…'}
        </div>
      </div>

      {/* Stats row */}
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
        <StatCard
          label="Requests"
          value={health?.usage.requests ?? 0}
          accent="text-sky-300"
        />
        <StatCard
          label="Tool Calls"
          value={health?.usage.tool_calls ?? 0}
          accent="text-violet-300"
        />
        <StatCard
          label="Tokens"
          value={health?.usage.tokens ?? 0}
          accent="text-amber-300"
        />
        <StatCard
          label="Errors"
          value={health?.usage.errors ?? 0}
          accent={health && health.usage.errors > 0 ? 'text-red-400' : 'text-zinc-400'}
        />
      </div>

      {/* Two-column lower section */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        {/* Activity feed */}
        <div className="bg-zinc-900 rounded-lg border border-zinc-800 p-4 flex flex-col">
          <div className="flex items-center justify-between mb-3">
            <h2 className="text-sm font-semibold text-zinc-200">Live Activity</h2>
            <span className="text-xs text-zinc-600">{events.length} events</span>
          </div>

          {events.length === 0 ? (
            <div className="flex-1 flex items-center justify-center py-12 text-zinc-600 text-sm">
              {connected ? 'Waiting for events…' : 'Not connected'}
            </div>
          ) : (
            <ul className="overflow-y-auto max-h-72 divide-y-0">
              {events.slice(0, 10).map((ev, i) => (
                // eslint-disable-next-line react/no-array-index-key
                <EventRow key={i} event={ev} />
              ))}
            </ul>
          )}
        </div>

        {/* Components health table */}
        <div className="bg-zinc-900 rounded-lg border border-zinc-800 p-4 flex flex-col">
          <h2 className="text-sm font-semibold text-zinc-200 mb-3">Components</h2>

          {healthLoading && (
            <p className="text-zinc-600 text-sm py-4">Loading…</p>
          )}
          {healthError && (
            <p className="text-red-400 text-sm py-4">Could not reach health endpoint.</p>
          )}
          {health && Object.keys(health.components).length === 0 && (
            <p className="text-zinc-600 text-sm py-4">No components registered.</p>
          )}
          {health && Object.keys(health.components).length > 0 && (
            <div className="overflow-x-auto">
              <table className="w-full text-left">
                <thead>
                  <tr className="border-b border-zinc-800">
                    <th className="pb-2 text-xs text-zinc-600 uppercase tracking-wider pr-4 font-medium">
                      Name
                    </th>
                    <th className="pb-2 text-xs text-zinc-600 uppercase tracking-wider pr-4 font-medium">
                      Status
                    </th>
                    <th className="pb-2 text-xs text-zinc-600 uppercase tracking-wider font-medium">
                      Detail
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {Object.entries(health.components).map(([name, check]) => (
                    <ComponentRow
                      key={name}
                      name={name}
                      status={check.status}
                      error={check.error}
                    />
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
