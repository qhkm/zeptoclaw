import { BrowserRouter, Routes, Route } from 'react-router'
import Layout from './components/Layout'
import Dashboard from './pages/Dashboard'
import Logs from './pages/Logs'
import Sessions from './pages/Sessions'
import CronRoutines from './pages/CronRoutines'
import Kanban from './pages/Kanban'
import Agents from './pages/Agents'

export default function App() {
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
