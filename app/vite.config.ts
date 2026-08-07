import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'
import { VitePWA } from 'vite-plugin-pwa'

// 默认连线上 server；本地联调: HANK_API=http://localhost:3000 pnpm dev
const apiTarget = process.env.HANK_API ?? 'https://trace.cpolar.cn'

export default defineConfig({
  base: '/app/',
  plugins: [
    vue(),
    tailwindcss(),
    VitePWA({
      registerType: 'prompt',
      includeAssets: ['favicon.png', 'favicon-32.png'],
      manifest: {
        name: 'App',
        short_name: 'App',
        description: 'Trace 远程终端 App',
        lang: 'zh-CN',
        theme_color: '#e1e5ec',
        background_color: '#e1e5ec',
        display: 'standalone',
        start_url: '/app/',
        scope: '/app/',
        icons: [
          { src: 'favicon.png', sizes: '192x192', type: 'image/png' },
          { src: 'favicon.png', sizes: '512x512', type: 'image/png' },
        ],
      },
      workbox: {
        navigateFallback: '/app/index.html',
        navigateFallbackDenylist: [/^\/api\//],
      },
      devOptions: { enabled: false },
    }),
  ],
  build: { outDir: 'dist' },
  server: {
    port: 18791,
    proxy: { '/api': apiTarget },
  },
})
