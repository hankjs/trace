# Trace

远程 AI Agent 服务 + Tauri 桌面客户端。Agent 运行在服务器上执行 shell 命令，客户端通过 SSE 实时展示工作状态。支持多 LLM 提供商、变更管理、Spec 系统、Checkpoint 回溯。

## 项目结构

```
server/src/              Axum HTTP/SSE 服务
  ├── main.rs            入口，路由注册
  ├── chat.rs            SSE 聊天处理 (chat/stop/resume)
  ├── routes.rs          会话、消息、Provider、文件系统 API
  ├── changes.rs         变更管理 (Artifacts, Tasks, Explore/Generate)
  ├── specs.rs           Spec 版本化管理
  ├── checkpoints.rs     Checkpoint 回溯系统
  ├── admin.rs           管理后台 (用户/Provider/Prompt/Replay)
  └── auth.rs            JWT 认证
crates/
  ├── hank-provider/     LLM 多厂商抽象 (Anthropic, OpenAI 兼容)
  ├── hank-agent/        Agent loop (消息历史 + tool calling)
  ├── hank-web-tools/    服务端工具 (shell, 文件操作)
  └── hank-db/           SQLite 持久化 (SQLx)
client/src/              Tauri v2 + Vue 3 客户端
  ├── views/             页面组件 (Login, SessionList, Chat, Specs, Changes, ChangeDetail)
  ├── components/        可复用 UI 组件
  ├── composables/       状态与逻辑 hooks
  ├── api/index.ts       API 客户端层
  └── router/index.ts    路由配置
openspec/                OpenSpec 集成
```

## 前置要求

- Rust 1.75+
- Node.js 18+ / pnpm
- 至少一个 LLM API key (Anthropic / OpenAI / 兼容服务)

## 快速开始

### 1. 配置

```bash
cp config.example.toml config.toml
```

编辑 `config.toml`，填入 API key：

```toml
[[providers]]
name = "anthropic"
type = "anthropic"
api_key = "sk-ant-your-key-here"
base_url = "https://api.anthropic.com"
default_model = "sonnet"

[providers.models]
sonnet = "claude-sonnet-4-20250514"
opus = "claude-opus-4-20250514"
haiku = "claude-haiku-4-5-20251001"
```

### 2. 启动 Server

```bash
cargo run -p hank-server
```

默认监听 `0.0.0.0:3000`。

### 3. 启动 Client

```bash
cd client
pnpm install
pnpm tauri dev
```

## 构建

### Server (Release)

```bash
cargo build --release -p hank-server
# 产物: target/release/hank-server
```

### Client (桌面应用)

```bash
cd client
pnpm tauri build
# 产物在 src-tauri/target/release/bundle/
```

## API

### 认证

所有 `/api/*` 路由（除 health 和 login）需要认证，通过 `Authorization: Bearer <token>` 传递：

- **JWT**：`POST /api/auth/login` 用用户名密码换取（30 天有效）。
- **API key**：外部系统（桥接服务等）用 `trk_` 前缀的 key，永远是 client scope
  （拿不到 admin 权限）；由 admin REST `/api/admin/api-keys` 或运维子命令
  `hank-server create-api-key` 签发，详见 `AGENTS.md` 的「API key 认证」一节。

### REST 端点

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | /api/health | 健康检查 |
| POST | /api/auth/login | 获取 JWT token |
| GET | /api/auth/whoami | 凭证自检：回显身份（JWT 与 API key 均可，401 = 无效/已吊销） |
| GET | /api/providers | 可用 Provider 和模型列表 |
| POST | /api/sessions | 创建会话 |
| GET | /api/sessions | 会话列表 |
| GET | /api/sessions/:id | 会话详情 |
| PUT | /api/sessions/:id | 更新会话 (标题/工作目录/本地Agent) |
| DELETE | /api/sessions/:id | 删除会话 |
| GET | /api/sessions/:id/messages | 消息历史 (支持 leaf_id 分支查询) |
| POST | /api/sessions/:id/messages | 保存消息 (本地 Agent 会话) |
| POST | /api/sessions/:id/messages/truncate | 截断消息 |
| GET | /api/sessions/:id/tree | 消息树结构 |
| PUT | /api/sessions/:id/active-leaf | 切换活跃分支 |
| POST | /api/sessions/:id/chat | SSE 聊天（交互单落库后补发 interaction_created 事件） |
| POST | /api/sessions/:id/stop | 停止生成 |
| GET | /api/sessions/:id/events/resume | 恢复事件流 |
| GET | /api/sessions/:id/interactions | 会话交互单列表（?status=，默认 pending，仅限本人会话） |
| POST | /api/sessions/:id/interactions/:iid/answer | 应答交互单（{"answer":"..."}，options 白名单校验后真派发） |
| POST | /api/sessions/:id/local-events | 上传本地执行事件 |
| GET | /api/sessions/:id/events | 获取会话事件 (remote + local) |
| GET | /api/sessions/:id/checkpoints | Checkpoint 列表 |
| PUT | /api/settings | 更新设置 |
| GET | /api/fs/list | 目录浏览 |

