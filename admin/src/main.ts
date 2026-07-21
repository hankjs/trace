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
    { path: '/image-providers', component: () => import('./views/ImageProviders.vue') },
    { path: '/weixin', component: () => import('./views/WeixinBot.vue') },
  ],
})

router.beforeEach((to) => {
  if (!to.meta.public && !hasToken()) {
    return '/login'
  }
})

const app = createApp(App)
app.use(router)
app.mount('#app')
