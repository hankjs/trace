# Hank Agent Web - 项目导读

## 概览

全栈 AI Agent 桌面应用：Rust (Axum) 后端 + Vue 3 (Tauri 2) 前端。
支持多 LLM 提供商、WebSocket 实时通信、变更管理、Spec 系统。

## 技术栈

- **前端**: Vue 3.5 + Composition API, Vue Router 4, Tailwind CSS 4, TypeScript, Vite 6, Tauri 2
- **后端**: Rust, Axum 0.8, Tokio, SQLx 0.8 (SQLite), JWT 认证
- **数据库**: SQLite (`data.db`)
- **配置**: `config.toml` (LLM 提供商配置)

## 目录结构

```
├── server/src/          # Rust 后端
│   ├── main.rs          # 入口，启动 Axum 服务 (0.0.0.0:3000)
│   ├── routes.rs        # 路由定义
│   ├── chat.rs          # SSE 聊天处理（run_chat_turn 与传输层解耦，各渠道复用）
│   ├── weixin/          # 微信渠道（ilink 长轮询 monitor/router/pusher）
│   ├── feishu/          # 飞书渠道（pbbp2 WS 长连接 + 任务卡片 + 按钮确认，见 docs/feishu.md）
│   ├── scheduler/       # 定时任务调度（cron + job_runs 日志 + admin 手动触发）
│   ├── interactions.rs  # 交互单 admin REST
│   ├── interaction_flow.rs # 交互单应答与派发（飞书按钮 / admin 共用）
│   ├── team_task/       # 团队任务流水线（开发→评审→测试多角色编排，见 docs/feature/team-task-pipeline.md）
│   ├── changes.rs       # 变更管理 API
│   ├── specs.rs         # Spec 管理 API
│   ├── admin.rs         # 管理端点
│   └── auth.rs          # JWT 认证
├── crates/              # Rust workspace crates
│   ├── hank-provider/   # LLM 提供商抽象 (Anthropic, OpenAI 兼容)
│   ├── hank-agent/      # Agent 循环、消息历史、工具调用
│   ├── hank-web-tools/  # 服务端工具 (shell 执行等)
│   └── hank-db/         # SQLite 持久化层
├── client/src/          # Vue 3 前端
│   ├── views/           # 页面组件 (路由级)
│   ├── components/      # 可复用 UI 组件
│   ├── composables/     # 状态与逻辑 (useSession, useCanvasTree 等)
│   ├── api/index.ts     # API 客户端层
│   ├── router/index.ts  # 路由配置
│   ├── App.vue          # 根组件
│   └── main.ts          # 前端入口
├── admin/src/           # Vue 3 管理后台（独立前端，非 Tauri）
│   ├── views/           # 管理页面（路由级）
│   ├── components/      # 可复用组件
│   └── composables/api.ts  # admin API 客户端与类型
├── team/                # 团队任务看板（独立前端，端口 18789，hash 路由）
│   └── src/views/       # Login / TaskBoard / TaskDetail
├── cli/                 # hank-cli：headless 远程终端节点（独立 Cargo 项目，不进 workspace）
├── quant/               # A股日频量化信息系统（独立 Python 项目，仅与 server 共用 MySQL）
│   ├── app/             # FastAPI 后端：baostock/akshare 采集、指标、策略信号、记账、回测
│   └── web/             # Vue 3 + Vite + Tailwind 4 + ECharts 看板（风格对齐 admin/）
├── openspec/            # OpenSpec 集成
├── config.toml          # 运行时配置
└── Cargo.toml           # Rust workspace 配置
```

## 前端路由

| 路径 | 组件 | 说明 |
|------|------|------|
| `/login` | Login.vue | 登录 |
| `/` | SessionList.vue | 会话列表 |
| `/chat/:sessionId` | Chat.vue | 主聊天界面 |
| `/specs` | Specs.vue | Spec 管理 |
| `/changes` | Changes.vue | 变更列表 |
| `/changes/:changeId` | ChangeDetail.vue | 变更详情 |

## Admin 页面

base path `/admin/`（history 路由）。飞书卡片深链：`{admin_base_url}/admin/interactions/{id}`。

