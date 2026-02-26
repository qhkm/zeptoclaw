// Login page — password entry form for the ZeptoClaw panel.
//
// Shown when the user has no valid token in localStorage.
// On successful login the parent (App.tsx) re-renders with isAuthenticated=true
// and the layout/dashboard becomes visible.

import { useState, type FormEvent } from 'react'
import { useAuth } from '../hooks/useAuth'

interface LoginProps {
  onSuccess: () => void
}

export default function Login({ onSuccess }: LoginProps) {
  const { login, error, loading } = useAuth()
  const [password, setPassword] = useState('')

  async function handleSubmit(e: FormEvent) {
    e.preventDefault()
    const ok = await login(password)
    if (ok) onSuccess()
  }

  return (
    <div className="min-h-screen bg-zinc-950 flex items-center justify-center px-4">
      <div className="w-full max-w-sm">
        {/* Logo / wordmark */}
        <div className="mb-8 text-center">
          <span className="text-2xl font-bold tracking-tight text-zinc-100">
            ZeptoClaw
          </span>
          <p className="mt-1 text-sm text-zinc-500">Control Panel</p>
        </div>

        {/* Card */}
        <div className="bg-zinc-900 border border-zinc-800 rounded-xl p-6 shadow-2xl">
          <h1 className="text-base font-semibold text-zinc-100 mb-5">Sign in</h1>

          <form onSubmit={handleSubmit} className="space-y-4">
            <div className="space-y-1.5">
              <label
                htmlFor="password"
                className="block text-xs font-medium text-zinc-400 uppercase tracking-wider"
              >
                Password
              </label>
              <input
                id="password"
                type="password"
                autoComplete="current-password"
                required
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="Enter panel password"
                className="w-full rounded-lg bg-zinc-800 border border-zinc-700 px-3 py-2.5
                           text-sm text-zinc-100 placeholder-zinc-600
                           focus:outline-none focus:ring-2 focus:ring-violet-500/50 focus:border-violet-500
                           disabled:opacity-50"
                disabled={loading}
              />
            </div>

            {error && (
              <p className="text-xs text-red-400 bg-red-500/10 border border-red-500/20 rounded-lg px-3 py-2">
                {error}
              </p>
            )}

            <button
              type="submit"
              disabled={loading || !password}
              className="w-full rounded-lg bg-violet-600 hover:bg-violet-500 active:bg-violet-700
                         disabled:opacity-40 disabled:cursor-not-allowed
                         text-sm font-semibold text-white
                         py-2.5 transition-colors duration-150"
            >
              {loading ? 'Signing in…' : 'Sign in'}
            </button>
          </form>
        </div>

        <p className="mt-4 text-center text-xs text-zinc-600">
          Alternatively, set{' '}
          <code className="font-mono text-zinc-500">panel_token</code> in
          localStorage to use a static API token.
        </p>
      </div>
    </div>
  )
}
