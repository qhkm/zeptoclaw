// Kanban — drag-and-drop task board with 4 columns.
//
// Architecture:
//   - DndContext wraps all 4 columns + DragOverlay
//   - Tasks fetched from GET /api/tasks via useQuery
//   - Column move via POST /api/tasks/{id}/move via useMutation
//   - Active drag renders a DragOverlay clone for smooth UX
//   - Filter bar narrows tasks by assignee and priority
//   - "Add Task" opens a modal with controlled form → POST /api/tasks

import { useState, useMemo } from 'react'
import {
  DndContext,
  DragOverlay,
  closestCenter,
  type DragStartEvent,
  type DragEndEvent,
} from '@dnd-kit/core'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiFetch } from '../lib/api'
import KanbanColumn from '../components/KanbanColumn'
import KanbanCard, { type KanbanTask } from '../components/KanbanCard'

// ---------------------------------------------------------------------------
// Column definitions
// ---------------------------------------------------------------------------

const COLUMNS: {
  id: string
  title: string
  color: 'zinc' | 'blue' | 'amber' | 'emerald'
}[] = [
  { id: 'backlog', title: 'Backlog', color: 'zinc' },
  { id: 'in_progress', title: 'In Progress', color: 'blue' },
  { id: 'review', title: 'Review', color: 'amber' },
  { id: 'done', title: 'Done', color: 'emerald' },
]

const PRIORITIES: KanbanTask['priority'][] = ['low', 'medium', 'high', 'critical']

// ---------------------------------------------------------------------------
// API helpers
// ---------------------------------------------------------------------------

async function fetchTasks(): Promise<KanbanTask[]> {
  return apiFetch<KanbanTask[]>('/api/tasks')
}

async function moveTask(id: string, column: string): Promise<KanbanTask> {
  return apiFetch<KanbanTask>(`/api/tasks/${id}/move`, {
    method: 'POST',
    body: JSON.stringify({ column }),
  })
}

async function createTask(data: Omit<KanbanTask, 'id' | 'created_at' | 'updated_at'>): Promise<KanbanTask> {
  return apiFetch<KanbanTask>('/api/tasks', {
    method: 'POST',
    body: JSON.stringify(data),
  })
}

// ---------------------------------------------------------------------------
// Create Task Modal
// ---------------------------------------------------------------------------

interface CreateTaskModalProps {
  onClose: () => void
  onCreated: () => void
}

