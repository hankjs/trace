import { createApp } from 'vue'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import './style.css'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', component: () => import('./views/Dashboard.vue') },
    { path: '/stock/:code', component: () => import('./views/StockDetail.vue') },
    { path: '/signals', component: () => import('./views/Signals.vue') },
    { path: '/portfolio', component: () => import('./views/Portfolio.vue') },
    { path: '/backtest', component: () => import('./views/Backtest.vue') },
  ],
})

const app = createApp(App)
app.use(router)
app.mount('#app')
