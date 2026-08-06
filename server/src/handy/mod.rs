//! handy 渠道：HTTP 推送 + webhook 回收应答，无 WS / 卡片生态。
//!
//! - `client`：handy `/api/v1/*` REST 封装（信封解析、Bearer 认证）
//! - `pusher`：agent 事件流 → handy 进度卡片 + 人工闸门（含轮询兜底）
//! - `webhook`：handy 回推入口（interaction.answered 应答 + message.created 留言）
//! - `router`：用户消息 → 查/建会话 → run_chat_turn
//!
//! 下行挂接：source=="handy" 的会话在 run 启动时自动挂 pusher
//! （chat.rs 里的薄 hook）；入站：handy 主动 webhook 推送留言。

pub mod client;
pub mod pusher;
pub mod router;
pub mod webhook;
