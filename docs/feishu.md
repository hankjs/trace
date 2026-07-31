# 飞书渠道接入指南

飞书渠道让 server 的 agent 直接挂到飞书群里：话题群里 @机器人 派任务，进度以卡片原地刷新，高成本操作弹确认卡片，点按钮即拍板。实现参考 `docs/book/agent-os` 课程文档，但用纯 Rust 复刻（`server/src/feishu/`），执行引擎复用 server 自己的 agent（`chat::run_chat_turn`），不是 headless Claude Code。

## 架构

```
飞书开放平台 ←─ WS 长连接(pbbp2 protobuf 帧) ─→ feishu/ws.rs
        │ im.message.receive_v1 / card.action.trigger
        ▼
feishu/router.rs（消息解析、话题=会话、/命令、派发）
        │ run_chat_turn（session metadata.source = "feishu"）
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
- **执行模式**：默认仍可绑定在线桌面 client；开启 `[server_agent]` 后，新话题按任务意图选择 Trace/quant Git worktree 或普通隔离目录，完全不依赖 `client/`
- **管理员边界**：server Agent 只接受绑定到 `can_login_admin = true` 用户的飞书消息

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
| `/diff` | 查看当前 worktree 相对生产基线的状态与 diff stat |
| `/test` | 按变更路径运行固定测试矩阵，可用 `/stop` 取消 |
| `/deploy` | 固化 commit 并发送部署审批卡；只有发起管理员可批准 |
| `/rollback` | 对最近一次成功部署发送回滚审批卡，切回 previous release 后健康检查 |
| `/help` | 命令列表 |

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
deploy_use_sudo = true
approval_ttl_secs = 600
```

### 工作区路由

- `/help`、`help`、`?help`、`？help`、`帮助` 等命令不创建会话工作区。
- 同一飞书话题已有 `feishu_chats` 映射时，始终复用原 session 和原工作区，不重新分类。
- 新话题会先由路由 Agent 选择 `conversation`、`trace_code`、`quant_code` 或 `general_task`，并把 `agent_backend`、`agent_kind`、`workspace_kind` 写入 session metadata。`trace_code`/`quant_code` 从 `trace-production` 创建 Git worktree，`general_task` 创建普通隔离目录，`conversation` 不创建目录；同一话题后续固定复用该路由结果。
- 只有 Git worktree 会注入 Trace/quant/同步协议，并支持 `/diff`、`/test`、`/deploy`、`/rollback`；普通隔离目录不能部署。

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
| headless `claude -p` 做执行引擎 | 复用 server 自己的 agent | server 已有完整 agent（quant 工具链/确认闸门/远程执行），第二引擎接不进去 |
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
