# Trace — AI Agent 开发环境

> 本文件面向 AI 编码 Agent，假设读者对本项目一无所知。
> 项目内注释、文档、commit message 主要使用中文。

## 项目概览

Trace 是一个全栈 AI Agent 桌面环境：**Rust (Axum) 服务端 + Tauri 2 / Vue 3 桌面客户端**。
Agent 运行在服务器上执行 shell 命令和文件操作，客户端通过 SSE 实时展示工作状态。
核心能力：多 LLM 提供商、消息树（对话分支/回溯）、Checkpoint 快照、变更管理
（Explore → Generate → Confirm → Archive 工作流）、版本化 Spec 系统、远程终端节点、微信通道。

这是一个 **monorepo**，包含四个相对独立的子系统：

| 目录 | 说明 |
|------|------|
| `server/` + `crates/` | Rust workspace：Axum 服务端及 Agent 核心 crates |
| `client/` | Tauri 2 + Vue 3 桌面客户端（包名 `hank-client`） |
| `admin/` | Vue 3 + Vite 管理后台 Web（包名 `hank-admin`） |

**quant 已迁出本 monorepo**：独立仓库 https://github.com/hankjs/quant（本地 `~/projects/hank/quant`）。
Trace 仅通过 HTTP A2A（`[quant_a2a]` + `quant_*` 工具 + `server/skills/quant-research`）调用 quant；
可共用同一 MySQL（`quant_*` 表前缀）与 JWT。改 quant 请到 quant 仓库并先读其 `AGENTS.md`。

## 技术栈

- **服务端**: Rust 2021, Axum 0.8, Tokio, SQLx 0.8 (**MySQL**, 非 SQLite), JWT (jsonwebtoken), bcrypt
- **客户端**: Vue 3.5 (`<script setup lang="ts">`), Vue Router 4, Tailwind CSS 4, TypeScript, Vite 6, Tauri 2, xterm.js, Vitest 4
- **管理后台**: Vue 3 + Vite + Tailwind 4（无 Tauri）
- **CLI**: Rust 独立项目 (clap, portable-pty, reqwest/rustls)
- **数据库**: MySQL（`config.toml` 的 `database_url`）；独立 quant 服务可共用同一 MySQL，其表带 `quant_` 前缀
- **配置**: 根目录 `config.toml`（gitignored，从 `config.example.toml` 复制）

注意：`README.md`/`CLAUDE.md` 中的部分描述已过时（如 SQLite、hank-agent/hank-web-tools 名称），以本文件和实际代码为准。

## 仓库结构

```
server/src/              Axum HTTP/SSE 服务 (hank-server, 0.0.0.0:3000)
  ├── main.rs            入口，AppState，路由注册
  ├── chat.rs            SSE 聊天处理 (chat/stop/resume)
  ├── routes.rs          会话、消息树、Provider、文件系统 API
  ├── changes.rs         变更管理 (Artifacts, Tasks, Explore/Generate)
  ├── specs.rs           Spec 版本化管理
  ├── checkpoints.rs     Checkpoint 回溯系统
  ├── admin.rs           管理后台 API
  ├── auth.rs            JWT 认证
  ├── llm.rs             通用 LLM completion / tool-exec 端点
  ├── image_gen.rs / skills.rs / requirement_docs.rs / snap_tools.rs / termshot.rs / websnap.rs
  └── weixin/            微信通道 (登录、消息路由、推送、监控)
crates/
  ├── hank-provider/     LLM 多厂商抽象 (anthropic.rs, openai.rs, types.rs)
  ├── code-agent/        Agent 核心：agent/ (orchestrator, worker, verifier, loop_detector),
  │                      context/ (上下文管理与摘要), runtime/ (事件与工具运行时), session.rs, retry.rs
  ├── code-tools/        工具实现：shell, read/write/str_replace 文件操作, search, git,
  │                      permission, explore/generate/spec 工具, web_fetch, test_runner, ask_user
  └── hank-db/           MySQL 持久化层 (SQLx, 单文件 lib.rs, 含连接重试)
client/src/              Tauri + Vue 3 客户端
  ├── views/             页面组件 (Login, SessionList, Chat, Agent, Changes, Specs, TerminalView 等)
  ├── components/        可复用组件 (Agent* 系列块渲染, chat/, terminal/)
  ├── composables/       状态与逻辑 hooks (useChatSSE, useAgentBlocks, useMessageTree 等)
  ├── agents/            ChangeAgent / ExploreAgent 前端逻辑
  ├── api/               API 客户端层
  └── terminal/          终端布局与标签状态
admin/src/               管理后台 (views/, components/, composables/)
deploy/                  部署脚本 + systemd unit (hank-server)
docs/                    mdBook 文档 (src/ 源码, book/ 已构建产物勿改), feature/, review/
Makefile                 所有常用任务的统一入口
```

