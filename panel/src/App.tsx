import { useState } from 'react'
import { BrowserRouter, Routes, Route } from 'react-router'
import Layout from './components/Layout'
import Dashboard from './pages/Dashboard'
import Login from './pages/Login'
import Logs from './pages/Logs'
import Sessions from './pages/Sessions'
import CronRoutines from './pages/CronRoutines'
import Kanban from './pages/Kanban'
import Agents from './pages/Agents'
import { useAuth } from './hooks/useAuth'

export default function App() {
  // Trigger a re-render after login so the gated layout mounts immediately.
  const [authVersion, setAuthVersion] = useState(0)
  const { isAuthenticated } = useAuth()

  // If no token is stored, prompt for a password.
  // The panel may also run without password auth (static API token pre-set
  // in localStorage), in which case isAuthenticated is already true.
  if (!isAuthenticated) {
    return (
      <Login
        onSuccess={() => {
          // Bump version so App re-reads localStorage and shows the dashboard.
          setAuthVersion((v) => v + 1)
        }}
      />
    )
  }

  // authVersion is consumed only to force a re-render after login.
  void authVersion

  return (
    <BrowserRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<Dashboard />} />
          <Route path="logs" element={<Logs />} />
          <Route path="sessions" element={<Sessions />} />
          <Route path="cron" element={<CronRoutines />} />
          <Route path="kanban" element={<Kanban />} />
          <Route path="agents" element={<Agents />} />
        </Route>
      </Routes>
    </BrowserRouter>
  )
}
