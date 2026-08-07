//! handy 渠道：HTTP 推送 + webhook 回收应答，无 WS / 卡片生态。
//!
//! - `client`：handy `/api/v1/*` REST 封装（信封解析、Bearer 认证）
//! - `pusher`：agent 事件流 → handy 进度卡片 + 人工闸门（轮询兜底在 scheduler）
//! - `webhook`：handy 回推入口（interaction.answered 应答 + message.created 留言）
//! - `router`：用户消息 → 查/建会话 → run_chat_turn
//! - `routes`：用户级 handy 连接配置管理（client scope REST）
//!
//! 凭证体系是**用户级 DB 配置**（handy_accounts 表，每 trace 用户一条），
//! 不是飞书/微信的 admin 级账号表，也不是 config.toml 全局对端：
//! webhook 按路径里的 user_id 解析账号，下行按会话 user_id 取账号建 client。
//!
//! 下行挂接：source=="handy" 的会话在 run 启动时自动挂 pusher
//! （chat.rs 里的薄 hook，按会话 user_id 查 handy_accounts 建 client）；
//! 入站：handy 主动 webhook 推送到 /api/channels/handy/{user_id}/webhook。

pub mod client;
pub mod pusher;
pub mod router;
pub mod routes;
pub mod webhook;
