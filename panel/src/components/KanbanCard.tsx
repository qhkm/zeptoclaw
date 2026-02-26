// KanbanCard — draggable task card using @dnd-kit/core useDraggable.
//
// Usage:
//   <KanbanCard task={task} />
//
// Priority dot colors map to low/medium/high/critical severity.
// Assignee badge distinguishes human vs agent tasks visually.

import { useDraggable } from '@dnd-kit/core'

export interface KanbanTask {
  id: string
  title: string
  description: string
  column: string
  assignee: string // "human" | "agent"
  priority: 'low' | 'medium' | 'high' | 'critical'
  labels: string[]
  created_at: string
  updated_at: string
}

const PRIORITY_COLORS: Record<KanbanTask['priority'], string> = {
  low: 'bg-zinc-500',
  medium: 'bg-blue-500',
  high: 'bg-amber-500',
  critical: 'bg-red-500',
}

const PRIORITY_BORDER: Record<KanbanTask['priority'], string> = {
  low: 'border-l-zinc-600',
  medium: 'border-l-blue-500',
  high: 'border-l-amber-500',
  critical: 'border-l-red-500',
}

export default function KanbanCard({ task }: { task: KanbanTask }) {
  const { attributes, listeners, setNodeRef, transform, isDragging } = useDraggable({
    id: task.id,
  })

  const style = transform
    ? { transform: `translate(${transform.x}px, ${transform.y}px)` }
    : undefined

  const descSnippet =
    task.description.length > 80
      ? task.description.slice(0, 80) + '…'
      : task.description

  return (
    <div
      ref={setNodeRef}
      style={style}
      {...listeners}
      {...attributes}
      className={[
        'bg-zinc-900 rounded-lg border border-zinc-800 border-l-2 p-3 cursor-grab active:cursor-grabbing',
        'hover:border-zinc-700 hover:shadow-lg hover:shadow-black/30',
        'transition-all duration-150',
        PRIORITY_BORDER[task.priority],
        isDragging ? 'opacity-40 shadow-xl scale-[1.02]' : 'opacity-100',
      ].join(' ')}
    >
      {/* Title row */}
      <div className="flex items-start gap-2 mb-1.5">
        {/* Priority dot */}
        <span
          className={`mt-1 shrink-0 w-1.5 h-1.5 rounded-full ${PRIORITY_COLORS[task.priority]}`}
          aria-label={`Priority: ${task.priority}`}
          title={`Priority: ${task.priority}`}
        />
        <span className="text-sm font-medium text-zinc-100 leading-snug flex-1 min-w-0">
          {task.title}
        </span>
      </div>

      {/* Description snippet */}
      {descSnippet && (
        <p className="text-xs text-zinc-500 leading-relaxed mb-2 ml-3.5">{descSnippet}</p>
      )}

      {/* Footer: assignee + labels */}
      <div className="flex items-center gap-1.5 flex-wrap ml-3.5">
        {/* Assignee badge */}
        <span
          className={`inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium ${
            task.assignee === 'agent'
              ? 'bg-violet-500/20 text-violet-300'
              : 'bg-zinc-700 text-zinc-300'
          }`}
        >
          {task.assignee === 'agent' ? 'Agent' : 'Human'}
        </span>

        {/* Labels */}
        {task.labels.slice(0, 3).map((label, i) => (
          <span
            key={`${i}-${label}`}
            className="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-zinc-800 text-zinc-400"
          >
            {label}
          </span>
        ))}
        {task.labels.length > 3 && (
          <span className="text-[10px] text-zinc-600">+{task.labels.length - 3}</span>
        )}
      </div>
    </div>
  )
}
