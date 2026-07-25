import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  test: {
    environment: 'jsdom',
    clearMocks: true,
    restoreMocks: true,
  },
  build: {
    outDir: 'dist',
  },
  server: {
    proxy: {
      '/api': 'http://localhost:8100',
    },
  },
})
