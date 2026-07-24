// API 地址优先级：VITE_API_BASE 显式指定 > dev 用本地 server / 构建用线上 server。
// 注意：远程执行的心跳与任务队列是 server 单进程内存状态，微信 bot 跑在线上，
// 所以桌面 client 必须连线上 server 才能被微信渠道派任务。
export const API_BASE =
  import.meta.env.VITE_API_BASE ||
  (import.meta.env.DEV ? "http://localhost:3000" : "http://111.170.174.167:3000");
