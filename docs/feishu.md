# 飞书渠道接入指南

飞书渠道让 server 的 agent 直接挂到飞书群里：话题群里 @机器人 派任务，进度以卡片原地刷新，高成本操作弹确认卡片，点按钮即拍板。入口和生命周期由 Rust server 统一管理（`server/src/feishu/`）；纯对话使用无工具的 native Agent，**quant 研究话题**（`quant_research`）使用 server 侧 native 会话并挂载 `quant_*` 工具（需 `quant_a2a.enabled`），**代码与文件任务的 Codex / Claude Code / Grok / Kimi Code 必须在用户本机在线 `hank-cli` 节点执行（client-only）**，server 不再 bubblewrap 回退执行，也不再由 wananyun 自行修改/部署 Trace 源码。

## 架构

```
飞书开放平台 ←─ WS 长连接(pbbp2 protobuf 帧) ─→ feishu/ws.rs
        │ im.message.receive_v1 / card.action.trigger
        ▼
feishu/router.rs（消息解析、话题=会话、/命令、派发）
        │ run_chat_turn（native）/ cli_agent::run_cli_turn（本机 CLI）
        ▼
feishu/pusher.rs（事件流 → 任务卡片 2s 节流刷新；终态卡可挂「查看详情」等按钮）
        │ AskUser → agent_interactions 落表 → 确认卡片 / task_gate 大卡片
        ▼
feishu/callback.rs（按钮回调 → interaction_flow::answer_and_resume；
                    终态卡 task_detail / task_suggest）
        │ 原子应答 → 在交互单冻结的 session 上 resume
        ▼
interaction_flow.rs（quant_confirm / ask_user → run_chat_turn；
                     task_gate → cli_agent resume 第二轮）
```

- **话题 = 会话**：`feishu_chats` 表把 `account_id:chat_id:topic_id`（topic = thread_id || root_id || "main"）映射到 server session，重启不丢
- **账号管理**：凭证存 `feishu_accounts` 表，admin REST 增删启停（与 weixin_accounts 同模式）；启用即起长连接，停用即断
- **用户绑定**：`feishu_bindings` 表，一次性 6 位绑定码流程（与微信相同），无需手配 open_id
- **任务进度卡**：标题带任务摘要（超长截断到 24 字）；终态绿卡可挂「查看详情」等按钮。按钮 callback value 只带 `feishu_card_actions` 主键 id，真正的详情全文 / 建议动作 prompt 存服务端（避免客户端改写指令、也避免 value 超长），启动时清理 30 天前的 payload
- **确认闸门 / 交互单落表**：`quant_confirm` 与 `ask_user` 统一写入 `agent_interactions` 表（有稳定主键），不再寄生在进程内 map 或 `sessions.pending_ask_user`（历史字段，已无读写路径）。飞书确认卡片展示任务编号、会话短 id 与 admin 深链；按钮回调按 `interaction_id` 原子应答，并在交互单冻结的 `session_id` 上 resume——话题 reuse policy 判 Recreate 重建 session 后点确认也不会丢单。微信仍是文本白名单（回复"确认"），TTL 写在行的 `expires_at`（微信 5 分钟，飞书/网页不过期）。交互单进入终态（应答 / 取代 / 取消 / 过期）时会同步把飞书卡片改成灰色终态，四条路径共用 `patch_card_to_done`，标题走 `interaction_card_title` 统一约定；admin 手动应答与取消同样会改卡（按交互单的 `account_id` 解析账号），改卡失败只记日志、不影响库状态。多问题 ask_user 的部分应答停在 `pending` 并写 `resume_ref.partial_answers`，不引入新状态
- **交互单管理入口**：`server/src/interactions.rs`（admin REST：列表 / 详情 / 手动应答 / 取消）与 `server/src/interaction_flow.rs`（应答派发；飞书按钮与 admin 手动应答共用同一条链路，避免顺序漂移）
- **两阶段任务闸门（task_gate）**：见下文「两阶段任务闸门」小节。默认关闭（`[server_agent].task_gate_enabled`），与 `server_agent.enabled` 解耦。
- **执行模式**：新话题始终由路由 Agent 确定 `agent_kind` 与 `agent_backend`（**不依赖** `[server_agent].enabled`）。五种 `agent_kind`：`conversation`（纯对话、无工具）、`quant_research`（A 股研究、仅 quant 工具，需 `quant_a2a.enabled`）、`trace_code` / `quant_code` / `general_task`（代码与文件任务）。`codex` / `claude` / `grok` / `kimi` **一律 client-only**：必须绑定在线且上报了对应 backend 的 `hank-cli`；节点不存在、离线或能力不匹配时直接失败，**绝不**回退 server bubblewrap、native 或另一节点。`[server_agent].enabled` 只管 server 侧 worktree / bubblewrap / `/diff` `/test` `/deploy` `/rollback`；关闭时 client-only hank-cli 链路仍然可用。`conversation` 与 `quant_research` 强制 `native`：前者无工具，后者只挂 `quant_*` + `ask_user` + `web_fetch`，均无工作区、不绑 `exec_client_id`。
- **管理员边界**：`can_login_admin` 仅在创建 **server 侧** native / worktree **工作区**时校验；纯对话、`quant_research` 与 client-only hank-cli 用户不要求 admin

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

