# 飞书渠道接入指南

飞书渠道让 server 的 agent 直接挂到飞书群里：话题群里 @机器人 派任务，进度以卡片原地刷新，高成本操作弹确认卡片，点按钮即拍板。入口和生命周期由 Rust server 统一管理（`server/src/feishu/`）。

执行形态只有 server 侧 native：纯对话无工具，**quant 研究话题**（`quant_research`）挂载 `quant_*` 工具（需 `quant_a2a.enabled`）。外部代码 Agent（Codex / Claude Code / Grok / Kimi）、用户本机 `hank-cli` 执行节点、server 侧 worktree / 沙箱与自助部署**均已下线**——server 不在任何机器上执行代码任务。

## 架构

```
飞书开放平台 ←─ WS 长连接(pbbp2 protobuf 帧) ─→ feishu/ws.rs
        │ im.message.receive_v1 / card.action.trigger
        ▼
feishu/router.rs（消息解析、话题=会话、/命令、派发）
        │ run_chat_turn（native，server 本地）
        ▼
feishu/pusher.rs（事件流 → 任务卡片 2s 节流刷新；首响卡由 router 创建、pusher 复用同一 card_message_id；终态卡可挂「查看详情」等按钮）
        │ AskUser → agent_interactions 落表 → 确认卡片
        ▼
feishu/callback.rs（按钮回调 → interaction_flow::answer_and_resume；
                    终态卡 task_detail / task_suggest）
        │ 原子应答 → 在交互单冻结的 session 上 resume
        ▼
interaction_flow.rs（quant_confirm / ask_user → run_chat_turn）
```

- **话题 = 会话**：`feishu_chats` 表把 `account_id:chat_id:topic_id`（topic = thread_id || root_id || "main"）映射到 server session，重启不丢
- **账号管理**：凭证存 `feishu_accounts` 表，admin REST 增删启停（与 weixin_accounts 同模式）；启用即起长连接，停用即断
- **用户绑定**：`feishu_bindings` 表，一次性 6 位绑定码流程（与微信相同），无需手配 open_id
- **任务进度卡**：标题带任务摘要（超长截断到 24 字）。新话题由 router 秒回「已收到」卡，pusher 复用同一 `card_message_id` 原地更新为运行中→终态（不新增消息）。终态绿卡可挂「查看详情」等按钮（schema 2.0 下 button 直接作为 body element，`update_card` 不接受 `tag:action`）。按钮 callback value 只带 `feishu_card_actions` 主键 id，真正的详情全文 / 建议动作 prompt 存服务端（避免客户端改写指令、也避免 value 超长），启动时清理 30 天前的 payload
- **确认闸门 / 交互单落表**：`quant_confirm` 与 `ask_user` 统一写入 `agent_interactions` 表（有稳定主键），不再寄生在进程内 map 或 `sessions.pending_ask_user`（历史字段，已无读写路径）。飞书确认卡片展示任务编号、会话短 id 与 admin 深链；按钮回调按 `interaction_id` 原子应答，并在交互单冻结的 `session_id` 上 resume——话题 reuse policy 判 Recreate 重建 session 后点确认也不会丢单。微信仍是文本白名单（回复"确认"），TTL 写在行的 `expires_at`（微信 5 分钟，飞书/网页不过期）。交互单进入终态（应答 / 取代 / 取消 / 过期）时会同步把飞书卡片改成灰色终态，四条路径共用 `patch_card_to_done`，标题走 `interaction_card_title` 统一约定；admin 手动应答与取消同样会改卡（按交互单的 `account_id` 解析账号），改卡失败只记日志、不影响库状态。多问题 ask_user 的部分应答停在 `pending` 并写 `resume_ref.partial_answers`，不引入新状态
- **交互单管理入口**：`server/src/interactions.rs`（admin REST：列表 / 详情 / 手动应答 / 取消）与 `server/src/interaction_flow.rs`（应答派发；飞书按钮与 admin 手动应答共用同一条链路，避免顺序漂移）
- **执行模式**：新话题由路由 Agent 确定 `agent_kind`。`conversation`（纯对话、无工具）与 `quant_research`（A 股研究，只挂 `quant_*` + `ask_user` + `web_fetch`，需 `quant_a2a.enabled`）都是 server 侧 native、无工作区。代码与文件任务已无执行通道：路由到这类意图时只做对话与分析，不承诺改码或部署。历史 `codex` / `claude` / `grok` / `kimi` 会话不静默复用，会提示 `/new` 重开。
- **管理员边界**：`can_login_admin` 仅在 `[server_agent].enabled` 开启、创建 server 侧会话时校验；纯对话与 `quant_research` 用户不要求 admin

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
| `@机器人 帮我做 xxx` | 派任务：新话题先秒回一张「已收到」卡片（路由分类需调 LLM，约 1 分钟），同一张卡片随后原地更新为运行中→终态，不新增消息；进度 2s 节流原地刷新，结束变绿/红；完成后可点「查看详情」在话题内获取完整总结（不受进度卡长度限制） |
| 终态卡建议动作 | agent 收尾时若调用 `suggest_actions`，绿卡上会出现最多 3 个自拟按钮；点击即以该建议为指令起新一轮（新蓝卡） |
| 直接问行情 / 信号 / 回测 | 路由到 `quant_research`（需 `quant_a2a.enabled`）：server 侧 native，挂 `quant_*` 工具，无代码工作区 |
| 发送截图 | 图片下载后作为多模态输入交给当前话题 Agent |
| 粘贴代码块 / 链接 | post 富文本里的 `code_block`、`code`、`a`、`md` 都会作为正文收下，`br` 转换行——可以直接把报错日志粘进来 |
| 话题内继续追问 | 同一会话续接（`feishu_chats` 映射），上下文不断 |
| 高成本 quant 工具 | 弹确认卡片三按钮：「确认」/「本会话全部同意」（等价「确认50次」）/「否」；也可文字回复「确认N次」（N≤50）批量授权（微信无批量） |
| 多问题 ask_user | agent 一次问多题时出多问题卡：可**逐题点按钮**（点后卡片刷新，已答显示 ✓，全部答完才 resume），或文字一次回「1A 2B」。格式错误会提示、交互单保持 pending 可重答。部分应答存在 DB，**跨重启仍有效** |
| `/new` | 关闭当前话题会话，下次发消息开新会话 |
| `/stop` | 取消当前执行中的任务 |
| `/status` | 查看当前话题的会话 ID 与状态 |
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

