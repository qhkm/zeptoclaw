// Polls GET /api/health every 5 seconds via react-query.
// Shape mirrors ZeptoClaw's HealthServer JSON response (src/health.rs).

import { useQuery } from '@tanstack/react-query'
import { apiFetch } from '../lib/api'

export interface ComponentCheck {
  status: string
  error?: string
  restart_count?: number
}

export interface HealthData {
  version: string
  uptime_secs: number
  rss_bytes: number
  status: string
  components: Record<string, ComponentCheck>
  usage: {
    requests: number
    tool_calls: number
    tokens: number
    errors: number
  }
}

export function useHealth() {
  return useQuery<HealthData>({
    queryKey: ['health'],
    queryFn: () => apiFetch<HealthData>('/api/health'),
    refetchInterval: 5_000,
    retry: false,
  })
}