function CreateTaskModal({ onClose, onCreated }: CreateTaskModalProps) {
  const queryClient = useQueryClient()
  const [title, setTitle] = useState('')
  const [description, setDescription] = useState('')
  const [column, setColumn] = useState('backlog')
  const [assignee, setAssignee] = useState<'human' | 'agent'>('human')
  const [priority, setPriority] = useState<KanbanTask['priority']>('medium')
  const [labelsRaw, setLabelsRaw] = useState('')

  const createMutation = useMutation({
    mutationFn: (data: Omit<KanbanTask, 'id' | 'created_at' | 'updated_at'>) => createTask(data),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['tasks'] })
      onCreated()
    },
  })

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (!title.trim()) return
    const labels = labelsRaw
      .split(',')
      .map((l) => l.trim())
      .filter(Boolean)
    createMutation.mutate({ title: title.trim(), description, column, assignee, priority, labels })
  }

  return (
    /* Overlay */
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm"
      role="dialog"
      aria-modal="true"
      aria-label="Create new task"
      onClick={(e) => { if (e.target === e.currentTarget) onClose() }}
    >
      {/* Modal card */}
      <div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6 w-full max-w-md mx-4 shadow-xl">
        <div className="flex items-center justify-between mb-5">
          <h2 className="text-base font-semibold text-zinc-100">New Task</h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close modal"
            className="text-zinc-500 hover:text-zinc-300 transition-colors"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
              <path d="M2 2l12 12M14 2L2 14" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            </svg>
          </button>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          {/* Title */}
          <div>
            <label htmlFor="task-title" className="block text-xs font-medium text-zinc-400 mb-1">
              Title <span aria-hidden="true" className="text-zinc-600">*</span>
            </label>
            <input
              id="task-title"
              type="text"
              required
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="Short task title"
              className="w-full bg-zinc-800 border border-zinc-700 rounded-md px-3 py-2 text-sm text-zinc-100 placeholder-zinc-600 focus:outline-none focus:border-zinc-500 transition-colors"
            />
          </div>

          {/* Description */}
          <div>
            <label htmlFor="task-desc" className="block text-xs font-medium text-zinc-400 mb-1">
              Description
            </label>
            <textarea
              id="task-desc"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Optional details…"
              rows={3}
              className="w-full bg-zinc-800 border border-zinc-700 rounded-md px-3 py-2 text-sm text-zinc-100 placeholder-zinc-600 focus:outline-none focus:border-zinc-500 transition-colors resize-none"
            />
          </div>

          {/* Column + Assignee row */}
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label htmlFor="task-column" className="block text-xs font-medium text-zinc-400 mb-1">
                Column
              </label>
              <select
                id="task-column"
                value={column}
                onChange={(e) => setColumn(e.target.value)}
                className="w-full bg-zinc-800 border border-zinc-700 rounded-md px-3 py-2 text-sm text-zinc-100 focus:outline-none focus:border-zinc-500 transition-colors"
              >
                {COLUMNS.map((col) => (
                  <option key={col.id} value={col.id}>
                    {col.title}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label htmlFor="task-assignee" className="block text-xs font-medium text-zinc-400 mb-1">
                Assignee
              </label>
              <select
                id="task-assignee"
                value={assignee}
                onChange={(e) => setAssignee(e.target.value as 'human' | 'agent')}
                className="w-full bg-zinc-800 border border-zinc-700 rounded-md px-3 py-2 text-sm text-zinc-100 focus:outline-none focus:border-zinc-500 transition-colors"
              >
                <option value="human">Human</option>
                <option value="agent">Agent</option>
              </select>
            </div>
          </div>

          {/* Priority */}
          <div>
            <label htmlFor="task-priority" className="block text-xs font-medium text-zinc-400 mb-1">
              Priority
            </label>
            <select
              id="task-priority"
              value={priority}
              onChange={(e) => setPriority(e.target.value as KanbanTask['priority'])}
              className="w-full bg-zinc-800 border border-zinc-700 rounded-md px-3 py-2 text-sm text-zinc-100 focus:outline-none focus:border-zinc-500 transition-colors"
            >
              {PRIORITIES.map((p) => (
                <option key={p} value={p}>
                  {p.charAt(0).toUpperCase() + p.slice(1)}
                </option>
              ))}
            </select>
          </div>

          {/* Labels */}
          <div>
            <label htmlFor="task-labels" className="block text-xs font-medium text-zinc-400 mb-1">
              Labels <span className="text-zinc-600 font-normal">(comma-separated)</span>
            </label>
            <input
              id="task-labels"
              type="text"
              value={labelsRaw}
              onChange={(e) => setLabelsRaw(e.target.value)}
              placeholder="frontend, urgent, v2"
              className="w-full bg-zinc-800 border border-zinc-700 rounded-md px-3 py-2 text-sm text-zinc-100 placeholder-zinc-600 focus:outline-none focus:border-zinc-500 transition-colors"
            />
          </div>

          {/* Error */}
          {createMutation.isError && (
            <p className="text-xs text-red-400" role="alert">
              {createMutation.error instanceof Error
                ? createMutation.error.message
                : 'Failed to create task.'}
            </p>
          )}

          {/* Actions */}
          <div className="flex items-center justify-end gap-3 pt-1">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 text-sm text-zinc-400 hover:text-zinc-200 transition-colors"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={createMutation.isPending || !title.trim()}
              className="px-4 py-2 text-sm font-medium bg-zinc-100 text-zinc-900 rounded-md hover:bg-white disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            >
              {createMutation.isPending ? 'Creating…' : 'Create Task'}
            </button>
          </div>
        </form>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Kanban page
// ---------------------------------------------------------------------------

export default function Kanban() {
  const queryClient = useQueryClient()

  // Drag state
  const [activeTask, setActiveTask] = useState<KanbanTask | null>(null)

  // Filter state
  const [assigneeFilter, setAssigneeFilter] = useState<'all' | 'human' | 'agent'>('all')
  const [priorityFilter, setPriorityFilter] = useState<KanbanTask['priority'] | 'all'>('all')

  // Modal state
  const [showModal, setShowModal] = useState(false)

  // Fetch tasks
  const { data: tasks = [], isLoading, isError } = useQuery<KanbanTask[]>({
    queryKey: ['tasks'],
    queryFn: fetchTasks,
    // Optimistic local data on 404 (API not yet wired) — surface empty board rather than error
    retry: 1,
  })

  // Move task mutation — optimistic update via queryClient.setQueryData
  const moveMutation = useMutation({
    mutationFn: ({ id, column }: { id: string; column: string }) => moveTask(id, column),
    onMutate: async ({ id, column }) => {
      await queryClient.cancelQueries({ queryKey: ['tasks'] })
      const prev = queryClient.getQueryData<KanbanTask[]>(['tasks'])
      queryClient.setQueryData<KanbanTask[]>(['tasks'], (old = []) =>
        old.map((t) => (t.id === id ? { ...t, column } : t)),
      )
      return { prev }
    },
    onError: (_err, _vars, ctx) => {
      if (ctx?.prev) queryClient.setQueryData(['tasks'], ctx.prev)
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ['tasks'] })
    },
  })

  // Filtered tasks
  const filteredTasks = useMemo(() => {
    return tasks.filter((t) => {
      if (assigneeFilter !== 'all' && t.assignee !== assigneeFilter) return false
      if (priorityFilter !== 'all' && t.priority !== priorityFilter) return false
      return true
    })
  }, [tasks, assigneeFilter, priorityFilter])

  // Tasks grouped by column
  const tasksByColumn = useMemo(() => {
    const map: Record<string, KanbanTask[]> = {}
    for (const col of COLUMNS) map[col.id] = []
    for (const task of filteredTasks) {
      if (map[task.column]) {
        map[task.column].push(task)
      } else {
        // Unknown column — fall into backlog
        map['backlog'].push(task)
      }
    }
    return map
  }, [filteredTasks])

  // Drag handlers
  function handleDragStart(event: DragStartEvent) {
    const task = tasks.find((t) => t.id === String(event.active.id))
    setActiveTask(task ?? null)
  }

  function handleDragEnd(event: DragEndEvent) {
    const { active, over } = event
    setActiveTask(null)
    if (!over) return
    const taskId = String(active.id)
    const targetColumn = String(over.id)
    const task = tasks.find((t) => t.id === taskId)
    if (!task || task.column === targetColumn) return
    moveMutation.mutate({ id: taskId, column: targetColumn })
  }

  return (
    <div className="flex flex-col h-full">
      {/* Page header */}
      <div className="flex items-center justify-between mb-4">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100 mb-0.5">Kanban</h1>
          <p className="text-sm text-zinc-400">
            Drag tasks between columns to update their status.
          </p>
        </div>
        <button
          type="button"
          onClick={() => setShowModal(true)}
          className="inline-flex items-center gap-1.5 px-4 py-2 text-sm font-medium bg-zinc-100 text-zinc-900 rounded-md hover:bg-white transition-colors"
        >
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
            <path d="M7 1v12M1 7h12" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
          </svg>
          Add Task
        </button>
      </div>

      {/* Filter bar */}
      <div className="flex items-center gap-3 mb-5 flex-wrap">
        {/* Assignee filter */}
        <div className="flex items-center gap-1 bg-zinc-900 border border-zinc-800 rounded-md p-1">
          {(['all', 'human', 'agent'] as const).map((v) => (
            <button
              key={v}
              type="button"
              onClick={() => setAssigneeFilter(v)}
              aria-pressed={assigneeFilter === v}
              className={[
                'px-3 py-1 rounded text-xs font-medium transition-colors',
                assigneeFilter === v
                  ? 'bg-zinc-700 text-zinc-100'
                  : 'text-zinc-500 hover:text-zinc-300',
              ].join(' ')}
            >
              {v === 'all' ? 'All' : v.charAt(0).toUpperCase() + v.slice(1)}
            </button>
          ))}
        </div>

        {/* Priority filter */}
        <div className="flex items-center gap-1 bg-zinc-900 border border-zinc-800 rounded-md p-1">
          {(['all', ...PRIORITIES] as const).map((v) => (
            <button
              key={v}
              type="button"
              onClick={() => setPriorityFilter(v)}
              aria-pressed={priorityFilter === v}
              className={[
                'px-3 py-1 rounded text-xs font-medium transition-colors capitalize',
                priorityFilter === v
                  ? 'bg-zinc-700 text-zinc-100'
                  : 'text-zinc-500 hover:text-zinc-300',
              ].join(' ')}
            >
              {v === 'all' ? 'Any Priority' : v}
            </button>
          ))}
        </div>

        {/* Task count */}
        <span className="text-xs text-zinc-600 ml-auto">
          {filteredTasks.length} task{filteredTasks.length !== 1 ? 's' : ''}
        </span>
      </div>

      {/* Loading / error states */}
      {isLoading && (
        <div className="flex-1 flex items-center justify-center text-zinc-600 text-sm">
          Loading tasks…
        </div>
      )}
      {isError && (
        <div className="flex-1 flex items-center justify-center">
          <p className="text-sm text-red-400">
            Could not load tasks. Make sure the API is reachable.
          </p>
        </div>
      )}

      {/* Board */}
      {!isLoading && (
        <DndContext
          collisionDetection={closestCenter}
          onDragStart={handleDragStart}
          onDragEnd={handleDragEnd}
        >
          <div
            className="flex gap-4 overflow-x-auto pb-4 flex-1"
            role="region"
            aria-label="Kanban board"
          >
            {COLUMNS.map((col) => (
              <KanbanColumn
                key={col.id}
                id={col.id}
                title={col.title}
                tasks={tasksByColumn[col.id] ?? []}
                color={col.color}
              />
            ))}
          </div>

          {/* Drag overlay — renders the card clone while dragging */}
          <DragOverlay dropAnimation={null}>
            {activeTask ? (
              <div className="rotate-1 scale-105 pointer-events-none">
                <KanbanCard task={activeTask} />
              </div>
            ) : null}
          </DragOverlay>
        </DndContext>
      )}

      {/* Create Task Modal */}
      {showModal && (
        <CreateTaskModal
          onClose={() => setShowModal(false)}
          onCreated={() => setShowModal(false)}
        />
      )}
    </div>
  )
}
