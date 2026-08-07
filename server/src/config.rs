use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

pub const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    /// 飞书驱动的 server-only monorepo Agent。默认关闭，避免本地开发环境
    /// 意外把消息路由到服务器文件系统。
    #[serde(default)]
    pub server_agent: ServerAgentConfig,
    #[serde(default)]
    pub quant_a2a: Option<QuantA2aConfig>,
    /// WebRTC ICE/TURN（admin 远程终端 P2P）。缺省只回公网 STUN。
    #[serde(default)]
    pub turn: TurnConfig,
}

/// coturn use-auth-secret 配置。urls 空时 ice 签发退回公网 STUN。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TurnConfig {
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub secret: String,
    #[serde(default = "default_turn_ttl")]
    pub ttl_seconds: u64,
}

fn default_turn_ttl() -> u64 {
    86400
}

/// server 侧原生会话开关。
///
/// 只剩 `enabled` 一个字段：worktree / 沙箱 / 降权用户 / 部署 helper / 任务闸门
/// 相关配置已随代码 Agent 执行链路下线。开启后 feishu 新话题会建 server 会话
/// （仅限管理员），会话仍然只有对话与 server 本地工具，不再创建任何工作区目录。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServerAgentConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuantA2aConfig {
    /// 是否在所有已登录会话注册 quant_* 工具（默认 false）。
    /// 开启后每个会话都会挂载 19 个工具 schema 并注入 quant-research skill 全文，
    /// 需显式 opt-in。
    #[serde(default)]
    pub enabled: bool,
    /// quant 服务根地址或完整 `/a2a` RPC 地址；客户端会统一规范化。
    #[serde(default = "default_quant_a2a_base_url")]
    pub base_url: String,
}

fn default_quant_a2a_base_url() -> String {
    "http://127.0.0.1:8100".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub jwt_secret: String,
    pub database_url: String,
    #[serde(default)]
    pub allowed_dirs: Vec<String>,
    /// 额外允许的 CORS origin（默认已含 Tauri 桌面端和本地开发端口）
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// 是否启动微信 getupdates 长轮询。多实例共库时只能有一个实例开启,
    /// 本地 dev 应设为 false, 避免与线上实例争抢消息
    #[serde(default = "default_true")]
    pub weixin_monitor: bool,
    /// 是否启动飞书 WS 长连接。多实例共库时只能有一个实例开启（与 weixin_monitor 同理）
    #[serde(default = "default_true")]
    pub feishu_monitor: bool,
    /// 是否启动定时任务调度器。多实例共库时只能有一个实例开启（避免重复推送/执行）
    #[serde(default = "default_true")]
    pub scheduler_enabled: bool,
    /// Chrome/Chromium 可执行文件路径（/snap 网页截图用）。留空则自动探测常见路径
    #[serde(default)]
    pub chrome_path: Option<String>,
    /// admin 后台外部可访问地址，用于在渠道卡片里生成交互单详情深链。
    /// 留空则卡片不渲染深链行（本地 dev 常见）。格式如 `https://admin.example.com`。
    #[serde(default)]
    pub admin_base_url: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Config {
    pub fn load() -> Result<Self> {
        let candidates = ["config.toml", "config.local.toml"];
        for path in &candidates {
            if Path::new(path).exists() {
                let content = std::fs::read_to_string(path)
                    .with_context(|| format!("Failed to read {path}"))?;
                let config: Config =
                    toml::from_str(&content).with_context(|| format!("Failed to parse {path}"))?;
                return Ok(config);
            }
        }

        if let Ok(path) = std::env::var("CONFIG_PATH") {
            let content =
                std::fs::read_to_string(&path).with_context(|| format!("Failed to read {path}"))?;
            let config: Config =
                toml::from_str(&content).with_context(|| format!("Failed to parse {path}"))?;
            return Ok(config);
        }

        bail!("No config file found. Create config.toml from config.example.toml")
    }
}
