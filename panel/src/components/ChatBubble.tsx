// ChatBubble — renders a single chat message with role alignment, styling,
// and an optional timestamp corner label.
//
// Usage:
//   <ChatBubble role="user" content="Hello!" timestamp="2024-01-15T10:30:00Z" />
//   <ChatBubble role="assistant" content="Hi there!" />

interface ChatBubbleProps {
  role: 'user' | 'assistant'
  content: string
  timestamp?: string
}

function formatTimestamp(iso: string): string {
  try {
    return new Date(iso).toLocaleTimeString([], {
      hour: '2-digit',
      minute: '2-digit',
    })
  } catch {
    return iso
  }
}

export default function ChatBubble({ role, content, timestamp }: ChatBubbleProps) {
  const isUser = role === 'user'

  return (
    <div className={`flex flex-col gap-1 ${isUser ? 'items-end' : 'items-start'}`}>
      {/* Role label */}
      <span className="text-[11px] font-medium uppercase tracking-wider text-zinc-500 px-1">
        {isUser ? 'You' : 'Assistant'}
      </span>

      {/* Bubble */}
      <div
        className={`relative max-w-[85%] rounded-lg border px-3 py-2.5 text-sm leading-relaxed ${
          isUser
            ? 'bg-violet-600/20 border-violet-500/30 text-violet-100'
            : 'bg-zinc-800 border-zinc-700 text-zinc-200'
        }`}
      >
        {/* Content — preserve whitespace for multi-line messages */}
        <p className="whitespace-pre-wrap break-words">{content}</p>

        {/* Timestamp — bottom corner */}
        {timestamp && (
          <span
            className={`block mt-1.5 text-[10px] tabular-nums ${
              isUser ? 'text-violet-400/60 text-right' : 'text-zinc-600'
            }`}
          >
            {formatTimestamp(timestamp)}
          </span>
        )}
      </div>
    </div>
  )
}
