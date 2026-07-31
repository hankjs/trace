use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

pub const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    #[serde(default)]
    pub quant_a2a: Option<QuantA2aConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuantA2aConfig {
    /// 是否在所有已登录会话注册 quant_* 工具（默认 false）。
    /// 开启后每个会话都会挂载 19 个工具 schema 并注入 quant-research skill 全文，
    /// 需显式 opt-in。
    #[serde(default)]
    pub enabled: bool,
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
    /// Chrome/Chromium 可执行文件路径（/snap 网页截图用）。留空则自动探测常见路径
    #[serde(default)]
    pub chrome_path: Option<String>,
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
