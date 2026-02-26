import { NavLink } from 'react-router'

interface NavItem {
  to: string
  label: string
  icon: string
}

const navItems: NavItem[] = [
  { to: '/',        label: 'Dashboard',      icon: '◈' },
  { to: '/logs',    label: 'Logs',           icon: '≡' },
  { to: '/sessions',label: 'Sessions',       icon: '◎' },
  { to: '/cron',    label: 'Cron & Routines',icon: '⏱' },
  { to: '/kanban',  label: 'Kanban',         icon: '▦' },
  { to: '/agents',  label: 'Agents',         icon: '⬡' },
]

export default function Sidebar() {
  return (
    <aside className="flex flex-col w-56 shrink-0 bg-zinc-900 border-r border-zinc-800 h-full">
      {/* Branding */}
      <div className="flex items-center gap-2 px-4 py-5 border-b border-zinc-800">
        <span className="text-lg font-bold text-zinc-100 tracking-tight leading-none">
          Zepto<span className="text-violet-400">Claw</span>
        </span>
      </div>

      {/* Navigation */}
      <nav className="flex-1 py-3 overflow-y-auto">
        <ul className="space-y-0.5 px-2">
          {navItems.map(({ to, label, icon }) => (
            <li key={to}>
              <NavLink
                to={to}
                end={to === '/'}
                className={({ isActive }) =>
                  [
                    'flex items-center gap-3 px-3 py-2 rounded-md text-sm transition-colors',
                    isActive
                      ? 'bg-zinc-800 text-zinc-100 font-medium'
                      : 'text-zinc-400 hover:bg-zinc-800/60 hover:text-zinc-200',
                  ].join(' ')
                }
              >
                <span className="text-base w-5 text-center leading-none" aria-hidden="true">
                  {icon}
                </span>
                {label}
              </NavLink>
            </li>
          ))}
        </ul>
      </nav>

      {/* Footer */}
      <div className="px-4 py-3 border-t border-zinc-800">
        <p className="text-xs text-zinc-600">v0.6.x</p>
      </div>
    </aside>
  )
}
