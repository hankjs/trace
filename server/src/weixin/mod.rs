//! 微信机器人模块：ilink 协议直实现。
//!
//! - `api`：ilink HTTP client（QR 登录 / getupdates / sendmessage）
//! - `channel`：渠道 agent（LLM 前置路由：回复 / 派发 / 停止 / 新会话）
//! - `kimi`：Kimi CLI 托管会话（远程终端 spawn / 输入转发 / 通知回推）
//! - `login`：QR 登录状态机（admin 轮询驱动）
//! - `monitor`：常驻 getupdates 长轮询任务
//! - `router`：消息路由（绑定 / 命令 / chat）
//! - `pusher`：agent 事件流回推微信
//! - `routes`：admin + client HTTP 接口

pub mod api;
pub mod channel;
pub mod kimi;
pub mod login;
pub mod monitor;
pub mod pusher;
pub mod router;
pub mod routes;
