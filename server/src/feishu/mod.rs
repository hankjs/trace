//! 飞书渠道：WS 长连接收消息/卡片回调 + REST 回消息。
//!
//! 参考 docs/book/agent-os 的 Node.js 实现复刻为 Rust：
//! - monitor: pbbp2 protobuf 帧长连接（无需公网入口），指数退避重连
//! - router:  消息解析、话题=会话映射、斜杠命令、任务派发（复用 chat::run_chat_turn）
//! - pusher:  agent 事件流 → 任务卡片节流刷新（蓝→绿/红）
//! - callback: 卡片按钮回调 → 包装成文本回复走现有确认闸门
//!
//! 与 weixin 模块的关系：同一套渠道模式（monitor/router/pusher），
//! 但飞书用卡片替代纯文本进度，确认从文本白名单升级为按钮点击。

pub mod api;
pub mod callback;
pub mod card;
pub mod monitor;
pub mod pusher;
pub mod router;
pub mod routes;
pub mod ws;
