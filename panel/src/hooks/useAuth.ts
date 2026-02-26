// useAuth — manages panel authentication state.
//
// Persists the token in localStorage under the key "panel_token" so the user
// stays authenticated across page refreshes.  The hook exposes login/logout
// helpers and an isAuthenticated flag consumed by App.tsx to gate the UI.

import { useState, useCallback } from 'react'
import { apiFetch } from '../lib/api'

interface LoginResponse {
  token: string
}

export function useAuth() {
  const [token, setToken] = useState<string | null>(
    () => localStorage.getItem('panel_token'),
  )
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  const login = useCallback(async (password: string): Promise<boolean> => {
    try {
      setError(null)
      setLoading(true)
      const res = await apiFetch<LoginResponse>('/api/auth/login', {
        method: 'POST',
        body: JSON.stringify({ password }),
      })
      localStorage.setItem('panel_token', res.token)
      setToken(res.token)
      return true
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Login failed')
      return false
    } finally {
      setLoading(false)
    }
  }, [])

  const logout = useCallback(() => {
    localStorage.removeItem('panel_token')
    setToken(null)
  }, [])

  return {
    token,
    isAuthenticated: !!token,
    login,
    logout,
    error,
    loading,
  }
}