## 常用命令

```bash
# 服务端 (Rust workspace)
cargo run -p hank-server          # 开发启动, 0.0.0.0:3000
cargo build --workspace           # 构建所有 crate
cargo build --release -p hank-server
cargo test --workspace            # Rust 测试 (主要在 crates/code-agent)

# 客户端 (client/)
cd client && pnpm install
pnpm tauri dev                    # Tauri 开发模式
pnpm dev                          # 仅 Vite 前端
pnpm build                        # vue-tsc 类型检查 + 构建
pnpm test:run                     # Vitest (happy-dom + msw, tests/ 与 src/**/*.spec.ts)

# 管理后台 (admin/)
cd admin && pnpm dev              # Vite 开发
cd admin && pnpm build            # vue-tsc + 构建, 产物 admin/dist 由服务端托管

# quant 量化系统（独立仓库，不在本 monorepo）
# cd ~/projects/hank/quant && make dev / make web / make deploy

# Makefile 汇总
make server-dev / client-dev / admin-dev
make app                          # 构建 Trace.app 并安装到 /Applications
```

包管理器统一用 **pnpm**。

## 运行时架构

- 服务端是唯一有状态中心：MySQL 存储会话/消息树/Spec/变更/用户，内存中维护活跃任务的
  `CancellationToken` 与 SSE 事件缓冲（断线可通过 `/events/resume` 恢复）。
- 所有 `/api/*`（除 `/api/health` 和 `/api/auth/login`）需要 `Authorization: Bearer <JWT>`。
- Agent loop 在 `code-agent` crate：orchestrator 编排 worker/verifier，工具由 `code-tools` 实现，
  LLM 调用走 `hank-provider`（Anthropic 原生协议 + OpenAI 兼容协议，可配多个 Provider 运行时切换）。
- Agent 工具一律在 server 进程内本地执行：远程执行节点（`hank-cli`）、沙箱降权与 server 侧
  代码工作区都已下线，server 不在任何用户机器上跑命令。
- 微信通道 (`server/src/weixin/`) 提供扫码登录、消息收发、`/snap` 网页截图 (headless Chromium)、
  `/shot` 终端截图 (SGR→SVG→PNG) 等能力。
- handy 等外部系统不经 trace 体内渠道代码接入，而是走独立桥接服务；
  trace 只暴露通用 client API（见下文「通用 client 交互单 API」）。

## 通用 client 交互单 API

任何第三方系统都可以通过 client API 驱动 trace 的 ask_user 交互（不需要 admin 权限，
只需普通 JWT；handler 内校验会话属于当前用户，不属于一律 404）：

- **SSE `interaction_created` 事件**：交互单落库成功后，服务端向同一事件流（broadcast，
  SSE 与 `/events/resume` 共用）紧随 ask_user 补发 `{"type":"interaction_created",
  interaction_id, session_id, kind, question, options, questions?, expires_at?}`。
  纯追加，ask_user 原事件不变；该事件由 server 侧生成，不进 agent_events 表。
- `GET /api/sessions/{id}/interactions?status=pending`：该会话交互单列表
  （默认只返回 pending，status 白名单与 admin 一致；字段 id/kind/status/question/
  options/questions/expires_at/created_at）。
- `POST /api/sessions/{id}/interactions/{iid}/answer` body `{"answer":"..."}`：
  options 白名单校验（多问题单跳过，与 admin 同一 `validate_answer`）后走
  `interaction_flow::answer_and_resume` 真派发，operator 为当前 JWT user_id。

管理侧对应端点在 `/api/admin/interactions*`（server/src/interactions.rs 前半）。

handy 等外部系统经独立桥接服务接入（桥接服务持有 trace 用户 JWT 或 API key，
调上述通用端点 + SSE/chat 接口驱动 trace）；trace 体内没有任何 handy 专用代码。

## API key 认证

外部系统（桥接服务等）可以不走用户名密码登录，直接用 API key 调 client API
（server/src/api_keys.rs）：

- **格式**：`trk_` + 32 字节随机的 urlsafe base64（共 47 字符）。明文只在创建时
  返回一次；`api_keys` 表只存 sha256 哈希（`key_hash` 唯一），另有
  revoked / created_at / last_used_at。
