// Polls GET /api/metrics every 10 seconds via react-query.
// Shape mirrors ZeptoClaw's telemetry JSON output (src/utils/telemetry.rs).

import { useQuery } from '@tanstack/react-query'
import { apiFetch } from '../lib/api'

export interface ToolStat {
  count: number
  total_ms: number
  errors: number
}

export interface MetricsData {
  tool_calls: Record<string, ToolStat>
  tokens: {
    prompt: number
    completion: number
  }
  cost: {
    total_usd: number
  }
}

export function useMetrics() {
  return useQuery<MetricsData>({
    queryKey: ['metrics'],
    queryFn: () => apiFetch<MetricsData>('/api/metrics'),
    refetchInterval: 10_000,
    retry: false,
  })
}
