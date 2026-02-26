// KanbanColumn — droppable column area using @dnd-kit/core useDroppable.
//
// Usage:
//   <KanbanColumn id="backlog" title="Backlog" tasks={tasks} color="zinc" />
//
// Highlights the drop zone when a card is dragged over it.

import { useDroppable } from '@dnd-kit/core'
import KanbanCard, { type KanbanTask } from './KanbanCard'

interface KanbanColumnProps {
  id: string
  title: string
  tasks: KanbanTask[]
  color: 'zinc' | 'blue' | 'amber' | 'emerald'
}

const COLOR_MAP: Record<KanbanColumnProps['color'], { badge: string; header: string; over: string }> = {
  zinc: {
    badge: 'bg-zinc-700/60 text-zinc-300',
    header: 'text-zinc-300',
    over: 'border-zinc-600 bg-zinc-800/40',
  },
  blue: {
    badge: 'bg-blue-500/20 text-blue-300',
    header: 'text-blue-300',
    over: 'border-blue-600/50 bg-blue-900/10',
  },
  amber: {
    badge: 'bg-amber-500/20 text-amber-300',
    header: 'text-amber-300',
    over: 'border-amber-600/50 bg-amber-900/10',
  },
  emerald: {
    badge: 'bg-emerald-500/20 text-emerald-300',
    header: 'text-emerald-300',
    over: 'border-emerald-600/50 bg-emerald-900/10',
  },
}

export default function KanbanColumn({ id, title, tasks, color }: KanbanColumnProps) {
  const { setNodeRef, isOver } = useDroppable({ id })
  const colors = COLOR_MAP[color]

  return (
    <div className="flex flex-col min-w-[280px] w-72 shrink-0">
      {/* Column header */}
      <div className="flex items-center gap-2 mb-3 px-1">
        <h2 className={`text-sm font-semibold ${colors.header}`}>{title}</h2>
        <span
          className={`inline-flex items-center justify-center min-w-[20px] h-5 px-1.5 rounded-full text-[10px] font-medium tabular-nums ${colors.badge}`}
        >
          {tasks.length}
        </span>
      </div>

      {/* Drop zone */}
      <div
        ref={setNodeRef}
        aria-label={`${title} column drop zone`}
        className={[
          'flex-1 min-h-[120px] rounded-lg border border-dashed transition-colors duration-150',
          'p-2 flex flex-col gap-2',
          isOver
            ? colors.over
            : 'border-zinc-800/60 bg-zinc-900/30',
        ].join(' ')}
      >
        {tasks.length === 0 && !isOver && (
          <div className="flex-1 flex items-center justify-center py-8">
            <span className="text-xs text-zinc-700">Drop tasks here</span>
          </div>
        )}

        {tasks.map((task) => (
          <KanbanCard key={task.id} task={task} />
        ))}
      </div>
    </div>
  )
}
