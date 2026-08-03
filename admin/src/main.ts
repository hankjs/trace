import { createApp } from 'vue'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import { hasToken } from './composables/api'
import './style.css'

const router = createRouter({
  history: createWebHistory('/admin/'),
  routes: [
    { path: '/login', component: () => import('./views/Login.vue'), meta: { public: true } },
    { path: '/', component: () => import('./views/Dashboard.vue') },
    { path: '/sessions', component: () => import('./views/Sessions.vue') },
    { path: '/sessions/:id', component: () => import('./views/SessionDetail.vue') },
    { path: '/sessions/:id/timeline', component: () => import('./views/SessionTimeline.vue') },
    { path: '/sessions/:id/explore', component: () => import('./views/SessionExplore.vue') },
    { path: '/explore', component: () => import('./views/ExploreList.vue') },
    { path: '/explore/:id', component: () => import('./views/SessionExplore.vue') },
    { path: '/prompts', component: () => import('./views/PromptLab.vue') },
    { path: '/users', component: () => import('./views/Users.vue') },
    { path: '/providers', component: () => import('./views/Providers.vue') },
    { path: '/agent-cli', component: () => import('./views/AgentCli.vue') },
    { path: '/image-providers', component: () => import('./views/ImageProviders.vue') },
    { path: '/weixin', component: () => import('./views/WeixinBot.vue') },
    { path: '/feishu', component: () => import('./views/FeishuBot.vue') },
    { path: '/chat-records', component: () => import('./views/ChatRecords.vue') },
    { path: '/jobs', component: () => import('./views/Jobs.vue') },
    { path: '/team-task', component: () => import('./views/TeamTask.vue') },
    { path: '/interactions', component: () => import('./views/Interactions.vue') },
    { path: '/interactions/:id', component: () => import('./views/Interactions.vue') },
    { path: '/terminals', component: () => import('./views/Terminals.vue') },
    { path: '/notifications', component: () => import('./views/Notifications.vue') },
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
