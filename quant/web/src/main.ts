import { createApp } from 'vue'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import { getToken } from './api'
import './style.css'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/login', component: () => import('./views/Login.vue'), meta: { public: true } },
    { path: '/', component: () => import('./views/Dashboard.vue') },
    { path: '/stock/:code', component: () => import('./views/StockDetail.vue') },
    { path: '/picks', component: () => import('./views/Picks.vue') },
    { path: '/screener', component: () => import('./views/Screener.vue') },
    { path: '/signals', component: () => import('./views/Signals.vue') },
    { path: '/portfolio', component: () => import('./views/Portfolio.vue') },
    { path: '/backtest', component: () => import('./views/Backtest.vue') },
    { path: '/leaderboard', component: () => import('./views/Leaderboard.vue') },
  ],
})

// 未登录一律跳登录页
router.beforeEach((to) => {
  if (!to.meta.public && !getToken()) {
    return { path: '/login' }
  }
  if (to.path === '/login' && getToken()) {
    return { path: '/' }
  }
})

const app = createApp(App)
app.use(router)
app.mount('#app')
