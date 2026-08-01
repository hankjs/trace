# 飞书渠道接入指南

飞书渠道让 server 的 agent 直接挂到飞书群里：话题群里 @机器人 派任务，进度以卡片原地刷新，高成本操作弹确认卡片，点按钮即拍板。入口和生命周期由 Rust server 统一管理（`server/src/feishu/`）；纯对话使用无工具的 native Agent，**代码与文件任务的 Codex / Claude Code / Grok / Kimi Code 必须在用户本机在线 `hank-cli` 节点执行（client-only）**，server 不再 bubblewrap 回退执行，也不再由 wananyun 自行修改/部署 Trace 源码。

## 架构

```
飞书开放平台 ←─ WS 长连接(pbbp2 protobuf 帧) ─→ feishu/ws.rs
        │ im.message.receive_v1 / card.action.trigger
        ▼
feishu/router.rs（消息解析、话题=会话、/命令、派发）
        │ run_chat_turn（读取 session metadata.agent_backend）
        ▼
feishu/pusher.rs（事件流 → 任务卡片 2s 节流刷新）
        │ AskUser → 确认卡片（按钮）
        ▼
feishu/callback.rs（按钮回调 → 包装成"确认"/"否"文本 → 现有确认闸门）
```

- **话题 = 会话**：`feishu_chats` 表把 `account_id:chat_id:topic_id`（topic = thread_id || root_id || "main"）映射到 server session，重启不丢
- **账号管理**：凭证存 `feishu_accounts` 表，admin REST 增删启停（与 weixin_accounts 同模式）；启用即起长连接，停用即断
- **用户绑定**：`feishu_bindings` 表，一次性 6 位绑定码流程（与微信相同），无需手配 open_id
- **确认闸门升级**：微信是文本白名单（回复"确认"），飞书是按钮卡片；回调文本化后走同一套 `handle_quant_confirmation`，code-agent 零改动
- **执行模式**：新话题始终由路由 Agent 确定 `agent_kind` 与 `agent_backend`（**不依赖** `[server_agent].enabled`）。`codex` / `claude` / `grok` / `kimi` **一律 client-only**：必须绑定在线且上报了对应 backend 的 `hank-cli`；节点不存在、离线或能力不匹配时直接失败，**绝不**回退 server bubblewrap、native 或另一节点。`[server_agent].enabled` 只管 server 侧 worktree / bubblewrap / `/diff` `/test` `/deploy` `/rollback`；关闭时 client-only hank-cli 链路仍然可用。纯对话 `native` 在 server_agent 开启时走 server 无工具会话，关闭时走普通 remote/native 会话。
- **管理员边界**：`can_login_admin` 仅在创建 **server 侧** native / worktree 会话时校验；纯对话与 client-only hank-cli 用户不要求 admin

## 一、飞书开放平台配置

