import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 9092,
    proxy: {
      '/api': 'http://localhost:9091',
      '/ws': { target: 'ws://localhost:9091', ws: true },
    },
  },
})
