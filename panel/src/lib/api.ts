// Base API client — reads panel_token from localStorage and injects Bearer auth.
// API_BASE is empty so requests hit the same origin; the dev proxy in vite.config.ts
// forwards /api/* and /ws/* to the ZeptoClaw health server.

const API_BASE = ''

export async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const token = localStorage.getItem('panel_token') ?? ''
  const res = await fetch(`${API_BASE}${path}`, {
    ...init,
    headers: {
      'Content-Type': 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...init?.headers,
    },
  })
  if (!res.ok) throw new Error(`API ${res.status}: ${res.statusText}`)
  return res.json() as Promise<T>
}
