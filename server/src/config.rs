use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

pub const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
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
