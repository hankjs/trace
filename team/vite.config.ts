import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

// 默认连本地 server；线上联调时用 HANK_API=http://... pnpm dev 覆盖
const apiTarget = process.env.HANK_API ?? 'http://127.0.0.1:3000'

export default defineConfig({
  // base: '/' 而非 /admin/：看板独立部署在自己的根路径下
  base: '/',
  plugins: [vue(), tailwindcss()],
  build: {
    outDir: 'dist',
  },
  server: {
    port: 18789,
    proxy: {
      '/api': apiTarget,
    },
  },
})
