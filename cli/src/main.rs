//! hank-cli：headless 远程终端节点。
//! 不依赖 Tauri app，独立向 server 暴露本机终端能力：
//! login → registration → 长轮询取工具调用 → 执行 → 回传结果。

mod api;
mod config;
mod notify;
mod terminal;
mod worker;

use std::process::exit;
use std::sync::Arc;

use clap::Parser;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = config::CliArgs::parse();
    let cfg = match config::load(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("配置错误: {e}");
            exit(1);
        }
    };
    tracing::info!(server = %cfg.server, client_id = %cfg.client_id, "hank-cli 启动");

    let api = api::ApiClient::new(cfg.server.clone(), cfg.username.clone(), cfg.password.clone());

    // 登录失败即退出并提示（账号错误属配置问题，重试无意义）
    if let Err(e) = api.login().await {
        eprintln!("登录失败: {e}");
        exit(1);
    }
    tracing::info!("登录成功");

    let hostname = gethostname::gethostname().to_string_lossy().to_string();
    if let Err(e) = api
        .register(&cfg.client_id, Some(&hostname), cfg.work_dir.as_deref())
        .await
    {
        eprintln!("注册失败: {e}");
        exit(1);
    }
    tracing::info!(hostname, work_dir = ?cfg.work_dir, "注册成功，进入 poll 循环");

    let term = Arc::new(terminal::TermManager::new(cfg.data_dir.clone()));
    worker::run(api, term, cfg.client_id.clone()).await;
}