1. 打开 [飞书开放平台](https://open.feishu.cn) → 开发者后台 → 创建企业自建应用
2. 记录 **App ID / App Secret**
3. **添加机器人能力**：应用功能 → 机器人 → 启用
4. **事件订阅**：事件与回调 → 事件订阅 → 订阅方式选 **使用长连接接收事件** →
   添加事件 `im.message.receive_v1`（接收消息）
5. **回调订阅**：事件与回调 → 回调订阅 → 订阅方式选 **使用长连接接收回调** →
   添加回调 `card.action.trigger`（卡片回传交互）
6. **权限**（权限管理，按需开通）：
   - `im:message`（读取消息）
   - `im:message:send_as_bot`（以机器人身份发消息）
   - `im:resource`（下载用户发来的截图）
   - `im:chat:readonly`（可选，后续扩展群信息）
7. **发布应用**：版本管理与发布 → 创建版本并发布（企业自建应用管理员审核后生效）
8. 建一个**话题群**，群设置 → 群机器人 → 把应用加进群

> 长连接只支持国内版飞书（open.feishu.cn）自建应用；海外版 Lark 不支持。

## 二、server 配置（admin REST，与微信同模式）

应用凭证**不写 config.toml**，存 `feishu_accounts` 表，通过 admin 接口管理（启停即启停长连接）：

```bash
# 添加应用（先校验凭证再落库，成功即启动 WS 长连接）
curl -X POST $SERVER/api/admin/feishu/accounts \
  -H "Authorization: Bearer $ADMIN_JWT" -H "Content-Type: application/json" \
  -d '{"name": "我的机器人", "app_id": "cli_xxx", "app_secret": "xxx"}'

# 查看 / 停用 / 启用 / 换 secret / 删除
curl $SERVER/api/admin/feishu/accounts -H "Authorization: Bearer $ADMIN_JWT"
curl -X PATCH $SERVER/api/admin/feishu/accounts/{id} -H "..." -d '{"enabled": false}'
curl -X PATCH $SERVER/api/admin/feishu/accounts/{id} -H "..." -d '{"app_secret": "new-secret"}'
curl -X DELETE $SERVER/api/admin/feishu/accounts/{id} -H "..."

# 绑定关系管理
curl $SERVER/api/admin/feishu/bindings -H "..."
curl -X DELETE $SERVER/api/admin/feishu/bindings/{id} -H "..."
```

**用户绑定**（与微信同一绑定码模式，不用手配 open_id）：

1. Trace client → 设置 → 飞书绑定 → 生成绑定码（6 位，10 分钟有效）
2. 飞书里给机器人发送 `bind 123456` → 绑定成功
3. 之后 @机器人 直接派任务即可

**多实例共库**：与 `weixin_monitor` 同理，只能一个实例开长连接：

```toml
[server]
feishu_monitor = false   # 其他实例关掉
```

## 三、用法

| 操作 | 说明 |
|------|------|
| `@机器人 帮我做 xxx` | 派任务：蓝卡片出现，进度 2s 节流原地刷新，结束变绿/红 |
| 发送截图 | 图片下载后作为多模态输入交给当前话题 Agent |
| 话题内继续追问 | 同一会话续接（`feishu_chats` 映射），上下文不断 |
| 高成本 quant 工具 | 弹确认卡片：点「确认」/「否」；文字回复「确认5次」可批量授权（N≤50） |
| `/new` | 关闭当前话题会话，下次发消息开新会话 |
| `/stop` | 取消当前执行中的任务 |
| `/status` | 查看当前话题的会话 ID 与状态 |
| `/nodes` | 列出当前绑定用户注册的 hank-cli 节点：hostname、在线/离线、work_dir、上报的 agent_backends |
| `/diff` | **仅 server worktree 会话**：查看相对生产基线的状态与 diff stat。**本机 CLI（client-only）会话会明确拒绝**，不会静默调用 server 部署逻辑 |
| `/test` | **仅 server worktree 会话**：按变更路径运行固定测试矩阵。**client-only 会话明确拒绝** |
| `/deploy` | **仅 server worktree 会话**：固化 commit 并发送部署审批卡。**client-only 会话明确拒绝** |
| `/rollback` | **仅 server worktree 会话**：回滚审批。**client-only 会话明确拒绝** |
| `/help` | 命令列表 |

### 本机 Agent CLI（client-only，默认代码执行路径）

`hank-cli` 启动时会探测本机 PATH 中的 `codex`、`claude`、`grok`、`kimi` 并把能力上报给 server。飞书新话题只要路由到这些外部后端（用户点名或任务分类为代码/文件任务），**必须**绑定一台在线且上报了对应 backend 的 `hank-cli` 节点；否则返回明确错误（例如「没有在线且支持 claude 的 hank-cli 节点」），**不会**回退到 server bubblewrap 或 native Agent。

**与 `[server_agent]` 解耦**：client-only 路径（`飞书 → server → hank-cli → 本机 CLI`）在 `[server_agent].enabled = false` 时同样可用。关闭 server_agent 时，路由仍会对新话题做 `agent_kind` / `agent_backend` 分类；代码/文件任务落到 hank-cli，缺节点时用户可见失败；纯对话 `conversation` 走 native 无工具。可用 `/nodes` 查看本机节点在线状态与 backends。

**conversation 会话上下文**：纯对话话题会在 system prompt 中注入飞书链路说明（话题固定 backend/节点、可用命令、本机 CLI 执行边界）以及**注入时刻**的 hank-cli 节点快照（hostname / 在线状态 / work_dir / backends），因此「现在有什么 cli 在线」类问题可直接据实回答，模型不得编造节点。conversation 会话**不绑定** `exec_client_id`、也无 `work_dir`（与 `WorkspaceKind::None` 一致）；即使本机有在线桌面 client，也不会挂远程执行工具，避免「有工作目录」与「纯对话无工具」的 prompt 冲突。

凭据和 CLI 配置留在本机，不上传 server。飞书不直连家庭网络，而是经 server 转发到 `hank-cli` 的出站长轮询，因此无需给电脑开公网端口。

`~/.hank-cli/config.toml` 建议显式指定允许访问的目录和后端：

```toml
server = "https://your-hank-server"
username = "admin"
password = "..."
work_dir = "/Users/you/projects"
agent_backends = ["codex", "claude", "grok", "kimi"]
```

在该目录或任意位置启动已安装的 `hank-cli`：

```bash
hank-cli
```

- **client-only 元数据**：会话写入 `agent_location=client`、`agent_backend`、`exec_client_id`；同一话题固定复用首次选择的 backend、节点和 CLI `thread_id`/`session_id`。切换后端或节点必须 `/new` 或新开话题。
- **节点离线 / 能力不匹配**：续聊时若绑定节点不在线，或 poll 上报的 `agent_backends` 不再包含该 backend，任务立即失败并提示用户在本机启动/修复 `hank-cli`，**不会**解绑后换节点，也**不会**落到 server 执行。
- **历史 server-agent 会话**：若话题仍映射到旧的 `server_agent=true` 且 backend 为 codex/claude/grok/kimi，server **拒绝静默复用**，要求发送 `/new` 转为 client-only 会话。
- **路径边界**：server **不会**把 wananyun 上的 worktree 绝对路径下发给本机；`hank-cli` 只在注册 `work_dir` 内解析 cwd（缺省即 work_dir 根）。Codex 另以 `workspace-write` 限制写入；Claude/Grok/Kimi 权限模式由各自 CLI 负责。`agent_backends` 是严格 allowlist，未知值会被丢弃。
- **取消与终态**：`/stop` 向同一节点下发 `agent_cancel` 并终止进程组；节点离线、结果通道中断或超时时卡片/回复会标失败，不吞错、不误报成功。
- **命令边界**：client-only 会话不支持 `/diff` `/test` `/deploy` `/rollback`（这些命令面向 server worktree 部署流程）。请在本机工作目录自行查看变更、跑测试或部署。

## 四、wananyun server Agent

不需要在 wananyun 维护两份手工同步的源码。目录职责如下：

```text
/opt/hank-src                         只保存 Git 生产基线 trace-production
/opt/hank-worktrees/<session-uuid>    每个飞书话题一个独立可写 worktree
/opt/hank-workspaces/<session-uuid>   与 Trace/quant 无关话题的普通隔离目录
/opt/hank*/releases/<deployment-id>   不可变运行 release
/opt/hank*/current                    systemd/nginx 当前版本链接
/opt/hank*/previous                   最近一个可回滚版本链接
/opt/hank/deploy-jobs                 审批后的结构化 manifest 与结果
```

可迭代范围是 `server/`、`crates/`、`admin/`、`cli/`、`quant/`、`docs/`；`client/` 在文件工具、shell 规则和部署前检查三处拒绝。`deploy/`、systemd、sudoers、`Makefile` 和配置模板属于部署基础设施，不能自部署，需走 SSH 应急入口。

### 首次初始化

首次仍需一次 SSH。先在本机提交实现并确认本地或线上已有生产 `config.toml`，然后执行：

```bash
make bootstrap-server-agent
make deploy
make deploy-cli       # 线上需要 hank-cli 时
make deploy-quant     # 线上需要 quant 时
make deploy-quant-slidev
```

`bootstrap-server-agent` 会创建两个账号：`hank` 运行 server 并持有审批权限，`hank-build` 只执行工作区 shell、测试和构建，不具备 root 部署权限。构建用户只信任生产基线和 server 为话题注册的具体 worktree。bootstrap 还会安装 root-owned `/usr/local/libexec/hank-deploy`；helper 只接受 UUID，从 `/opt/hank/deploy-jobs` 读取 manifest，并把任务转交给独立 systemd transient unit，因此更新 `hank-server` 本身不会中断部署。

bootstrap 由本机生成 Git bundle 并通过 SSH 上传，wananyun 不执行 `clone`、`fetch` 或 `push` GitHub。仓库的 `origin` URL 仅保留为 SSH break-glass 元数据。

生产配置最终应包含：

```toml
[server_agent]
enabled = true
repository_root = "/opt/hank-src"
worktrees_root = "/opt/hank-worktrees"
general_workspaces_root = "/opt/hank-workspaces"
base_ref = "trace-production"
deploy_jobs_dir = "/opt/hank/deploy-jobs"
deploy_helper = "/usr/local/libexec/hank-deploy"
execution_user = "hank-build"
agent_cli_root = "/opt/hank-agent-cli"
agent_state_root = "/opt/hank-agent-state"
agent_timeout_secs = 1800
agent_output_limit_bytes = 2097152
agent_sandbox_bin = "/usr/bin/bwrap"
deploy_use_sudo = true
approval_ttl_secs = 600
```

### 工作区路由（迁移后）

- `/help`、`help`、`?help`、`？help`、`帮助`、`/nodes` 等命令不创建会话工作区。
- 同一飞书话题已有 `feishu_chats` 映射时：
  - **client-only**（`agent_location=client`）：固定复用 backend / `exec_client_id` / `agent_thread_id`，不重新分类、不换节点。
  - **历史 server-agent + 外部 backend**：拒绝复用，要求 `/new`。
  - **native conversation** 等非外部后端 managed 会话：可继续复用。
- 新话题**始终**由路由 Agent 选择 `conversation`、`trace_code`、`quant_code` 或 `general_task`，同时选择 `native`、`codex`、`claude`、`grok` 或 `kimi`（**即使 `[server_agent].enabled = false`**）。`conversation` 强制 `native`；其他任务若误返回 `native` 会归一为默认 CLI 后端（优先 Codex，否则 Claude）。用户点名后端时保留选择。`server_agent` 关闭时 prompt 会明确告知无 server 工作区，代码任务只能落到 hank-cli。
- **外部 backend（codex/claude/grok/kimi）只走 client-only**：必须命中在线且上报该能力的 `hank-cli`；否则用户可见失败。**飞书不再创建 server bubblewrap / worktree 代码会话**（server-only 实现仍保留编译兼容，但不由飞书入口创建）。
- `native` 对话不创建代码工作区；client-only 会话的工作目录是节点 `work_dir`，不在 wananyun 上建 worktree。conversation 会话会注入链路说明与节点快照（见上文「本机 Agent CLI」），且不绑定 `exec_client_id` / `work_dir`。
- `/nodes` 列出当前用户注册的 hank-cli 节点（在线状态、work_dir、backends）；无节点时给出安装/启动提示。
- `/diff`、`/test`、`/deploy`、`/rollback` 仅适用于仍存在的 server worktree 管理会话；对 client-only 会话返回「本机 CLI 会话不支持该命令 / 请在本机执行」，**不会误报成功或静默操作 server 部署**。
- 本机 CLI 的 stdout 按 JSONL 解析，线程 ID 写回 metadata；server 处理超时、输出上限、`/stop` 取消与飞书终态卡片。节点离线或结果超时时明确失败。

### 离线安装 Codex 与 Claude Code

在可访问 npm registry 的本机执行：

```bash
make install-agent-clis
```

脚本固定下载 `@openai/codex` 与 `@anthropic-ai/claude-code-linux-x64` 的 Linux x64 原生制品，先校验 npm 发布元数据中的 SHA-1，再生成 SHA-256 清单连同制品传到 wananyun；远端复验后安装到 `/opt/hank-agent-cli` 并原子更新 `current` 链接。wananyun 不访问 GitHub，也不在线下载 CLI。

凭据按三级优先级解析，每轮任务读一次：admin「Agent CLI」页（`agent_cli_profiles` 表，每后端可存多份命名配置、同时启用一份，切换即时生效）→ wananyun 的 `/opt/hank/agent-cli.env`（`root:hank 0640`，可配置 `OPENAI_API_KEY`、`OPENAI_BASE_URL`、`OPENAI_MODEL`、`ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN` 或 `CLAUDE_CODE_OAUTH_TOKEN`）→ server 中已启用的 provider 记录。Claude 可复用已启用的 Anthropic provider；Codex 只自动复用官方 `api.openai.com` provider，因为 Codex 0.146 只支持 Responses API，普通 Chat Completions 兼容网关不能直接复用。第三方 Responses 兼容端点必须在 admin 或环境文件中显式配置 key 和 base URL，并由 CLI 启动器组装独立的 custom Responses provider，不能只覆盖内置 OpenAI provider 的 base URL。环境文件不回传、不进 Git，凭据不写入命令行、sudo 环境、session metadata 或日志；修改环境文件后需重启 `hank-server` 让 systemd 重新载入，改 admin 配置则不需要。

本机 CC Switch 不安装到无 GUI 的 wananyun。需要复用本机已经生效的 Claude Code / Codex 第三方 API 配置时执行：

```bash
make sync-agent-cli-config
```

文件范围、远端路径、权限和共同维护规则统一见
[`Server Agent 双向 Git 同步协议`](src/operations/server-agent-sync.md#claude-code--codex-配置同步)。

### 日常流程

1. 在飞书新话题描述需求，server 按上面的规则选择工作区。
2. 继续在同一话题补充、修正；用 `/diff` 检查范围。
3. 用 `/test` 跑固定测试矩阵。
4. 用 `/deploy` 生成 10 分钟有效的审批卡，确认目标和 diff stat 后点击部署。
5. helper 从审批 commit 导出一次性源码快照，先构建非 core 目标，最后切换 core；每个目标切换后检查服务或 HTTP 健康状态。
6. 任一步失败只回滚本次已切换目标；结果在 server 重启后仍会回到原飞书话题。

并发部署由全局 `flock` 串行化。审批 commit 必须仍基于当前 `trace-production`，基线已前进时旧话题会被拒绝，需在新话题重新应用变更。

### 离线 Git 回传

本机与 wananyun 的分支职责、拉回命令、分叉处理、安全边界和协议维护方式统一见
[`Server Agent 双向 Git 同步协议`](src/operations/server-agent-sync.md)。该文件是唯一事实来源，本指南不重复维护第二套步骤。

### quant 迁移

常规 `/test`、`/deploy` 和 `/rollback` 都不会执行 Alembic。检测到 `quant/alembic/` 变更会直接拒绝部署；维护窗口中先通过 SSH 备份并执行 `uv run alembic upgrade head`，确认 schema 后再部署应用。此例外是刻意保留的 break-glass 流程。

## 五、与 agent-os 文档的对应与差异

| 文档做法 | 本项目实现 | 原因 |
|---------|-----------|------|
| Node.js + `@larksuiteoapi/node-sdk` | 纯 Rust 手写 pbbp2 帧（prost）+ REST | 社区 SDK（open-lark 0.14）不转发 `card` 类型帧，确认按钮回调收不到 |
| headless `claude -p` 做唯一执行引擎 | native / Codex / Claude Code 按话题固定路由 | 对话禁用工具；代码任务可复用 CLI 原生上下文，并由 server 统一管理工作区、取消和终态 |
| 会话存 `data/sessions.json` | `feishu_chats` 表 + server 会话本就在 DB | 天然解决持久化与重启恢复 |
| 文本回复确认 | 卡片按钮确认 | 文档后期审批篇的形态，提前落地 |

## 六、故障排查

- **收不到消息**：检查事件订阅是否选了长连接、`im.message.receive_v1` 是否添加、应用是否已发布、机器人是否在群里；群里消息必须 @机器人（权限是"群聊中被 @ 的消息"）
- **按钮点了没反应**：检查回调订阅是否也开了长连接并添加 `card.action.trigger`
- **回复"请先生成绑定码"**：Trace client → 设置 → 飞书绑定 → 生成绑定码，发给机器人 `bind 123456`
- **日志看连接状态**：`feishu monitor started` / `feishu ws connected, service_id=...`；断线会指数退避重连（1s→30s）
- **常见错误码**：`code=99991663` token 失效（自动刷新）；权限类错误回开放平台检查权限范围
- **部署启动失败**：检查 `sudo -l -U hank`、`/usr/local/libexec/hank-deploy` 权限和 `/opt/hank/deploy-jobs`
- **构建用户报权限错误**：检查 `hank`、`hank-build` 都在 `hank-workspace` 组，worktree 目录应为 setgid/group-writable；旧版 Git 的 `safe.directory` 必须注册具体 worktree 路径，不能使用 `*` 通配
- **部署日志**：`journalctl -u 'hank-deploy-*'`；server/CLI/quant 分别看对应 systemd unit
- **应急回退**：飞书不可用时通过 SSH 将目标的 `current` 原子切到 `previous` 并重启服务；SSH 始终保留为 break-glass

## 七、定时任务（系统主动推送）

`server/src/scheduler/`：cron 调度器（上海时区），agent-os 文档"自动化工作流"的落地。
管理入口在 admin「定时任务」页：查看调度状态/下次执行时间、启停、手动触发、执行记录。

- 任务定义在代码（`JOB_DEFS` 注册表），启停状态在 `job_states` 表，执行日志在 `job_runs` 表（镜像 quant 的 `quant_job_run` 模型）
- 多实例共库只能一个实例开调度：`[server] scheduler_enabled = false` 关闭其他实例
- 手动触发与系统调度走同一执行路径，不绕过任务内部守卫；每 job 一把并发锁；进程重启遗留的"执行中"记录启动时自动收尾为失败

**当前注册的 job**：

| id | 调度 | 说明 |
|----|------|------|
| `quant_signal_brief` | 工作日 17:45 | 盘后信号简报：逐绑定用户签内部 JWT 调 quant `/api/signals?date=today`（策略可见性过滤天然生效），有信号推飞书单聊，无信号保持安静 |

新增 job 的步骤：在 `scheduler/jobs.rs` 写 handler（返回 JSON 结果），在 `JOB_DEFS` 注册 cron，admin 页面自动出现。

## 八、后续（未实现）

- 普通文件附件下载（当前已支持图片输入和 `[file:]` 图片回传）
- 更多 job：agent 整理的简报（cron 驱动 run_chat_turn）、失败 @人告警、巡检类任务
- 多 bot 互相 @ 协作（agent-os 文档后半程的团队作战）
