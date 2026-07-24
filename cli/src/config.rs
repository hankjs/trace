//! 配置加载：~/.hank-cli/config.toml + CLI 参数覆盖。
//! 首次运行自动生成 client_id 并写回配置文件；token 不落盘（过期直接重登）。

use std::path::PathBuf;

use clap::Parser;
use serde::{Deserialize, Serialize};

/// 默认数据目录名（home 下）
const DATA_DIR_NAME: &str = ".hank-cli";

#[derive(Parser)]
#[command(name = "hank-cli", about = "hank headless 远程终端节点：向 server 暴露本机终端能力")]
pub struct CliArgs {
    /// 配置文件路径（默认 ~/.hank-cli/config.toml）
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// server 地址，覆盖配置文件（如 http://127.0.0.1:3000）
    #[arg(long)]
    pub server: Option<String>,
    /// 登录用户名，覆盖配置文件
    #[arg(long)]
    pub username: Option<String>,
    /// 登录密码，覆盖配置文件
    #[arg(long)]
    pub password: Option<String>,
}

/// 配置文件（~/.hank-cli/config.toml）原始结构
#[derive(Serialize, Deserialize, Default)]
struct FileConfig {
    server: Option<String>,
    username: Option<String>,
    password: Option<String>,
    /// 注册用工作目录（可选）
    work_dir: Option<String>,
    /// 本节点 ID，首次运行自动生成 uuid 并写回
    client_id: Option<String>,
}

/// 合并 CLI 覆盖后的最终配置
pub struct Config {
    pub server: String,
    pub username: String,
    pub password: String,
    pub work_dir: Option<String>,
    pub client_id: String,
    /// 数据目录（~/.hank-cli），shell-integration 等运行时文件放这里
    pub data_dir: PathBuf,
}

fn default_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(DATA_DIR_NAME).join("config.toml"))
}

/// 加载配置：读配置文件 → 生成/写回 client_id → CLI 参数覆盖 → 校验必填项
pub fn load(args: &CliArgs) -> Result<Config, String> {
    let config_path = args
        .config
        .clone()
        .or_else(default_config_path)
        .ok_or("无法确定 home 目录，请用 --config 指定配置文件")?;
    let data_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut file = FileConfig::default();
    if config_path.exists() {
        let text = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("读取配置失败 {}: {e}", config_path.display()))?;
        file = toml::from_str(&text)
            .map_err(|e| format!("解析配置失败 {}: {e}", config_path.display()))?;
    }

    // client_id 缺失时生成并写回配置文件（同一节点保持稳定 ID）
    if file.client_id.as_deref().unwrap_or("").trim().is_empty() {
        file.client_id = Some(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| format!("创建数据目录失败 {}: {e}", data_dir.display()))?;
        let text = toml::to_string_pretty(&file).map_err(|e| format!("序列化配置失败: {e}"))?;
        std::fs::write(&config_path, text)
            .map_err(|e| format!("写回配置失败 {}: {e}", config_path.display()))?;
    }

    let server = args
        .server
        .clone()
        .or(file.server)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("缺少 server 配置，请在 {} 或 --server 中指定", config_path.display()))?;
    let username = args
        .username
        .clone()
        .or(file.username)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("缺少 username 配置，请在 {} 或 --username 中指定", config_path.display()))?;
    let password = args
        .password
        .clone()
        .or(file.password)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("缺少 password 配置，请在 {} 或 --password 中指定", config_path.display()))?;

    Ok(Config {
        server: server.trim_end_matches('/').to_string(),
        username,
        password,
        work_dir: file.work_dir.filter(|s| !s.trim().is_empty()),
        client_id: file.client_id.unwrap(),
        data_dir,
    })
}