**admin 深链**（可选）：卡片上的「在 admin 查看」链接需要 server 知道 admin 的
外部地址，在 `config.toml` 配置：

```toml
[server]
admin_base_url = "https://your-host"   # 留空则卡片不渲染深链行
```

深链格式 `{admin_base_url}/admin/interactions/{id}`（admin 是 history 路由，base path `/admin/`）。
本地 dev 通常留空。

## 三、用法

| 操作 | 说明 |
|------|------|
| `@机器人 帮我做 xxx` | 派任务：蓝卡片出现（标题带任务摘要），进度 2s 节流原地刷新，结束变绿/红；完成后可点「查看详情」在话题内获取完整总结（不受进度卡长度限制） |
| 终态卡建议动作 | agent 收尾时若调用 `suggest_actions`，绿卡上会出现最多 3 个自拟按钮；点击即以该建议为指令起新一轮（新蓝卡） |
| 直接问行情 / 信号 / 回测 | 路由到 `quant_research`（需 `quant_a2a.enabled`）：server 侧 native，挂 `quant_*` 工具，无代码工作区 |
| 发送截图 | 图片下载后作为多模态输入交给当前话题 Agent |
| 话题内继续追问 | 同一会话续接（`feishu_chats` 映射），上下文不断 |
| 高成本 quant 工具 | 弹确认卡片三按钮：「确认」/「本会话全部同意」（等价「确认50次」）/「否」；也可文字回复「确认N次」（N≤50）批量授权（微信无批量） |
| 多问题 ask_user | agent 一次问多题时出多问题卡：可**逐题点按钮**（点后卡片刷新，已答显示 ✓，全部答完才 resume），或文字一次回「1A 2B」。格式错误会提示、交互单保持 pending 可重答。部分应答存在 DB，**跨重启仍有效** |
| `/new` | 关闭当前话题会话，下次发消息开新会话 |
| `/stop` | 取消当前执行中的任务 |
| `/status` | 查看当前话题的会话 ID 与状态 |
| `/nodes` | 列出当前绑定用户注册的 hank-cli 节点：hostname、在线/离线、work_dir、上报的 agent_backends |
| `/diff` | **仅 server worktree 会话**：查看相对生产基线的状态与 diff stat。**本机 CLI（client-only）会话会明确拒绝**，不会静默调用 server 部署逻辑 |
| `/test` | **仅 server worktree 会话**：按变更路径运行固定测试矩阵。**client-only 会话明确拒绝** |
| `/deploy` | **仅 server worktree 会话**：固化 commit 并发送部署审批卡。**client-only 会话明确拒绝** |
| `/rollback` | **仅 server worktree 会话**：回滚审批。**client-only 会话明确拒绝** |
| `/help` | 命令列表 |

