import { createApp } from 'vue'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import { getToken, isAdmin } from './api'
import './style.css'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/login', name: 'login', component: () => import('./views/Login.vue'), meta: { public: true } },
    { path: '/', name: 'dashboard', component: () => import('./views/Dashboard.vue') },
    { path: '/watchlist', name: 'watchlist', component: () => import('./views/Watchlist.vue') },
    { path: '/stock/:code', name: 'stock', component: () => import('./views/StockDetail.vue') },
    { path: '/selection', name: 'selection', component: () => import('./views/SelectionWorkspace.vue') },
    { path: '/pools', name: 'pools', component: () => import('./views/Pools.vue') },
    { path: '/signals', name: 'signals', component: () => import('./views/Signals.vue') },
    // 策略研究拆为独立页面,左侧导航以二级菜单进入
    { path: '/strategies/backtest', name: 'strategies-backtest', component: () => import('./views/Backtest.vue') },
    { path: '/strategies/experiments', name: 'strategies-experiments', component: () => import('./views/Experiments.vue') },
    { path: '/strategies/leaderboard', name: 'strategies-leaderboard', component: () => import('./views/Leaderboard.vue') },
    { path: '/strategies/manage', name: 'strategies-manage', component: () => import('./views/Strategies.vue') },
    {
      // 旧工作台按 tab 查询参数跳转到对应独立页面
      path: '/strategies',
      redirect: (to) => {
        const { tab, ...query } = to.query
        const name =
          tab === 'experiments' ? 'strategies-experiments'
            : tab === 'leaderboard' ? 'strategies-leaderboard'
              : tab === 'manage' ? 'strategies-manage'
                : 'strategies-backtest'
        return { name, query }
      },
    },
    { path: '/portfolio', name: 'portfolio', component: () => import('./views/Portfolio.vue') },
    { path: '/tasks', name: 'tasks', component: () => import('./views/Tasks.vue') },
    { path: '/catalog', name: 'catalog', component: () => import('./views/Catalog.vue') },
    { path: '/settings', name: 'settings', component: () => import('./views/Settings.vue') },
    { path: '/admin/jobs', name: 'admin-jobs', component: () => import('./views/AdminJobs.vue'), meta: { admin: true } },
    { path: '/admin/factors', name: 'admin-factors', component: () => import('./views/FactorsAdmin.vue'), meta: { admin: true } },
    { path: '/admin/gaps', name: 'admin-gaps', component: () => import('./views/AdminGaps.vue'), meta: { admin: true } },
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
      redirect: (to) => ({ name: 'strategies-backtest', query: to.query }),
    },
    {
      path: '/leaderboard',
      redirect: (to) => ({ name: 'strategies-leaderboard', query: to.query }),
    },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

// 未登录一律跳登录页;admin 页面非管理员回首页
router.beforeEach((to) => {
  if (!to.meta.public && !getToken()) {
    return { path: '/login' }
  }
  if (to.path === '/login' && getToken()) {
    return { path: '/' }
  }
  if (to.meta.admin && !isAdmin()) {
    return { path: '/' }
  }
})

const app = createApp(App)
app.use(router)
app.mount('#app')
