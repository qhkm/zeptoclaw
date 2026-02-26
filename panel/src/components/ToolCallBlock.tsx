// ToolCallBlock — collapsible block showing a tool call name, arguments,
// optional result, and duration. Click the header to expand/collapse.
//
// Usage:
//   <ToolCallBlock
//     name="web_search"
//     args='{"query":"zeptoclaw"}'
//     result='{"results":[...]}'
//     duration_ms={342}
//   />

import { useState } from 'react'

interface ToolCallBlockProps {
  name: string
  args: string
  result?: string
  duration_ms?: number
}

function formatDuration(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`
  return `${ms}ms`
}

function tryPrettyJson(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2)
  } catch {
    return raw
  }
}

export default function ToolCallBlock({ name, args, result, duration_ms }: ToolCallBlockProps) {
  const [expanded, setExpanded] = useState(false)

  const prettyArgs = tryPrettyJson(args)
  const prettyResult = result != null ? tryPrettyJson(result) : undefined

  return (
    <div className="bg-zinc-800/50 border border-zinc-700 rounded-md text-sm overflow-hidden">
      {/* Header — always visible, click to toggle */}
      <button
        type="button"
        onClick={() => setExpanded((prev) => !prev)}
        className="w-full flex items-center justify-between gap-3 px-3 py-2 hover:bg-zinc-700/40 transition-colors text-left"
        aria-expanded={expanded}
      >
        <div className="flex items-center gap-2 min-w-0">
          {/* Chevron indicator */}
          <svg
            className={`shrink-0 w-3.5 h-3.5 text-zinc-500 transition-transform ${expanded ? 'rotate-90' : ''}`}
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
            aria-hidden="true"
          >
            <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
          </svg>

          {/* Tool icon */}
          <svg
            className="shrink-0 w-3.5 h-3.5 text-cyan-500"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
            aria-hidden="true"
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M11.42 15.17L17.25 21A2.652 2.652 0 0021 17.25l-5.877-5.877M11.42 15.17l2.496-3.03c.317-.384.74-.626 1.208-.766M11.42 15.17l-4.655 5.653a2.548 2.548 0 11-3.586-3.586l6.837-5.63m5.108-.233c.55-.164 1.163-.188 1.743-.14a4.5 4.5 0 004.486-6.336l-3.276 3.277a3.004 3.004 0 01-2.25-2.25l3.276-3.276a4.5 4.5 0 00-6.336 4.486c.091 1.076-.071 2.264-.904 2.95l-.102.085m-1.745 1.437L5.909 7.5H4.5L2.25 3.75l1.5-1.5L7.5 4.5v1.409l4.26 4.26m-1.745 1.437l1.745-1.437m6.615 8.206L15.75 15.75M4.867 19.125h.008v.008h-.008v-.008z"
            />
          </svg>

          {/* Tool name */}
          <span className="font-mono text-cyan-400 truncate">{name}</span>
        </div>

        {/* Duration badge */}
        {duration_ms != null && (
          <span className="shrink-0 text-xs text-zinc-500 tabular-nums">
            {formatDuration(duration_ms)}
          </span>
        )}
      </button>

      {/* Expanded body */}
      {expanded && (
        <div className="border-t border-zinc-700 divide-y divide-zinc-700/60">
          {/* Args */}
          <div className="px-3 py-2.5">
            <p className="text-[11px] font-medium uppercase tracking-wider text-zinc-500 mb-1.5">
              Arguments
            </p>
            <pre className="text-xs text-zinc-300 bg-zinc-900/60 rounded p-2 overflow-x-auto whitespace-pre-wrap break-words">
              {prettyArgs || <span className="text-zinc-600 italic">none</span>}
            </pre>
          </div>

          {/* Result */}
          {prettyResult != null && (
            <div className="px-3 py-2.5">
              <p className="text-[11px] font-medium uppercase tracking-wider text-zinc-500 mb-1.5">
                Result
              </p>
              <pre className="text-xs text-zinc-300 bg-zinc-900/60 rounded p-2 overflow-x-auto whitespace-pre-wrap break-words">
                {prettyResult || <span className="text-zinc-600 italic">empty</span>}
              </pre>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
