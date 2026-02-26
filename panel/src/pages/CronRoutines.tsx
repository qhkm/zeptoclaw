// CronRoutines — tabbed page for cron job and routine management.
//
// Tabs:
//   1. Cron Jobs  — list, trigger, create, edit, delete scheduled cron jobs
//   2. Routines   — list, toggle, create, edit, delete event-driven routines
//
// Usage:
//   <Route path="/cron" element={<CronRoutines />} />

import { useState } from 'react'
import {
  useQuery,
  useMutation,
  useQueryClient,
} from '@tanstack/react-query'
import { apiFetch } from '../lib/api'

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface CronJob {
  id: string
  name: string
  expression: string
  action: string
  enabled: boolean
  last_run?: string
  next_run?: string
}

interface Routine {
  id: string
  name: string
  description: string
  trigger_type: 'cron' | 'event' | 'webhook' | 'manual'
  trigger_value: string
  enabled: boolean
  last_triggered?: string
}

type ActiveTab = 'cron' | 'routines'

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

function formatDateTime(iso?: string): string {
  if (!iso) return '—'
  try {
    return new Date(iso).toLocaleString([], {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    })
  } catch {
    return iso
  }
}

// ---------------------------------------------------------------------------
// Toggle switch
// ---------------------------------------------------------------------------

function Toggle({
  enabled,
  onChange,
  disabled,
}: {
  enabled: boolean
  onChange: (next: boolean) => void
  disabled?: boolean
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={enabled}
      disabled={disabled}
      onClick={() => onChange(!enabled)}
      className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-violet-500 disabled:opacity-40 disabled:cursor-not-allowed ${
        enabled ? 'bg-violet-600' : 'bg-zinc-700'
      }`}
    >
      <span
        className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform ${
          enabled ? 'translate-x-[18px]' : 'translate-x-[3px]'
        }`}
      />
    </button>
  )
}

// ---------------------------------------------------------------------------
// Trigger type badge
// ---------------------------------------------------------------------------

const TRIGGER_BADGE: Record<Routine['trigger_type'], string> = {
  cron:    'bg-orange-500/20 text-orange-300',
  event:   'bg-blue-500/20 text-blue-300',
  webhook: 'bg-emerald-500/20 text-emerald-300',
  manual:  'bg-zinc-500/20 text-zinc-300',
}

function TriggerBadge({ type }: { type: Routine['trigger_type'] }) {
  return (
    <span
      className={`inline-flex items-center px-2 py-0.5 rounded text-[11px] font-medium capitalize ${TRIGGER_BADGE[type]}`}
    >
      {type}
    </span>
  )
}

// ---------------------------------------------------------------------------
// Modal wrapper
// ---------------------------------------------------------------------------

