// API 地址优先级：VITE_API_BASE 显式指定 > dev 用本地 server / 构建用线上 server。
// 远程终端长轮询 / 渠道派发都是 server 单进程内存状态，桌面 client 生产包必须连线上 server。
export const API_BASE =
  import.meta.env.VITE_API_BASE ||
  (import.meta.env.DEV ? "http://localhost:3000" : "https://trace.cpolar.cn");