| 路径 | 组件 | 说明 |
|------|------|------|
| `/login` | Login.vue | 登录 |
| `/` | Dashboard.vue | 概览 |
| `/sessions` | Sessions.vue | 会话列表 |
| `/sessions/:id` | SessionDetail.vue | 会话详情 |
| `/sessions/:id/timeline` | SessionTimeline.vue | 会话时间线 |
| `/sessions/:id/explore` | SessionExplore.vue | 会话 Explore |
| `/explore` | ExploreList.vue | Explore 列表 |
| `/explore/:id` | SessionExplore.vue | Explore 详情（复用） |
| `/prompts` | PromptLab.vue | Prompt 实验室 |
| `/users` | Users.vue | 用户管理 |
| `/providers` | Providers.vue | LLM Provider |
| `/agent-cli` | AgentCli.vue | Agent CLI 凭据与配置 |
| `/image-providers` | ImageProviders.vue | 图像生成 Provider |
| `/weixin` | WeixinBot.vue | 微信机器人账号与绑定 |
| `/feishu` | FeishuBot.vue | 飞书机器人账号与绑定 |
| `/chat-records` | ChatRecords.vue | 渠道消息留档 |
| `/jobs` | Jobs.vue | 定时任务与执行记录 |
| `/interactions` | Interactions.vue | 交互单：确认闸门 / ask_user / 任务闸门的状态与手动应答 |
| `/team-task` | TeamTask.vue | 团队任务流水线配置：闸门/流水线开关、角色顺序、闸门边界（存 DB，即时生效） |
| `/interactions/:id` | Interactions.vue | 交互单详情（同页） |
| `/terminals` | Terminals.vue | 远程终端 |
| `/notifications` | Notifications.vue | 通知 |

## 前端组件清单

**views/**: Login, SessionList, Chat, Specs, Changes, ChangeDetail
**components/**: ArtifactReview, ChangeChatPanel, ConversationOutline, FolderPicker, LocalAgentSettings, MessageToast, NewChangeDialog, SpecPanel
**composables/**: useSession (认证), useCanvasTree (树结构), useMessageTree, useMessage, changes (变更API), specs (Spec API)

## 后端 Crate 职责

| Crate | 职责 |
|-------|------|
| `hank-provider` | LLM API 调用抽象，支持 Anthropic/OpenAI 协议 |
| `hank-agent` | Agent 主循环，消息管理，工具调用编排 |
| `hank-web-tools` | 具体工具实现 (shell, 文件操作等) |
| `hank-db` | 数据库 schema、迁移、CRUD 操作 |

## 常用命令

```bash
# 前端开发
cd client && npm run dev        # Vite 开发服务器
cd client && npm run build      # 构建

# 管理后台
cd admin && pnpm dev            # admin 开发服务器
cd admin && pnpm build          # admin 构建（严格 TS 检查）

# 团队任务看板（独立前端，端口 18789，hash 路由 #/team/:taskNo）
cd team && pnpm install && pnpm dev
cd team && pnpm build

# 后端开发
cargo run -p hank-server        # 启动后端服务
cargo build --workspace         # 构建所有 crate

# Tauri 开发
cd client && pnpm tauri dev     # Tauri 开发模式

# quant 量化系统(独立 Python 项目)
cd quant && uv sync                          # 安装依赖(Python 3.11~3.13)
cd quant && uv run uvicorn app.main:app --port 8100   # 后端(含定时采集任务)
cd quant/web && pnpm install && pnpm dev     # 前端看板(/api 代理到 8100)
```

quant 说明:A股日频量化信息系统,纯信息系统不做自动交易。baostock 提供历史/盘后日线(前复权),akshare 提供盘中快照;所有表带 `quant_` 前缀,与 server 共用同一个 MySQL,除此之外完全解耦。后期通过 quant 的 REST API 对接 server 的微信通道。详见 `quant/README.md`。

## 编码约定

- 前端使用 `<script setup lang="ts">` + Composition API
- 样式使用 Tailwind CSS utility classes
- API 调用集中在 `client/src/api/index.ts`
- 状态逻辑抽取为 composables
- 后端错误处理使用 `anyhow` / 自定义 Error 类型
- 中文注释和 commit message