### 多问题 ask_user

`ask_user` 支持两种入参（向后兼容，至少传一组）：

- **单问题**（旧）：`{question, options}` —— 一行按钮，点一下即应答并 resume。
- **多问题**（新）：`{questions: [{id, question, options}, ...]}` —— 最多 5 题、每题最多 4 选项；id 限 `[A-Za-z0-9_-]` 且 ≤8 字符。

飞书多问题卡：

1. **逐题点选**：按钮文案形如 `1A main`；点第 1 题后卡片原地刷新为「✓ 1A main」，其余题仍可点。`partial_answers` 用 MySQL `JSON_SET` 原子写入，状态仍是 `pending`。**所有题答完**才走 `answer_and_resume` 整单应答并 resume agent。
2. **文字一次答完**：直接回复 `1A 2B`（大小写不敏感，中英文逗号/空格/无分隔均可）。漏题、越界、乱串会得到中文提示，交互单保持 pending，可重答；不会把垃圾串塞给模型。

部分应答跨重启：`resume_ref.partial_answers` 在 DB，卡片还在飞书侧；服务重启后继续点仍然有效（状态机未引入新中间态，仍是 `pending`）。`answer` 列 `VARCHAR(64)` 对超长完整串会截断，完整版在 `resume_ref.final_answer`。

微信走文字路径，自动获得「1A 2B」解析能力；不专门做多问题卡渲染。

### 两阶段任务闸门（task_gate）

默认**关闭**。在 `config.toml` 的 `[server_agent]` 段设置：

```toml
[server_agent]
# 与 enabled 无关：client-only 在 enabled=false 时同样可开闸门
task_gate_enabled = true
```

开启后，飞书派**代码任务**（`trace_code` / `quant_code` / `general_task`）时不再直接改码，而是：

1. **第一轮**：CLI 只读分析（靠 prompt 约束；CLI 以 bypass-approvals 启动，写操作不会被沙箱拦住），产出 `## 目标` / `## 范围` / `## 疑似改动点` / `## 风险`。
2. 落 `kind=task_gate` 交互单，发大卡片（任务编号、目标、基本信息、分析全文、`开始修` / `跳过`）。进度卡同时落「等待确认」灰色终态。
3. 用户点 **开始修** → 在同一 CLI thread 上 resume 第二轮真正改代码；点 **跳过** → 交互单 `cancelled`，**不继续执行**（不保证第一轮无副作用——若模型未听话已改文件，卡片会提示「第一轮已产生 N 个文件改动」，回滚由用户自行 git 处理）。

「第一轮已产生 N 个文件改动」是**增量**口径：第一轮开跑前先取一次 `git status --porcelain` 作基线，事后做差集，因此用户本机原有的未提交改动不会被算进来。查不到时（节点离线 / 非 git 目录）不显示这行，不会把「不知道」说成「没改动」。

**不走闸门**：开关关闭、`conversation` / `quant_research`、非飞书来源、话题已有 `agent_thread_id`（续聊）。第一轮若拿不到 CLI `thread_id` 则无法续接，终态会明确说明「本轮只做了只读分析，没有修改代码」，需要执行请再发一次指令。

**闸门单的生命周期**：同一会话开始新一轮时，仍 `pending` 的旧闸门单会被标 `cancelled`（卡片改灰），避免用户过很久回头点老卡片，在上下文已跑偏的 thread 上 resume。第二轮派发前会用 `resume_ref.thread_id` 校正 session 的 `agent_thread_id`：一致则直接 resume；session 丢了 thread（话题会话被重建）则写回；已分叉则把 `analysis` 全文注入第二轮 prompt 兜底。服务重启时滞留在 `executing` 的闸门单会被标 `failed`，不会永远卡在中间态。

