import { createApp } from 'vue'
import { createRouter, createWebHashHistory } from 'vue-router'
import App from './App.vue'
import { hasToken } from './composables/api'
import './style.css'

// 看板用 hash 路由而 admin 用 history——托管方式不同：
// admin 由 server 的 ServeDir 托管、写成 hash 会 404；
// 看板独立部署，hash 路由不需要服务端 rewrite 配合。
// 深链格式：http://host:18789/#team/tsk_xxx（与 sync_team_card 拼的 URL 一致）
const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/login', component: () => import('./views/Login.vue'), meta: { public: true } },
    { path: '/', component: () => import('./views/TaskBoard.vue') },
    { path: '/team/:taskNo', component: () => import('./views/TaskDetail.vue') },
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
