import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

// 默认连线上 server；本地联调时用 HANK_API=http://localhost:3000 pnpm dev 覆盖
const apiTarget = process.env.HANK_API ?? 'https://trace.cpolar.cn'

export default defineConfig({
  base: '/admin/',
  plugins: [vue(), tailwindcss()],
  build: {
    outDir: 'dist',
  },
  server: {
    proxy: {
      '/api': apiTarget,
    },
  },
})
