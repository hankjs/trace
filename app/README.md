# App — Trace 远程终端

手机一等公民的远程终端 Web 应用（PWA）。用 Trace 账号登录后，操作本机桌面客户端里已开启「允许远程终端」的节点。

## 开发

```bash
# 后端
make server-dev

# App 前端（默认代理线上 API）
make app-dev
# 或连本地 server：
cd app && HANK_API=http://localhost:3000 pnpm dev
```

访问：http://localhost:18791/app/

## 构建

```bash
cd app && pnpm build   # 产物 app/dist，由 hank-server 挂载在 /app
```

## 能力

- 节点列表 / 启停
- 终端列表、新建、关闭
- WebRTC 直连优先，失败静默回落 3s 中转
- 主题：亮 / 暗 / 跟随系统