### Specs API

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | /api/specs | Spec 列表 |
| POST | /api/specs | 创建 Spec |
| GET | /api/specs/:id | Spec 详情 |
| PUT | /api/specs/:id | 更新 Spec |
| DELETE | /api/specs/:id | 删除 Spec |
| GET | /api/specs/:id/versions | Spec 版本历史 |

### Changes API

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | /api/changes | 变更列表 |
| POST | /api/changes | 创建变更 |
| GET | /api/changes/:id | 变更详情 |
| PUT | /api/changes/:id | 更新变更 |
| DELETE | /api/changes/:id | 删除变更 |
| POST | /api/changes/:id/explore | 启动探索 |
| POST | /api/changes/:id/generate | 启动生成 |
| POST | /api/changes/:id/artifacts/confirm | 确认 Artifacts |
| POST | /api/changes/:id/archive | 归档变更 |
| GET | /api/changes/:id/artifacts | Artifact 列表 |
| POST | /api/changes/:id/artifacts | 创建 Artifact |
| GET | /api/changes/:id/artifacts/:aid | Artifact 详情 |
| PUT | /api/changes/:id/artifacts/:aid | 更新 Artifact |
| DELETE | /api/changes/:id/artifacts/:aid | 删除 Artifact |
| GET | /api/changes/:id/tasks | 任务列表 |
| POST | /api/changes/:id/tasks | 批量创建任务 |
| PUT | /api/changes/:id/tasks/:tid | 更新任务 |
| DELETE | /api/changes/:id/tasks/:tid | 删除任务 |
| GET | /api/changes/:id/context | 变更上下文 |

### Admin API

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | /api/admin/sessions | 所有会话 |
| GET | /api/admin/sessions/:id/replay | 会话回放 |
| GET | /api/admin/sessions/:id/events | 会话事件 |
| GET | /api/admin/metrics/overview | 总览指标 |
| GET | /api/admin/metrics/by-session/:id | 会话指标 |
| POST | /api/admin/prompt-templates | 创建 Prompt 模板 |
| GET | /api/admin/prompt-templates | Prompt 模板列表 |
| DELETE | /api/admin/prompt-templates/:id | 删除 Prompt 模板 |
| POST | /api/admin/chat/generate | 生成对话 |
| POST | /api/admin/replay | Prompt 回放 |
| GET | /api/admin/users | 用户列表 |
| POST | /api/admin/users | 创建用户 |
| PUT | /api/admin/users/:id | 更新用户 |
| DELETE | /api/admin/users/:id | 删除用户 |
| GET | /api/admin/providers | Provider 列表 |
| POST | /api/admin/providers | 创建 Provider |
| PUT | /api/admin/providers/:id | 更新 Provider |
| DELETE | /api/admin/providers/:id | 删除 Provider |

## 多 Provider 配置

支持同时配置多个 Provider，每个有独立的 base_url、api_key 和默认模型：

```toml
[[providers]]
name = "deepseek"
type = "openai"
api_key = "sk-xxx"
base_url = "https://api.deepseek.com"
default_model = "chat"

[providers.models]
chat = "deepseek-chat"
reasoner = "deepseek-reasoner"
```

`type = "openai"` 兼容所有 OpenAI 格式的 API（DeepSeek、Groq 等）。

## 外部系统接入

handy 等外部系统经独立桥接服务接入，trace 体内没有 handy 专用代码。trace 只暴露
通用 client API：SSE 聊天流（交互单落库后补发 `interaction_created` 事件）+
`GET /api/sessions/:id/interactions` / `POST /api/sessions/:id/interactions/:iid/answer`
交互单查询与应答端点，详见 `AGENTS.md` 的「通用 client 交互单 API」一节。

## 核心功能

- **多 LLM 提供商**: 同时配置 Anthropic、OpenAI 兼容服务，运行时切换
- **消息树**: 支持对话分支、回溯到任意节点继续对话
- **Checkpoint 系统**: 保存/恢复对话状态快照
- **变更管理**: 结构化的 Explore → Generate → Confirm → Archive 工作流
- **Spec 系统**: 版本化的项目规格文档管理
- **本地 Agent**: 支持本地 ACP 执行，事件同步到服务端
- **Admin 后台**: 用户管理、Provider 管理、Prompt Playground、会话回放
