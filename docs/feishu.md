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
- **远程执行**：会话创建时自动绑定在线桌面 client（与微信渠道同逻辑），飞书派的活在你的桌面上跑

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
| 话题内继续追问 | 同一会话续接（`feishu_chats` 映射），上下文不断 |
| 高成本 quant 工具 | 弹确认卡片：点「确认」/「否」；文字回复「确认5次」可批量授权（N≤50） |
| `/new` | 关闭当前话题会话，下次发消息开新会话 |
| `/stop` | 取消当前执行中的任务 |
| `/status` | 查看当前话题的会话 ID 与状态 |
| `/help` | 命令列表 |

## 四、与 agent-os 文档的对应与差异

| 文档做法 | 本项目实现 | 原因 |
|---------|-----------|------|
| Node.js + `@larksuiteoapi/node-sdk` | 纯 Rust 手写 pbbp2 帧（prost）+ REST | 社区 SDK（open-lark 0.14）不转发 `card` 类型帧，确认按钮回调收不到 |
| headless `claude -p` 做执行引擎 | 复用 server 自己的 agent | server 已有完整 agent（quant 工具链/确认闸门/远程执行），第二引擎接不进去 |
| 会话存 `data/sessions.json` | `feishu_chats` 表 + server 会话本就在 DB | 天然解决持久化与重启恢复 |
| 文本回复确认 | 卡片按钮确认 | 文档后期审批篇的形态，提前落地 |

## 五、故障排查

- **收不到消息**：检查事件订阅是否选了长连接、`im.message.receive_v1` 是否添加、应用是否已发布、机器人是否在群里；群里消息必须 @机器人（权限是"群聊中被 @ 的消息"）
- **按钮点了没反应**：检查回调订阅是否也开了长连接并添加 `card.action.trigger`
- **回复"请先生成绑定码"**：Trace client → 设置 → 飞书绑定 → 生成绑定码，发给机器人 `bind 123456`
- **日志看连接状态**：`feishu monitor started` / `feishu ws connected, service_id=...`；断线会指数退避重连（1s→30s）
- **常见错误码**：`code=99991663` token 失效（自动刷新）；权限类错误回开放平台检查权限范围

## 六、后续（未实现）

- 图片/文件下载（`im:resource` 权限 + resource.get），截图修 bug 场景
- `[file:]` 标记媒体回传（上传 image/file 再发送）
- 多 bot 互相 @ 协作、定时巡检（agent-os 文档后半程的团队作战）
