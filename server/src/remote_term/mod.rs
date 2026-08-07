//! 桌面 client 远程终端通道（精简版）。
//!
//! 只服务 admin 网页终端代理与 WebRTC 信令：
//! - client 长轮询取 `terminal_*` / `rtc_signal` 工具调用，POST 回传结果
//! - **不**恢复 agent 远程执行（shell / 写文件 / exec_client_id）
//!
//! 在线判定：client 每次 poll 刷新 last_poll；60s 内有 poll 即在线。
//! 单 worker 内存 hub：多实例时 client 只连一个 API_BASE，与 handy 同约束。

mod hub;
mod routes;

pub use hub::{dispatch_tool_call, is_client_online, UserHub};
pub use routes::{list_online, poll_requests, post_tool_result, register_client};