admin `/interactions` 可筛 `kind=task_gate` 查看 `goal` / `analysis` 全文；手动应答与飞书按钮共用 `answer_and_resume`。

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
- **输出闸门**：CLI 输出分两级限制。单行超 `agent_max_line_mib`（默认 1 MiB）只丢该行并继续执行——`stream-json` 下一条 tool_result 可能带整个文件内容，探索型任务撞线是常态，不按异常处理；整流累计超 `agent_max_stream_mib`（默认 64 MiB）才是 runaway，终止进程组并标失败。回传 server 的 stdout 只保留**尾部** 256 KiB（`final_text` 兜底解析要的 `result` 事件在流末尾）。超限失败时已产出的 `final_text` 会先落库成 assistant 消息再报错，不整轮丢弃。
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
agent_output_limit_bytes = 67108864
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
- 新话题**始终**由路由 Agent 选择 `conversation`、`quant_research`（仅 `quant_a2a.enabled` 时）、`trace_code`、`quant_code` 或 `general_task`，同时选择 `native`、`codex`、`claude`、`grok` 或 `kimi`（**即使 `[server_agent].enabled = false`**）。`conversation` 与 `quant_research` 强制 `native`；其他任务若误返回 `native` 会归一为默认 CLI 后端（优先 Codex，否则 Claude）。用户点名后端时保留选择。`server_agent` 关闭时 prompt 会明确告知无 server 工作区，代码任务只能落到 hank-cli。`quant_a2a` 关闭时路由**不会**产出 `quant_research`。
- **外部 backend（codex/claude/grok/kimi）只走 client-only**：必须命中在线且上报该能力的 `hank-cli`；否则用户可见失败。**飞书不再创建 server bubblewrap / worktree 代码会话**（server-only 实现仍保留编译兼容，但不由飞书入口创建）。
- `native` 对话与 `quant_research` 均不创建代码工作区；client-only 会话的工作目录是节点 `work_dir`，不在 wananyun 上建 worktree。conversation 会话会注入链路说明与节点快照（见上文「本机 Agent CLI」），且不绑定 `exec_client_id` / `work_dir`。`quant_research` 同样不绑定执行节点，只挂 quant 研究工具，高成本操作走确认卡片闸门。
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

## 六、团队任务流水线

代码任务从单角色两阶段（分析 → 闸门 → 执行）扩展成 **开发 → 评审 → 测试**
串行流水线，每个角色独占 CLI thread，角色间用产物交接、不共享上下文。
设计见 `docs/feature/team-task-pipeline.md`，实现在 `server/src/team_task/`。

**开关**：admin「团队任务」页（`/admin/team-task`）。改完即时生效、无需重启
（配置存 DB，优先于 `config.toml`）。流水线入口依赖两阶段闸门
（`task_gate_enabled`）：分析轮结束后落闸门卡，点「开始修」才进编排器。

**流转（简化）**：

```
pending_confirm → running_developer → [pending_review_gate?] → running_reviewer
       │                                      │ reject
       └ 跳过 → cancelled                     ↓
                                    pending_dev_gate? / running_developer (round+1)
                                              │ pass
                                              ↓
                              [pending_test_gate?] → running_tester → done
```

闸门边界由配置 `gates` 控制；默认只开 `dev_start`，其余自动流转。
打回上限 `max_dev_rounds`（默认 3），触顶进 `failed`。

**角色**：

| 角色 | 做什么 | thread |
|------|--------|--------|
| developer | 改代码，输出变更摘要 | 独占新 thread |
| reviewer | 只读评审，verdict=pass/reject | 独占新 thread |
| tester | 只读验证，verdict=pass/reject | 独占新 thread |

**看板**：独立前端 `team/`，开发端口 **18789**（`cd team && pnpm dev`）。
深链格式 `{dashboard_base_url}/#/team/{task_no}`（hash 路由，与 admin 的
history 托管方式不同）。主卡上的「在看板查看」链接与此一致；
`dashboard_base_url` 未配置时主卡不渲染该行。