function Modal({
  title,
  onClose,
  children,
}: {
  title: string
  onClose: () => void
  children: React.ReactNode
}) {
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 px-4"
      onClick={(e) => { if (e.target === e.currentTarget) onClose() }}
    >
      <div
        className="w-full max-w-md bg-zinc-900 border border-zinc-800 rounded-lg p-6 space-y-4 shadow-xl"
        role="dialog"
        aria-modal="true"
        aria-labelledby="modal-title"
      >
        <div className="flex items-center justify-between">
          <h2 id="modal-title" className="text-base font-semibold text-zinc-100">
            {title}
          </h2>
          <button
            type="button"
            onClick={onClose}
            className="text-zinc-500 hover:text-zinc-300 transition-colors"
            aria-label="Close modal"
          >
            <svg className="w-5 h-5" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
              <path d="M6.28 5.22a.75.75 0 0 0-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 1 0 1.06 1.06L10 11.06l3.72 3.72a.75.75 0 1 0 1.06-1.06L11.06 10l3.72-3.72a.75.75 0 0 0-1.06-1.06L10 8.94 6.28 5.22Z" />
            </svg>
          </button>
        </div>
        {children}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Form field helper
// ---------------------------------------------------------------------------

function Field({
  label,
  children,
}: {
  label: string
  children: React.ReactNode
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <label className="text-xs font-medium text-zinc-400 uppercase tracking-wider">
        {label}
      </label>
      {children}
    </div>
  )
}

const inputCls =
  'w-full rounded-md bg-zinc-800 border border-zinc-700 px-3 py-2 text-sm text-zinc-100 placeholder-zinc-600 focus:outline-none focus:ring-2 focus:ring-violet-500 focus:border-transparent transition'

// ---------------------------------------------------------------------------
// Cron Job Modal
// ---------------------------------------------------------------------------

interface CronFormState {
  name: string
  expression: string
  action: string
  enabled: boolean
}

function CronModal({
  initial,
  onClose,
  onSave,
  isSaving,
}: {
  initial?: Partial<CronFormState>
  onClose: () => void
  onSave: (data: CronFormState) => void
  isSaving: boolean
}) {
  const [form, setForm] = useState<CronFormState>({
    name: initial?.name ?? '',
    expression: initial?.expression ?? '',
    action: initial?.action ?? '',
    enabled: initial?.enabled ?? true,
  })

  function set<K extends keyof CronFormState>(key: K, value: CronFormState[K]) {
    setForm((prev) => ({ ...prev, [key]: value }))
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    onSave(form)
  }

  const isEdit = initial?.name !== undefined

  return (
    <Modal title={isEdit ? 'Edit Cron Job' : 'Add Cron Job'} onClose={onClose}>
      <form onSubmit={handleSubmit} className="space-y-4">
        <Field label="Name">
          <input
            className={inputCls}
            value={form.name}
            onChange={(e) => set('name', e.target.value)}
            placeholder="e.g. daily-summary"
            required
            autoFocus
          />
        </Field>
        <Field label="Cron Expression">
          <input
            className={inputCls}
            value={form.expression}
            onChange={(e) => set('expression', e.target.value)}
            placeholder="0 9 * * *"
            required
          />
          <span className="text-[11px] text-zinc-600">
            Standard cron syntax — minute hour day month weekday
          </span>
        </Field>
        <Field label="Action / Prompt">
          <textarea
            className={`${inputCls} resize-none min-h-[80px]`}
            value={form.action}
            onChange={(e) => set('action', e.target.value)}
            placeholder="Summarize the day's activity and send to Telegram"
            required
          />
        </Field>
        <Field label="Enabled">
          <div className="flex items-center gap-2">
            <Toggle enabled={form.enabled} onChange={(v) => set('enabled', v)} />
            <span className="text-sm text-zinc-400">{form.enabled ? 'Active' : 'Paused'}</span>
          </div>
        </Field>
        <div className="flex justify-end gap-2 pt-2">
          <button
            type="button"
            onClick={onClose}
            className="px-4 py-2 text-sm text-zinc-400 hover:text-zinc-200 transition-colors"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={isSaving}
            className="px-4 py-2 text-sm font-medium bg-violet-600 hover:bg-violet-500 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded-md transition-colors"
          >
            {isSaving ? 'Saving…' : 'Save'}
          </button>
        </div>
      </form>
    </Modal>
  )
}

// ---------------------------------------------------------------------------
// Routine Modal
// ---------------------------------------------------------------------------

interface RoutineFormState {
  name: string
  description: string
  trigger_type: Routine['trigger_type']
  trigger_value: string
  enabled: boolean
}

function RoutineModal({
  initial,
  onClose,
  onSave,
  isSaving,
}: {
  initial?: Partial<RoutineFormState>
  onClose: () => void
  onSave: (data: RoutineFormState) => void
  isSaving: boolean
}) {
  const [form, setForm] = useState<RoutineFormState>({
    name: initial?.name ?? '',
    description: initial?.description ?? '',
    trigger_type: initial?.trigger_type ?? 'manual',
    trigger_value: initial?.trigger_value ?? '',
    enabled: initial?.enabled ?? true,
  })

  function set<K extends keyof RoutineFormState>(key: K, value: RoutineFormState[K]) {
    setForm((prev) => ({ ...prev, [key]: value }))
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    onSave(form)
  }

  const isEdit = initial?.name !== undefined

  return (
    <Modal title={isEdit ? 'Edit Routine' : 'Add Routine'} onClose={onClose}>
      <form onSubmit={handleSubmit} className="space-y-4">
        <Field label="Name">
          <input
            className={inputCls}
            value={form.name}
            onChange={(e) => set('name', e.target.value)}
            placeholder="e.g. on-new-order"
            required
            autoFocus
          />
        </Field>
        <Field label="Description">
          <textarea
            className={`${inputCls} resize-none min-h-[60px]`}
            value={form.description}
            onChange={(e) => set('description', e.target.value)}
            placeholder="What does this routine do?"
          />
        </Field>
        <Field label="Trigger Type">
          <select
            className={inputCls}
            value={form.trigger_type}
            onChange={(e) => set('trigger_type', e.target.value as Routine['trigger_type'])}
          >
            <option value="cron">Cron</option>
            <option value="event">Event</option>
            <option value="webhook">Webhook</option>
            <option value="manual">Manual</option>
          </select>
        </Field>
        <Field label="Trigger Value">
          <input
            className={inputCls}
            value={form.trigger_value}
            onChange={(e) => set('trigger_value', e.target.value)}
            placeholder={
              form.trigger_type === 'cron'
                ? '0 * * * *'
                : form.trigger_type === 'event'
                ? 'message.received'
                : form.trigger_type === 'webhook'
                ? '/hooks/my-routine'
                : '(leave blank for manual)'
            }
          />
        </Field>
        <Field label="Enabled">
          <div className="flex items-center gap-2">
            <Toggle enabled={form.enabled} onChange={(v) => set('enabled', v)} />
            <span className="text-sm text-zinc-400">{form.enabled ? 'Active' : 'Paused'}</span>
          </div>
        </Field>
        <div className="flex justify-end gap-2 pt-2">
          <button
            type="button"
            onClick={onClose}
            className="px-4 py-2 text-sm text-zinc-400 hover:text-zinc-200 transition-colors"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={isSaving}
            className="px-4 py-2 text-sm font-medium bg-violet-600 hover:bg-violet-500 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded-md transition-colors"
          >
            {isSaving ? 'Saving…' : 'Save'}
          </button>
        </div>
      </form>
    </Modal>
  )
}

// ---------------------------------------------------------------------------
// Cron Tab
// ---------------------------------------------------------------------------

function CronTab() {
  const qc = useQueryClient()
  const [showCreate, setShowCreate] = useState(false)
  const [editJob, setEditJob] = useState<CronJob | null>(null)

  const { data: jobs = [], isLoading, isError } = useQuery<CronJob[]>({
    queryKey: ['cron'],
    queryFn: () => apiFetch<CronJob[]>('/api/cron'),
  })

  const createMut = useMutation({
    mutationFn: (body: Omit<CronJob, 'id' | 'last_run' | 'next_run'>) =>
      apiFetch<CronJob>('/api/cron', {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['cron'] })
      setShowCreate(false)
    },
  })

  const updateMut = useMutation({
    mutationFn: ({ id, ...body }: Partial<CronJob> & { id: string }) =>
      apiFetch<CronJob>(`/api/cron/${id}`, {
        method: 'PUT',
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['cron'] })
      setEditJob(null)
    },
  })

  const deleteMut = useMutation({
    mutationFn: (id: string) =>
      apiFetch<void>(`/api/cron/${id}`, { method: 'DELETE' }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['cron'] }),
  })

  const triggerMut = useMutation({
    mutationFn: (id: string) =>
      apiFetch<void>(`/api/cron/${id}/trigger`, { method: 'POST' }),
  })

  return (
    <div className="space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <p className="text-sm text-zinc-400">
          {isLoading ? 'Loading…' : `${jobs.length} job${jobs.length !== 1 ? 's' : ''}`}
        </p>
        <button
          type="button"
          onClick={() => setShowCreate(true)}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium bg-violet-600 hover:bg-violet-500 text-white rounded-md transition-colors"
        >
          <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
            <path d="M8.75 3.75a.75.75 0 0 0-1.5 0v3.5h-3.5a.75.75 0 0 0 0 1.5h3.5v3.5a.75.75 0 0 0 1.5 0v-3.5h3.5a.75.75 0 0 0 0-1.5h-3.5v-3.5Z" />
          </svg>
          Add Cron Job
        </button>
      </div>

      {/* Error state */}
      {isError && (
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4 text-sm text-red-400">
          Failed to load cron jobs.
        </div>
      )}

      {/* Empty state */}
      {!isLoading && !isError && jobs.length === 0 && (
        <div className="bg-zinc-900 rounded-lg border border-zinc-800 p-8 text-center">
          <p className="text-zinc-500 text-sm">No cron jobs yet.</p>
          <button
            type="button"
            onClick={() => setShowCreate(true)}
            className="mt-3 text-sm text-violet-400 hover:text-violet-300 transition-colors"
          >
            Add your first cron job
          </button>
        </div>
      )}

      {/* Job list */}
      {jobs.length > 0 && (
        <div className="bg-zinc-900 rounded-lg border border-zinc-800 overflow-hidden">
          <table className="w-full text-left">
            <thead>
              <tr className="border-b border-zinc-800">
                <th className="px-4 py-3 text-xs font-medium text-zinc-500 uppercase tracking-wider">
                  Name / Expression
                </th>
                <th className="px-4 py-3 text-xs font-medium text-zinc-500 uppercase tracking-wider hidden sm:table-cell">
                  Last Run
                </th>
                <th className="px-4 py-3 text-xs font-medium text-zinc-500 uppercase tracking-wider hidden md:table-cell">
                  Next Run
                </th>
                <th className="px-4 py-3 text-xs font-medium text-zinc-500 uppercase tracking-wider">
                  Status
                </th>
                <th className="px-4 py-3 text-xs font-medium text-zinc-500 uppercase tracking-wider text-right">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody>
              {jobs.map((job) => (
                <tr
                  key={job.id}
                  className="border-b border-zinc-800/60 last:border-0 hover:bg-zinc-800/30 transition-colors"
                >
                  <td className="px-4 py-3">
                    <div className="text-sm font-medium text-zinc-200">{job.name}</div>
                    <div className="text-xs font-mono text-zinc-500 mt-0.5">{job.expression}</div>
                  </td>
                  <td className="px-4 py-3 text-sm text-zinc-400 hidden sm:table-cell tabular-nums">
                    {formatDateTime(job.last_run)}
                  </td>
                  <td className="px-4 py-3 text-sm text-zinc-400 hidden md:table-cell tabular-nums">
                    {formatDateTime(job.next_run)}
                  </td>
                  <td className="px-4 py-3">
                    <span
                      className={`inline-flex items-center gap-1.5 text-xs font-medium px-2 py-0.5 rounded ${
                        job.enabled
                          ? 'bg-emerald-500/15 text-emerald-400'
                          : 'bg-zinc-700/40 text-zinc-500'
                      }`}
                    >
                      <span
                        className={`w-1.5 h-1.5 rounded-full ${
                          job.enabled ? 'bg-emerald-400' : 'bg-zinc-600'
                        }`}
                      />
                      {job.enabled ? 'Active' : 'Paused'}
                    </span>
                  </td>
                  <td className="px-4 py-3">
                    <div className="flex items-center justify-end gap-1">
                      <button
                        type="button"
                        title="Trigger now"
                        onClick={() => triggerMut.mutate(job.id)}
                        disabled={triggerMut.isPending}
                        className="p-1.5 text-zinc-500 hover:text-emerald-400 disabled:opacity-40 transition-colors rounded"
                        aria-label={`Trigger ${job.name} now`}
                      >
                        <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                          <path d="M3 2.75A.75.75 0 0 1 4.15 2.1l9 5.25a.75.75 0 0 1 0 1.3l-9 5.25A.75.75 0 0 1 3 13.25v-10.5Z" />
                        </svg>
                      </button>
                      <button
                        type="button"
                        title="Edit"
                        onClick={() => setEditJob(job)}
                        className="p-1.5 text-zinc-500 hover:text-zinc-200 transition-colors rounded"
                        aria-label={`Edit ${job.name}`}
                      >
                        <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                          <path d="M11.013 2.513a1.75 1.75 0 0 1 2.475 2.474L6.226 12.25a2.751 2.751 0 0 1-.892.596l-2.047.848a.75.75 0 0 1-.98-.98l.848-2.047a2.75 2.75 0 0 1 .596-.892l7.262-7.262Z" />
                        </svg>
                      </button>
                      <button
                        type="button"
                        title="Delete"
                        onClick={() => {
                          if (confirm(`Delete cron job "${job.name}"?`)) {
                            deleteMut.mutate(job.id)
                          }
                        }}
                        disabled={deleteMut.isPending}
                        className="p-1.5 text-zinc-500 hover:text-red-400 disabled:opacity-40 transition-colors rounded"
                        aria-label={`Delete ${job.name}`}
                      >
                        <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                          <path d="M5 3.25V4H2.75a.75.75 0 0 0 0 1.5h.3l.815 8.15A1.5 1.5 0 0 0 5.357 15h5.285a1.5 1.5 0 0 0 1.493-1.35l.815-8.15h.3a.75.75 0 0 0 0-1.5H11v-.75A2.25 2.25 0 0 0 8.75 1h-1.5A2.25 2.25 0 0 0 5 3.25Zm2.25-.75a.75.75 0 0 0-.75.75V4h3v-.75a.75.75 0 0 0-.75-.75h-1.5ZM6.05 6a.75.75 0 0 1 .787.713l.275 5.5a.75.75 0 0 1-1.498.075l-.275-5.5A.75.75 0 0 1 6.05 6Zm3.9 0a.75.75 0 0 1 .712.787l-.275 5.5a.75.75 0 0 1-1.498-.075l.275-5.5A.75.75 0 0 1 9.95 6Z" />
                        </svg>
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Create modal */}
      {showCreate && (
        <CronModal
          onClose={() => setShowCreate(false)}
          onSave={(data) => createMut.mutate(data)}
          isSaving={createMut.isPending}
        />
      )}

      {/* Edit modal */}
      {editJob && (
        <CronModal
          initial={editJob}
          onClose={() => setEditJob(null)}
          onSave={(data) => updateMut.mutate({ id: editJob.id, ...data })}
          isSaving={updateMut.isPending}
        />
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Routines Tab
// ---------------------------------------------------------------------------

function RoutinesTab() {
  const qc = useQueryClient()
  const [showCreate, setShowCreate] = useState(false)
  const [editRoutine, setEditRoutine] = useState<Routine | null>(null)

  const { data: routines = [], isLoading, isError } = useQuery<Routine[]>({
    queryKey: ['routines'],
    queryFn: () => apiFetch<Routine[]>('/api/routines'),
  })

  const createMut = useMutation({
    mutationFn: (body: Omit<Routine, 'id' | 'last_triggered'>) =>
      apiFetch<Routine>('/api/routines', {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['routines'] })
      setShowCreate(false)
    },
  })

  const updateMut = useMutation({
    mutationFn: ({ id, ...body }: Partial<Routine> & { id: string }) =>
      apiFetch<Routine>(`/api/routines/${id}`, {
        method: 'PUT',
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['routines'] })
      setEditRoutine(null)
    },
  })

  const toggleMut = useMutation({
    mutationFn: (id: string) =>
      apiFetch<Routine>(`/api/routines/${id}/toggle`, { method: 'POST' }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['routines'] }),
  })

  const deleteMut = useMutation({
    mutationFn: (id: string) =>
      apiFetch<void>(`/api/routines/${id}`, { method: 'DELETE' }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['routines'] }),
  })

  return (
    <div className="space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <p className="text-sm text-zinc-400">
          {isLoading ? 'Loading…' : `${routines.length} routine${routines.length !== 1 ? 's' : ''}`}
        </p>
        <button
          type="button"
          onClick={() => setShowCreate(true)}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium bg-violet-600 hover:bg-violet-500 text-white rounded-md transition-colors"
        >
          <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
            <path d="M8.75 3.75a.75.75 0 0 0-1.5 0v3.5h-3.5a.75.75 0 0 0 0 1.5h3.5v3.5a.75.75 0 0 0 1.5 0v-3.5h3.5a.75.75 0 0 0 0-1.5h-3.5v-3.5Z" />
          </svg>
          Add Routine
        </button>
      </div>

      {/* Error state */}
      {isError && (
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4 text-sm text-red-400">
          Failed to load routines.
        </div>
      )}

      {/* Empty state */}
      {!isLoading && !isError && routines.length === 0 && (
        <div className="bg-zinc-900 rounded-lg border border-zinc-800 p-8 text-center">
          <p className="text-zinc-500 text-sm">No routines yet.</p>
          <button
            type="button"
            onClick={() => setShowCreate(true)}
            className="mt-3 text-sm text-violet-400 hover:text-violet-300 transition-colors"
          >
            Add your first routine
          </button>
        </div>
      )}

      {/* Routines card list */}
      {routines.length > 0 && (
        <div className="space-y-3">
          {routines.map((routine) => (
            <div
              key={routine.id}
              className="bg-zinc-900 rounded-lg border border-zinc-800 p-4 flex flex-col sm:flex-row sm:items-center gap-3"
            >
              {/* Left: name + badge + meta */}
              <div className="flex-1 min-w-0 space-y-1">
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="text-sm font-medium text-zinc-200 truncate">
                    {routine.name}
                  </span>
                  <TriggerBadge type={routine.trigger_type} />
                </div>
                {routine.description && (
                  <p className="text-xs text-zinc-500 line-clamp-2">{routine.description}</p>
                )}
                <div className="flex items-center gap-3 text-[11px] text-zinc-600">
                  {routine.trigger_value && (
                    <span className="font-mono">{routine.trigger_value}</span>
                  )}
                  {routine.last_triggered && (
                    <span>Last: {formatDateTime(routine.last_triggered)}</span>
                  )}
                </div>
              </div>

              {/* Right: toggle + edit + delete */}
              <div className="flex items-center gap-3 shrink-0">
                <div className="flex items-center gap-2">
                  <Toggle
                    enabled={routine.enabled}
                    onChange={() => toggleMut.mutate(routine.id)}
                    disabled={toggleMut.isPending}
                  />
                  <span className="text-xs text-zinc-500">
                    {routine.enabled ? 'On' : 'Off'}
                  </span>
                </div>
                <div className="flex items-center gap-1">
                  <button
                    type="button"
                    title="Edit"
                    onClick={() => setEditRoutine(routine)}
                    className="p-1.5 text-zinc-500 hover:text-zinc-200 transition-colors rounded"
                    aria-label={`Edit ${routine.name}`}
                  >
                    <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                      <path d="M11.013 2.513a1.75 1.75 0 0 1 2.475 2.474L6.226 12.25a2.751 2.751 0 0 1-.892.596l-2.047.848a.75.75 0 0 1-.98-.98l.848-2.047a2.75 2.75 0 0 1 .596-.892l7.262-7.262Z" />
                    </svg>
                  </button>
                  <button
                    type="button"
                    title="Delete"
                    onClick={() => {
                      if (confirm(`Delete routine "${routine.name}"?`)) {
                        deleteMut.mutate(routine.id)
                      }
                    }}
                    disabled={deleteMut.isPending}
                    className="p-1.5 text-zinc-500 hover:text-red-400 disabled:opacity-40 transition-colors rounded"
                    aria-label={`Delete ${routine.name}`}
                  >
                    <svg className="w-4 h-4" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
                      <path d="M5 3.25V4H2.75a.75.75 0 0 0 0 1.5h.3l.815 8.15A1.5 1.5 0 0 0 5.357 15h5.285a1.5 1.5 0 0 0 1.493-1.35l.815-8.15h.3a.75.75 0 0 0 0-1.5H11v-.75A2.25 2.25 0 0 0 8.75 1h-1.5A2.25 2.25 0 0 0 5 3.25Zm2.25-.75a.75.75 0 0 0-.75.75V4h3v-.75a.75.75 0 0 0-.75-.75h-1.5ZM6.05 6a.75.75 0 0 1 .787.713l.275 5.5a.75.75 0 0 1-1.498.075l-.275-5.5A.75.75 0 0 1 6.05 6Zm3.9 0a.75.75 0 0 1 .712.787l-.275 5.5a.75.75 0 0 1-1.498-.075l.275-5.5A.75.75 0 0 1 9.95 6Z" />
                    </svg>
                  </button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      {/* Create modal */}
      {showCreate && (
        <RoutineModal
          onClose={() => setShowCreate(false)}
          onSave={(data) => createMut.mutate(data)}
          isSaving={createMut.isPending}
        />
      )}

      {/* Edit modal */}
      {editRoutine && (
        <RoutineModal
          initial={editRoutine}
          onClose={() => setEditRoutine(null)}
          onSave={(data) => updateMut.mutate({ id: editRoutine.id, ...data })}
          isSaving={updateMut.isPending}
        />
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

const TABS: { id: ActiveTab; label: string }[] = [
  { id: 'cron', label: 'Cron Jobs' },
  { id: 'routines', label: 'Routines' },
]

export default function CronRoutines() {
  const [activeTab, setActiveTab] = useState<ActiveTab>('cron')

  return (
    <div className="space-y-6">
      {/* Page header */}
      <div>
        <h1 className="text-2xl font-bold text-zinc-100 mb-1">Cron &amp; Routines</h1>
        <p className="text-zinc-400 text-sm">
          Manage scheduled cron jobs and event-driven routines for automated agent tasks.
        </p>
      </div>

      {/* Tab navigation */}
      <div className="flex border-b border-zinc-800 gap-6">
        {TABS.map((tab) => (
          <button
            key={tab.id}
            type="button"
            onClick={() => setActiveTab(tab.id)}
            className={`pb-2.5 text-sm font-medium transition-colors ${
              activeTab === tab.id
                ? 'border-b-2 border-violet-500 text-zinc-100'
                : 'text-zinc-400 hover:text-zinc-200'
            }`}
            aria-selected={activeTab === tab.id}
            role="tab"
          >
            {tab.label}
          </button>
        ))}
      </div>

      {/* Tab content */}
      <div role="tabpanel">
        {activeTab === 'cron' ? <CronTab /> : <RoutinesTab />}
      </div>
    </div>
  )
}