- **认证**：`Authorization: Bearer trk_...` 在 auth_middleware 走 API key 路径
  （sha256 查表 → 未吊销 → 归属用户存在 → 合成与 JWT 等价的 Claims，
  last_used_at 后台异步更新）；其余 Bearer 照旧走 JWT，两条路径互不影响。
- **权限口径**：API key 永远是 client scope（can_admin 恒 false，即使归属用户是
  admin），admin 路由天然拒绝；会话归属校验等 client 端点逻辑与 JWT 完全一致。
- **管理路径**：admin REST（`POST/GET /api/admin/api-keys`、
  `POST /api/admin/api-keys/{id}/revoke`，吊销幂等）+ 运维 provision 子命令
  （直连 DB，不启动 HTTP，cwd 需能读到 config.toml）：
  `hank-server create-api-key --username <名> --name <key名>`（明文只打印一次）/
  `list-api-keys` / `revoke-api-key --id <id>`。

## 编码约定

- 前端统一 `<script setup lang="ts">` + Composition API；样式用 Tailwind utility classes；
  API 调用集中在 `client/src/api/`；状态逻辑抽取为 composables。
- UI 设计遵循 `DESIGN.md`（设计 token）与 `PRODUCT.md`（产品原则：密度优先、可回溯、拒绝聊天机器人式 UI）。
- 后端错误处理用 `anyhow` / `thiserror`；异步统一 Tokio；日志用 `tracing`。
- workspace 依赖集中在根 `Cargo.toml` 的 `[workspace.dependencies]`，新增依赖先加到这里。
- 注释与 commit message 使用中文。
- `cli/Cargo.toml` 里有自足 `[workspace]` 声明，**不要**把它加进根 workspace（服务器上独立构建）。

## 测试

- Rust：`cargo test --workspace`。集成测试在 `crates/code-agent/tests/`，单元测试随源码 `#[cfg(test)]`。
- 客户端：`cd client && pnpm test:run`（Vitest + happy-dom + msw；测试在 `client/tests/` 和 `src/**/*.spec.ts`）。
- quant：`uv run pytest tests/`（engine 测试用合成数据，API 测试用内存 SQLite，不需要 MySQL）；前端 `cd web && pnpm test`。
- 改动后至少跑对应子系统的构建命令（`cargo build` / `pnpm build` 的 vue-tsc 检查）验证编译与类型。

## 部署

通过 `deploy/` 下的脚本 SSH 到线上服务器（`SSH_HOST`，默认 `wananyun`），systemd 管理服务。
构建全部在本地完成，服务器只接收产物：

```bash
make deploy               # 本地 zigbuild 交叉编译 server + 本地构建 admin -> 推产物到 /opt/hank -> systemctl 重启
./deploy/bootstrap-server.sh   # 仅全新服务器一次性初始化（建 hank 用户与目录、装 unit）
# quant 部署: cd ~/projects/hank/quant && make deploy
```

服务器是 Ubuntu 18.04（glibc 2.27），因此用 zig 做链接器交叉编译；本机需要
`brew install zig && cargo install cargo-zigbuild` 与 `rustup target add x86_64-unknown-linux-gnu`。
服务器上不装 rustup/node，也不再有 Git 基线仓库或沙箱。

`/opt/hank/current` 是指向 `releases/<id>` 的软链，`previous` 保留上一版可回退。
服务器上的 `config.toml` 只在缺失时上传一次，之后不会被覆盖。systemd unit 在 `deploy/hank-server.service`。

## 安全注意事项

- `config.toml` 含 API key、JWT secret、数据库凭据，已 gitignore，**绝不提交**；模板是 `config.example.toml`。
- 密码用 bcrypt 哈希；管理端存储的 Provider key 有加密处理。
- `/snap` 网页截图需要服务器安装 Chromium 并配置 `chrome_path`；`/shot` 终端截图需要等宽 CJK 字体。
- Agent 会在服务器上执行真实 shell 命令与文件写操作，工具权限控制在 `code-tools/permission.rs`，改动需谨慎。
- quant（独立仓库）是纯信息/回测系统：**永远不添加券商连接、下单或任何自动/半自动交易能力**（见 quant 仓库 `AGENTS.md` 的产品边界）。

## 其他

- 项目级 skills 在 `.agents/skills/`（agent-practice, impeccable, vue），前端 UI 工作可参考。
- `docs/` 是 mdBook 项目，`docs/book/` 为构建产物，不要手改。
- 现有 `README.md` / `CLAUDE.md` 有部分过时信息，遇到冲突以代码与本文件为准。
