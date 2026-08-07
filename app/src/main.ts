import { createApp } from 'vue'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import './style.css'
import { getToken } from './api'
import './composables/useTheme'

const router = createRouter({
  history: createWebHistory('/app/'),
  routes: [
    { path: '/login', component: () => import('./views/Login.vue') },
    { path: '/', component: () => import('./views/Nodes.vue') },
    { path: '/nodes/:id', component: () => import('./views/NodeDetail.vue') },
    { path: '/settings', component: () => import('./views/Settings.vue') },
  ],
})

router.beforeEach((to) => {
  if (to.path !== '/login' && !getToken()) return '/login'
  if (to.path === '/login' && getToken()) return '/'
  return true
})

createApp(App).use(router).mount('#app')