**飞书主卡**：点「开始修」后 reply 闸门卡生成，整条流水线原地刷新
（流转记录 + 当前进展）。依赖 pusher 发闸门卡时回填的 `origin_message_id`。

## 七、故障排查

- **收不到消息**：检查事件订阅是否选了长连接、`im.message.receive_v1` 是否添加、应用是否已发布、机器人是否在群里；群里消息必须 @机器人（权限是"群聊中被 @ 的消息"）
- **按钮点了没反应**：检查回调订阅是否也开了长连接并添加 `card.action.trigger`
- **回复"请先生成绑定码"**：Trace client → 设置 → 飞书绑定 → 生成绑定码，发给机器人 `bind 123456`
- **日志看连接状态**：`feishu monitor started` / `feishu ws connected, service_id=...`；断线会指数退避重连（1s→30s）
- **常见错误码**：`code=99991663` token 失效（自动刷新）；权限类错误回开放平台检查权限范围
- **部署启动失败**：检查 `sudo -l -U hank`、`/usr/local/libexec/hank-deploy` 权限和 `/opt/hank/deploy-jobs`
- **构建用户报权限错误**：检查 `hank`、`hank-build` 都在 `hank-workspace` 组，worktree 目录应为 setgid/group-writable；旧版 Git 的 `safe.directory` 必须注册具体 worktree 路径，不能使用 `*` 通配
- **部署日志**：`journalctl -u 'hank-deploy-*'`；server/CLI/quant 分别看对应 systemd unit
- **应急回退**：飞书不可用时通过 SSH 将目标的 `current` 原子切到 `previous` 并重启服务；SSH 始终保留为 break-glass
- **点了确认卡片没反应**：先看 admin「交互单」页找到该任务编号，看 `status`。
  `pending` 说明应答没写进去（多为节点离线，卡片会带提示）；`answered` 长期不动
  说明派发未完成（server 重启会自动退回 `pending` 可重试）；`cancelled` 是被取消。
  卡死的交互单可以在 admin 里直接手动应答，不必 `/new` 重开话题。
- **确认卡片提示「这个操作已经提交过了」**：同一张卡片只能应答一次
  （按 `interaction_id` 原子抢答）。若确实需要重跑，让 agent 重新发起工具调用。
- **闸门大卡片不出现**：确认 `[server_agent].task_gate_enabled = true`；
  且话题必须是新话题（已有 `agent_thread_id` 的续聊不弹闸门）、
  来源是飞书、`agent_kind` 是代码类。第一轮拿不到 CLI `thread_id` 时不落闸门
  （日志有 warn），终态文案会说明「本轮只做了只读分析，没有修改代码」。
- **闸门卡片变灰、写着「已被新一轮取代」**：闸门挂着期间同会话又派了新任务，
  旧闸门单已作废。这是有意行为——在跑偏的 thread 上 resume 比重新派单更糟。
- **团队任务主卡不出现**：检查任务是否有 `origin_message_id`
  （闸门卡片没发成功时为空；`card_message_id` 由首个角色派发时 reply 生成）。
- **评审 verdict 是 unknown、任务莫名 failed**：模型没按格式输出交接段，
  看 `team_task_runs.summary` / `handoff` 原文；Unknown 一律 failed、不开闸门。
- **任务卡在 `running_*`**：可能是 run 终态回调丢了（channel fire-and-forget，
  进程被杀会丢）。重启后会被 `fail_stale_team_tasks` 标 failed，在看板点重试。
- **打回反复到上限**：`max_dev_rounds` 触顶是有意行为，需人工接手。

## 八、定时任务（系统主动推送）

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

## 九、后续（未实现）

- 普通文件附件下载（当前已支持图片输入和 `[file:]` 图片回传）
- 更多 job：agent 整理的简报（cron 驱动 run_chat_turn）、失败 @人告警、巡检类任务
- 多 bot 互相 @ 协作（agent-os 文档后半程的团队作战）
- admin 交互单详情的 `analysis` 渲染 markdown（当前纯文本展示）
