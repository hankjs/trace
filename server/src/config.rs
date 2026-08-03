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
    /// 团队任务流水线（开发→评审→测试多角色编排）。默认关闭。
    #[serde(default)]
    pub team_task: TeamTaskConfig,
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
    /// 两阶段任务闸门的**初始默认值**。
    /// 运行时真正生效的值在数据库（settings 表，admin「团队任务」页可改），
    /// 见 `team_task::settings::effective`。这里只在 DB 尚无配置时作兜底。
    ///
    /// 与 `enabled` 解耦：client-only 链路在 `server_agent.enabled = false` 时
    /// 同样要能用闸门。调用方不得写成 `enabled && task_gate_enabled`。
    #[serde(default)]
    pub task_gate_enabled: bool,
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
            task_gate_enabled: false,
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

/// 外部 Agent stdout 上限（runaway 保护）。JSONL 是逐行转发即弃的，不占内存，
/// 所以这里只需要防跑飞；stream-json 下探索型任务几 MiB 起步，2 MiB 会误杀。
fn default_agent_output_limit_bytes() -> usize {
    64 * 1024 * 1024
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
    /// quant 服务根地址或完整 `/a2a` RPC 地址；客户端会统一规范化。
    #[serde(default = "default_quant_a2a_base_url")]
    pub base_url: String,
}

fn default_quant_a2a_base_url() -> String {
    "http://127.0.0.1:8100".to_string()
}

/// 团队任务流水线的**初始默认值**。运行时真正生效的配置在数据库
/// （settings 表，admin 可改），见 `team_task::settings::effective`。
/// 这里的值只在 DB 里还没有配置时作为兜底，便于升级上线时行为不变。
///
/// config.toml 不写 `[team_task]` 段也能启动（默认 enabled = false）。
#[derive(Debug, Clone, Deserialize)]
pub struct TeamTaskConfig {
    /// 总开关默认值。关闭时 task_gate 走原来的单角色两阶段路径。
    #[serde(default)]
    pub enabled: bool,
    /// 参与流水线的角色默认顺序。可裁剪成 ["developer"] 只跑单角色。
    #[serde(default = "default_team_roles")]
    pub roles: Vec<String>,
    /// 需要人工确认的边界默认值。默认只保留开发前闸门。
    #[serde(default = "default_team_gates")]
    pub gates: Vec<String>,
    /// 评审打回后最多重新开发几轮的默认上限。
    #[serde(default = "default_max_dev_rounds")]
    pub max_dev_rounds: i32,
    /// 看板外部可访问地址默认值。
    #[serde(default)]
    pub dashboard_base_url: Option<String>,
}

impl Default for TeamTaskConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            roles: default_team_roles(),
            gates: default_team_gates(),
            max_dev_rounds: default_max_dev_rounds(),
            dashboard_base_url: None,
        }
    }
}

fn default_team_roles() -> Vec<String> {
    vec![
        "developer".to_string(),
        "reviewer".to_string(),
        "tester".to_string(),
    ]
}

fn default_team_gates() -> Vec<String> {
    vec!["dev_start".to_string()]
}

fn default_max_dev_rounds() -> i32 {
    3
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小可反序列化的 [server] 段（必填字段）。
    const MIN_SERVER: &str = r#"
[server]
host = "127.0.0.1"
port = 3000
jwt_secret = "test-secret"
database_url = "mysql://u:p@localhost/db"
"#;

    #[test]
    fn team_task_absent_uses_defaults() {
        let cfg: Config = toml::from_str(MIN_SERVER).expect("parse");
        assert!(!cfg.team_task.enabled);
        assert_eq!(
            cfg.team_task.roles,
            vec!["developer", "reviewer", "tester"]
        );
        assert_eq!(cfg.team_task.gates, vec!["dev_start"]);
        assert_eq!(cfg.team_task.max_dev_rounds, 3);
        assert!(cfg.team_task.dashboard_base_url.is_none());
    }

    #[test]
    fn team_task_enabled_only_keeps_other_defaults() {
        let toml = format!("{MIN_SERVER}\n[team_task]\nenabled = true\n");
        let cfg: Config = toml::from_str(&toml).expect("parse");
        assert!(cfg.team_task.enabled);
        assert_eq!(
            cfg.team_task.roles,
            vec!["developer", "reviewer", "tester"]
        );
        assert_eq!(cfg.team_task.gates, vec!["dev_start"]);
        assert_eq!(cfg.team_task.max_dev_rounds, 3);
        assert!(cfg.team_task.dashboard_base_url.is_none());
    }

    #[test]
    fn team_task_roles_override_leaves_gates_default() {
        let toml = format!(
            r#"{MIN_SERVER}
[team_task]
roles = ["developer"]
"#
        );
        let cfg: Config = toml::from_str(&toml).expect("parse");
        assert!(!cfg.team_task.enabled);
        assert_eq!(cfg.team_task.roles, vec!["developer"]);
        assert_eq!(cfg.team_task.gates, vec!["dev_start"]);
        assert_eq!(cfg.team_task.max_dev_rounds, 3);
    }
}
