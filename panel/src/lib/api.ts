// Base API client — reads panel_token from localStorage and injects Bearer auth.
// API_BASE is empty so requests hit the same origin; the dev proxy in vite.config.ts
// forwards /api/* and /ws/* to the ZeptoClaw health server.
//
// Mutating requests (POST/PUT/DELETE/PATCH) automatically fetch and attach a CSRF
// token via the X-CSRF-Token header.  On a 403 response the token is cleared and
// the request is retried once in case the token expired.

let csrfToken: string | null = null

async function getCsrfToken(): Promise<string> {
  if (csrfToken) return csrfToken
  const res = await fetch('/api/csrf-token')
  const data = await res.json()
  csrfToken = data.token as string
  return csrfToken
}

export function clearCsrfToken(): void {
  csrfToken = null
}

export async function apiFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
  const token = localStorage.getItem('panel_token')
  const method = (init.method ?? 'GET').toUpperCase()
  const isMutating = ['POST', 'PUT', 'DELETE', 'PATCH'].includes(method)

  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
    ...(init.headers as Record<string, string> ?? {}),
  }

  if (isMutating) {
    headers['X-CSRF-Token'] = await getCsrfToken()
  }

  const res = await fetch(path, { ...init, headers })

  if (res.status === 403 && isMutating) {
    // CSRF token might be expired — clear and retry once
    csrfToken = null
    headers['X-CSRF-Token'] = await getCsrfToken()
    const retry = await fetch(path, { ...init, headers })
    if (!retry.ok) throw new Error(`API ${retry.status}: ${retry.statusText}`)
    return retry.json() as Promise<T>
  }

  if (!res.ok) throw new Error(`API ${res.status}: ${res.statusText}`)
  return res.json() as Promise<T>
}
