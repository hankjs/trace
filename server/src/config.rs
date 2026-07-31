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
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerAgentConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_repository_root")]
    pub repository_root: String,
    #[serde(default = "default_worktrees_root")]
    pub worktrees_root: String,
    /// 与 Trace/quant 无关的任务使用普通隔离目录，不创建 Git worktree。
    #[serde(default = "default_general_workspaces_root")]
    pub general_workspaces_root: String,
    #[serde(default = "default_base_ref")]
    pub base_ref: String,
    #[serde(default = "default_deploy_jobs_dir")]
    pub deploy_jobs_dir: String,
    #[serde(default = "default_deploy_helper")]
    pub deploy_helper: String,
    /// 执行仓库 shell、测试和构建的低权限用户，不具备部署 sudoers。
    #[serde(default = "default_execution_user")]
    pub execution_user: String,
    /// Claude Code / Codex 的离线安装目录。
    #[serde(default = "default_agent_cli_root")]
    pub agent_cli_root: String,
    /// 每个飞书话题独占的 CLI HOME 与上下文目录。
    #[serde(default = "default_agent_state_root")]
    pub agent_state_root: String,
    /// 外部 Agent 单轮最长运行时间。
    #[serde(default = "default_agent_timeout_secs")]
    pub agent_timeout_secs: u64,
    /// 外部 Agent stdout/stderr 各自允许保留的最大字节数。
    #[serde(default = "default_agent_output_limit_bytes")]
    pub agent_output_limit_bytes: usize,
    /// 外部 Agent 必须通过该程序进入文件系统沙箱；缺失时拒绝启动。
    #[serde(default = "default_agent_sandbox_bin")]
    pub agent_sandbox_bin: String,
    /// 生产环境以非 root 用户运行 server 时，通过 sudo 调用唯一允许的部署 helper。
    #[serde(default)]
    pub deploy_use_sudo: bool,
    #[serde(default = "default_deploy_approval_ttl_secs")]
    pub approval_ttl_secs: u64,
}

impl Default for ServerAgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            repository_root: default_repository_root(),
            worktrees_root: default_worktrees_root(),
            general_workspaces_root: default_general_workspaces_root(),
            base_ref: default_base_ref(),
            deploy_jobs_dir: default_deploy_jobs_dir(),
            deploy_helper: default_deploy_helper(),
            execution_user: default_execution_user(),
            agent_cli_root: default_agent_cli_root(),
            agent_state_root: default_agent_state_root(),
            agent_timeout_secs: default_agent_timeout_secs(),
            agent_output_limit_bytes: default_agent_output_limit_bytes(),
            agent_sandbox_bin: default_agent_sandbox_bin(),
            deploy_use_sudo: false,
            approval_ttl_secs: default_deploy_approval_ttl_secs(),
        }
    }
}

fn default_repository_root() -> String {
    "/opt/hank-src".to_string()
}

fn default_worktrees_root() -> String {
    "/opt/hank-worktrees".to_string()
}

fn default_general_workspaces_root() -> String {
    "/opt/hank-workspaces".to_string()
}

fn default_base_ref() -> String {
    "trace-production".to_string()
}

fn default_deploy_jobs_dir() -> String {
    "/opt/hank/deploy-jobs".to_string()
}

fn default_deploy_helper() -> String {
    "/usr/local/libexec/hank-deploy".to_string()
}

fn default_execution_user() -> String {
    "hank-build".to_string()
}

fn default_agent_cli_root() -> String {
    "/opt/hank-agent-cli".to_string()
}

fn default_agent_state_root() -> String {
    "/opt/hank-agent-state".to_string()
}

fn default_agent_timeout_secs() -> u64 {
    30 * 60
}

fn default_agent_output_limit_bytes() -> usize {
    2 * 1024 * 1024
}

fn default_agent_sandbox_bin() -> String {
    "/usr/bin/bwrap".to_string()
}

fn default_deploy_approval_ttl_secs() -> u64 {
    10 * 60
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
    /// 是否启动飞书 WS 长连接。多实例共库时只能有一个实例开启（与 weixin_monitor 同理）
    #[serde(default = "default_true")]
    pub feishu_monitor: bool,
    /// 是否启动定时任务调度器。多实例共库时只能有一个实例开启（避免重复推送/执行）
    #[serde(default = "default_true")]
    pub scheduler_enabled: bool,
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
