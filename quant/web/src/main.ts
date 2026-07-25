import { createApp } from 'vue'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import { getToken } from './api'
import './style.css'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/login', name: 'login', component: () => import('./views/Login.vue'), meta: { public: true } },
    { path: '/', name: 'dashboard', component: () => import('./views/Dashboard.vue') },
    { path: '/stock/:code', name: 'stock', component: () => import('./views/StockDetail.vue') },
    { path: '/selection', name: 'selection', component: () => import('./views/SelectionWorkspace.vue') },
    { path: '/pools', name: 'pools', component: () => import('./views/Pools.vue') },
    { path: '/signals', name: 'signals', component: () => import('./views/Signals.vue') },
    { path: '/strategies', name: 'strategies', component: () => import('./views/StrategyWorkspace.vue') },
    { path: '/portfolio', name: 'portfolio', component: () => import('./views/Portfolio.vue') },
    { path: '/catalog', name: 'catalog', component: () => import('./views/Catalog.vue') },
    {
      path: '/picks',
      redirect: (to) => ({ name: 'selection', query: { ...to.query, tab: 'picks' } }),
    },
    {
      path: '/screener',
      redirect: (to) => ({ name: 'selection', query: { ...to.query, tab: 'screener' } }),
    },
    {
      path: '/backtest',
      redirect: (to) => ({ name: 'strategies', query: { ...to.query, tab: 'backtest' } }),
    },
    {
      path: '/leaderboard',
      redirect: (to) => ({ name: 'strategies', query: { ...to.query, tab: 'leaderboard' } }),
    },
    { path: '/:pathMatch(.*)*', redirect: '/' },
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
