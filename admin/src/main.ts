import { createApp } from 'vue'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import { hasToken } from './composables/api'
import './style.css'

declare module 'vue-router' {
  interface RouteMeta {
    public?: boolean
    fill?: boolean
    width?: 'wide' | 'full'
  }
}

const router = createRouter({
  history: createWebHistory('/admin/'),
  routes: [
    { path: '/login', component: () => import('./views/Login.vue'), meta: { public: true } },
    { path: '/', component: () => import('./views/Dashboard.vue') },
    { path: '/sessions', component: () => import('./views/Sessions.vue') },
    { path: '/sessions/:id', component: () => import('./views/SessionDetail.vue') },
    { path: '/sessions/:id/timeline', component: () => import('./views/SessionTimeline.vue') },
    { path: '/sessions/:id/explore', component: () => import('./views/SessionExplore.vue'), meta: { width: 'wide' } },
    { path: '/explore', component: () => import('./views/ExploreList.vue') },
    { path: '/explore/:id', component: () => import('./views/SessionExplore.vue'), meta: { width: 'wide' } },
    { path: '/prompts', component: () => import('./views/PromptLab.vue') },
    { path: '/users', component: () => import('./views/Users.vue') },
    { path: '/api-keys', component: () => import('./views/ApiKeys.vue') },
    { path: '/providers', component: () => import('./views/Providers.vue') },
    { path: '/image-providers', component: () => import('./views/ImageProviders.vue') },
    { path: '/weixin', component: () => import('./views/WeixinBot.vue') },
    { path: '/feishu', component: () => import('./views/FeishuBot.vue') },
    { path: '/chat-records', component: () => import('./views/ChatRecords.vue'), meta: { fill: true, width: 'wide' } },
    { path: '/jobs', component: () => import('./views/Jobs.vue') },
    { path: '/interactions', component: () => import('./views/Interactions.vue'), meta: { width: 'wide' } },
    { path: '/interactions/:id', component: () => import('./views/Interactions.vue'), meta: { width: 'wide' } },
  ],
})

// 兼容飞书卡片深链：{admin_base_url}/#/interactions/{id}
// admin 使用 history 路由，hash 不会被 vue-router 消费，启动时改写到 history 路径。
const deepHash = window.location.hash.match(/^#\/(interactions(?:\/[^/?#]+)?)/)
if (deepHash) {
  const target = `/${deepHash[1]}`
  window.history.replaceState(
    null,
    '',
    `${import.meta.env.BASE_URL.replace(/\/$/, '')}${target}`,
  )
}

router.beforeEach((to) => {
  if (!to.meta.public && !hasToken()) {
    return '/login'
  }
})

const app = createApp(App)
app.use(router)
app.mount('#app')