## 四、与 agent-os 文档的对应与差异

| 文档做法 | 本项目实现 | 原因 |
|---------|-----------|------|
| Node.js + `@larksuiteoapi/node-sdk` | 纯 Rust 手写 pbbp2 帧（prost）+ REST | 社区 SDK（open-lark 0.14）不转发 `card` 类型帧，确认按钮回调收不到 |
| headless `claude -p` 做唯一执行引擎 | native / Codex / Claude Code 按话题固定路由 | 对话禁用工具；代码任务可复用 CLI 原生上下文，并由 server 统一管理工作区、取消和终态 |
| 会话存 `data/sessions.json` | `feishu_chats` 表 + server 会话本就在 DB | 天然解决持久化与重启恢复 |
| 文本回复确认 | 卡片按钮确认 | 文档后期审批篇的形态，提前落地 |

## 五、故障排查

- **收不到消息**：检查事件订阅是否选了长连接、`im.message.receive_v1` 是否添加、应用是否已发布、机器人是否在群里；群里消息必须 @机器人（权限是"群聊中被 @ 的消息"）
- **按钮点了没反应**：检查回调订阅是否也开了长连接并添加 `card.action.trigger`
- **回复"请先生成绑定码"**：Trace client → 设置 → 飞书绑定 → 生成绑定码，发给机器人 `bind 123456`
- **日志看连接状态**：`feishu monitor started` / `feishu ws connected, service_id=...`；断线会指数退避重连（1s→30s）
- **常见错误码**：`code=99991663` token 失效（自动刷新）；权限类错误回开放平台检查权限范围
- **应急回退**：飞书不可用时通过 SSH 将目标的 `current` 原子切到 `previous` 并重启服务；SSH 始终保留为 break-glass
- **点了确认卡片没反应**：先看 admin「交互单」页找到该任务编号，看 `status`。
  `pending` 说明应答没写进去（多为节点离线，卡片会带提示）；`answered` 长期不动
  说明派发未完成（server 重启会自动退回 `pending` 可重试）；`cancelled` 是被取消。
  卡死的交互单可以在 admin 里直接手动应答，不必 `/new` 重开话题。
- **确认卡片提示「这个操作已经提交过了」**：同一张卡片只能应答一次
  （按 `interaction_id` 原子抢答）。若确实需要重跑，让 agent 重新发起工具调用。

## 六、定时任务（系统主动推送）

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

## 七、后续（未实现）

- 普通文件附件下载（当前已支持图片输入和 `[file:]` 图片回传）
- 更多 job：agent 整理的简报（cron 驱动 run_chat_turn）、失败 @人告警、巡检类任务
- 多 bot 互相 @ 协作（agent-os 文档后半程的团队作战）
- admin 交互单详情的 `analysis` 渲染 markdown（当前纯文本展示）
