mod admin;
mod admin_terminal;
mod auth;
mod changes;
mod chat;
mod checkpoints;
mod cli_agent;
mod channel_records;
mod config;
mod deployment;
mod image_gen;
mod llm;
pub mod provider_registry;
pub mod remote_exec;
pub mod remote_tools;
mod requirement_docs;
pub mod response;
mod routes;
mod skills;
mod snap_tools;
mod specs;
pub mod task_state;
mod termshot;
mod websnap;
mod feishu;
mod scheduler;
mod weixin;

use anyhow::Result;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, Request},
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, patch, post, put},
    Router,
};
use config::Config;
use hank_db::Database;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::chat::EventBuffer;

pub struct AppState {
    pub db: Database,
    pub jwt_secret: String,
    pub config: Config,
    pub active_tasks: RwLock<HashMap<String, CancellationToken>>,
    pub event_buffers: RwLock<HashMap<String, EventBuffer>>,
    /// 微信 QR 登录状态机（login_id → 状态）
    pub weixin_logins: weixin::login::LoginStates,
    /// 微信账号 monitor 任务（account_id → 停止令牌）
    pub weixin_monitors: RwLock<HashMap<String, Arc<CancellationToken>>>,
    /// 飞书账号 WS 长连接（account_id → 停止令牌）
    pub feishu_monitors: RwLock<HashMap<String, Arc<CancellationToken>>>,
    /// 定时任务调度器状态（并发锁 + 下次执行时间）
    pub scheduler: scheduler::SchedulerState,
    /// 微信渠道 agent 的短期对话记忆（binding_id → 最近若干轮问答）
    pub weixin_channel_history:
        RwLock<HashMap<String, std::collections::VecDeque<weixin::channel::ChannelTurn>>>,
    /// 桌面 client 远程执行通道（user_id → 长轮询/派发状态）
    pub client_hubs: RwLock<HashMap<String, remote_exec::UserHub>>,
    /// quant 高成本 skill 会话级授权存储（进程内，重启失效）
    pub quant_grant_store: Arc<code_tools::quant_grant::QuantGrantStore>,
    /// quant 高成本工具待确认单（进程内 map，5 分钟 TTL，重启作废；设计 §5.4.4）
    pub quant_pending_confirms: Arc<code_tools::quant_grant::QuantPendingConfirmStore>,
    /// 渠道任务闸门与实时进度快照（单任务串行 + 随时可查进度）
    pub tasks: Arc<task_state::TaskRegistry>,
}

async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(t) => match auth::verify_token(t, &state.jwt_secret) {
            Ok(claims) => {
                request.extensions_mut().insert(claims);
                next.run(request).await
            }
            Err(e) => {
                tracing::warn!(error = %e, "auth failed: invalid token");
                response::unauthorized("invalid or expired token")
            }
        },
        _ => {
            tracing::warn!("auth failed: missing token");
            response::unauthorized("missing authorization token")
        }
    }
}

/// 在 auth_middleware 之后运行，要求 claims.can_admin == true
async fn admin_required(request: Request<axum::body::Body>, next: Next) -> Response {
    match request.extensions().get::<auth::Claims>() {
        Some(claims) if claims.can_admin => next.run(request).await,
        _ => {
            tracing::warn!("admin access denied: insufficient permissions");
            response::forbidden("admin access required")
        }
    }
}

/// 构建 CORS 白名单：Tauri 桌面端 + 本地开发端口 + config 中额外配置的 origin
fn cors_layer(extra_origins: &[String]) -> CorsLayer {
    use axum::http::{header, HeaderValue, Method};

    let mut origins: Vec<HeaderValue> = [
        "tauri://localhost",      // Tauri webview (macOS/Linux)
        "http://tauri.localhost", // Tauri webview (Windows)
        "http://localhost:1420",  // client dev server
        "http://localhost:5173",  // admin dev server
    ]
    .iter()
    .filter_map(|s| s.parse().ok())
    .collect();

    for o in extra_origins {
        match o.parse() {
            Ok(v) => origins.push(v),
            Err(_) => tracing::warn!(origin = %o, "ignoring invalid cors_origins entry"),
        }
    }

    CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("安装 Rustls ring CryptoProvider 失败"))?;

    // 日志：同时输出到终端和文件（按天滚动，实时写入）
    let file_appender = tracing_appender::rolling::daily("logs", "hank.log");

    let env_filter = EnvFilter::from_default_env()
        .add_directive("hank_server=debug".parse()?)
        .add_directive("code_agent=debug".parse()?)
        .add_directive("hank_provider=debug".parse()?)
        .add_directive("hank_db=debug".parse()?)
        .add_directive("code_tools=debug".parse()?);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().with_writer(std::io::stdout))
        .with(fmt::layer().with_writer(file_appender).with_ansi(false))
        .init();

    let config = Config::load()?;
    let db = Database::new(&config.server.database_url).await?;

    let state = Arc::new(AppState {
        db,
        jwt_secret: config.server.jwt_secret.clone(),
        config: config.clone(),
        active_tasks: RwLock::new(HashMap::new()),
        event_buffers: RwLock::new(HashMap::new()),
        weixin_logins: RwLock::new(HashMap::new()),
        weixin_monitors: RwLock::new(HashMap::new()),
        feishu_monitors: RwLock::new(HashMap::new()),
        scheduler: scheduler::SchedulerState::new(),
        weixin_channel_history: RwLock::new(HashMap::new()),
        client_hubs: RwLock::new(HashMap::new()),
        quant_grant_store: Arc::new(code_tools::quant_grant::QuantGrantStore::new()),
        quant_pending_confirms: Arc::new(
            code_tools::quant_grant::QuantPendingConfirmStore::new(),
        ),
        tasks: Arc::new(task_state::TaskRegistry::new()),
    });

    // 启动微信 bot 长轮询（为每个 enabled 账号起一个 monitor task）
    weixin::monitor::start_monitors(state.clone());

    // 启动飞书 WS 长连接（为每个 enabled 账号起一个 monitor task）
    feishu::monitor::start_monitors(state.clone());

    // 恢复跨越 server 自身重启的独立部署任务监听。
    deployment::recover_deployments(state.clone());

    // 启动定时任务调度器（cron 驱动的系统主动工作入口）
    scheduler::start(state.clone());

    // 启动 kimi 托管通知消费循环（client 通知 → 微信推送）
    tokio::spawn(weixin::kimi::run_notification_consumer(state.clone()));

    // Public routes (no auth required)
    let public = Router::new()
        .route("/api/health", get(routes::health))
        .route("/api/auth/login", post(routes::login));

    // Protected routes (auth required)
    let protected = Router::new()
        .route("/api/sessions", post(routes::create_session))
        .route("/api/sessions", get(routes::list_sessions))
        .route("/api/sessions/{id}", get(routes::get_session))
        .route("/api/sessions/{id}", delete(routes::delete_session))
        .route("/api/sessions/{id}", put(routes::update_session))
        .route("/api/sessions/{id}/messages", get(routes::get_messages))
        .route("/api/sessions/{id}/messages", post(routes::post_message))
        .route(
            "/api/sessions/{id}/messages/truncate",
            post(routes::truncate_messages),
        )
        .route("/api/sessions/{id}/tree", get(routes::get_message_tree))
        .route(
            "/api/sessions/{id}/active-leaf",
            put(routes::update_active_leaf),
        )
        .route(
            "/api/sessions/{id}/local-events",
            post(routes::post_local_events),
        )
        .route("/api/sessions/{id}/events", get(routes::get_session_events))
        .route(
            "/api/sessions/{id}/transcript",
            get(routes::get_session_transcript),
        )
        .route("/api/settings", put(routes::update_settings))
        .route("/api/providers", get(routes::list_providers))
        // Image providers (public list for client)
        .route("/api/image-providers", get(image_gen::list_image_providers))
        // Image generation
        .route("/api/image/generate", post(image_gen::generate_image))
        .route("/api/image/edit", post(image_gen::edit_image))
        .route("/api/sessions/{id}/chat", post(chat::chat_handler))
        .route("/api/sessions/{id}/stop", post(chat::stop_handler))
        .route(
            "/api/sessions/{id}/events/resume",
            get(chat::resume_handler),
        )
        .route("/api/llm/completion", post(llm::completion_handler))
        .route("/api/llm/tool-exec", post(llm::tool_exec_handler))
        .route("/api/fs/list", get(routes::list_directory))
        // Templates (client-accessible, read-only)
        .route("/api/templates", get(routes::list_templates))
        // Specs routes
        .route("/api/specs", get(specs::list_specs))
        .route("/api/specs", post(specs::create_spec))
        .route("/api/specs/{id}", get(specs::get_spec))
        .route("/api/specs/{id}", put(specs::update_spec))
        .route("/api/specs/{id}", delete(specs::delete_spec))
        .route("/api/specs/{id}/versions", get(specs::list_spec_versions))
        // Changes routes
        .route("/api/changes", get(changes::list_changes))
        .route("/api/changes", post(changes::create_change))
        .route("/api/changes/{id}", get(changes::get_change))
        .route("/api/changes/{id}", put(changes::update_change))
        .route("/api/changes/{id}", delete(changes::delete_change))
        .route("/api/changes/{id}/explore", post(changes::start_explore))
        .route("/api/changes/{id}/generate", post(changes::start_generate))
        .route(
            "/api/changes/{id}/artifacts/confirm",
            post(changes::confirm_artifacts),
        )
        .route("/api/changes/{id}/archive", post(changes::archive_change))
        // Artifacts routes
        .route("/api/changes/{id}/artifacts", get(changes::list_artifacts))
        .route(
            "/api/changes/{id}/artifacts",
            post(changes::create_artifact),
        )
        .route(
            "/api/changes/{id}/artifacts/{aid}",
            get(changes::get_artifact),
        )
        .route(
            "/api/changes/{id}/artifacts/{aid}",
            put(changes::update_artifact),
        )
        .route(
            "/api/changes/{id}/artifacts/{aid}",
            delete(changes::delete_artifact),
        )
        // Tasks routes
        .route("/api/changes/{id}/tasks", get(changes::list_tasks))
        .route("/api/changes/{id}/tasks", post(changes::batch_create_tasks))
        .route("/api/changes/{id}/tasks/{tid}", put(changes::update_task))
        .route(
            "/api/changes/{id}/tasks/{tid}",
            delete(changes::delete_task),
        )
        // Context route
        .route(
            "/api/changes/{id}/context",
            get(changes::get_change_context),
        )
        // Checkpoints routes
        .route(
            "/api/sessions/{id}/checkpoints",
            get(checkpoints::list_checkpoints_handler),
        )
        .route(
            "/api/sessions/{id}/rewind/{cpid}",
            post(checkpoints::rewind_handler),
        )
        // Skills routes
        .route("/api/skills", get(skills::list_skills))
        .route("/api/skills/install", post(skills::install_skill))
        .route("/api/skills/{name}", delete(skills::uninstall_skill))
        // Requirement docs routes (client)
        .route("/api/requirement-docs", post(requirement_docs::create_doc))
        .route(
            "/api/requirement-docs/{id}",
            put(requirement_docs::update_doc),
        )
        .route(
            "/api/requirement-docs/by-change/{changeId}",
            get(requirement_docs::get_doc_by_change),
        )
        // Weixin routes (client)
        .route(
            "/api/weixin/bind-code",
            post(weixin::routes::create_bind_code),
        )
        .route("/api/weixin/binding", get(weixin::routes::get_binding))
        .route(
            "/api/weixin/binding",
            delete(weixin::routes::delete_binding),
        )
        // Feishu routes (client)
        .route(
            "/api/feishu/bind-code",
            post(feishu::routes::create_bind_code),
        )
        .route("/api/feishu/binding", get(feishu::routes::get_binding))
        .route(
            "/api/feishu/binding",
            delete(feishu::routes::delete_binding),
        )
        // Remote execution: desktop client long-poll channel
        .route(
            "/api/client/registration",
            put(remote_exec::register_client),
        )
        .route("/api/client/notify", post(remote_exec::post_notification))
        .route("/api/client/poll", get(remote_exec::poll_requests))
        // tool-result 可能携带媒体文件 base64 回传（20MB 文件约 27MB），放宽 body 上限
        .route(
            "/api/client/tool-result",
            post(remote_exec::post_tool_result)
                .route_layer(DefaultBodyLimit::max(40 * 1024 * 1024)),
        )
        .route("/api/client/online", get(remote_exec::list_online))
        .route(
            "/api/sessions/{id}/exec-client",
            put(remote_exec::set_session_exec_client),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // Admin API routes (also protected)
    let admin_api = Router::new()
        .route("/api/admin/sessions", get(admin::list_sessions))
        .route(
            "/api/admin/sessions/{id}/replay",
            get(admin::session_replay),
        )
        .route(
            "/api/admin/sessions/{id}/events",
            get(admin::session_events),
        )
        .route("/api/admin/metrics/overview", get(admin::metrics_overview))
        .route(
            "/api/admin/metrics/by-session/{id}",
            get(admin::metrics_by_session),
        )
        .route(
            "/api/admin/prompt-templates",
            post(admin::create_prompt_template),
        )
        .route(
            "/api/admin/prompt-templates",
            get(admin::list_prompt_templates),
        )
        .route(
            "/api/admin/prompt-templates/{id}",
            delete(admin::delete_prompt_template),
        )
        .route("/api/admin/chat/generate", post(admin::chat_generate))
        .route("/api/admin/replay", post(admin::replay_with_prompt))
        .route("/api/admin/users", get(admin::list_users))
        .route("/api/admin/users", post(admin::create_user))
        .route("/api/admin/users/{id}", put(admin::update_user))
        .route("/api/admin/users/{id}", delete(admin::delete_user))
        .route("/api/admin/providers", get(admin::list_providers))
        .route("/api/admin/providers", post(admin::create_provider))
        .route("/api/admin/providers/{id}", put(admin::update_provider))
        .route("/api/admin/providers/{id}", delete(admin::delete_provider))
        // 外部 Agent CLI（codex / claude）凭据：每后端多份配置，切换启用即时生效
        .route(
            "/api/admin/agent-cli-config",
            get(admin::list_agent_cli_configs),
        )
        .route(
            "/api/admin/agent-cli-config/{backend}",
            post(admin::create_agent_cli_profile),
        )
        .route(
            "/api/admin/agent-cli-config/{backend}/deactivate",
            post(admin::deactivate_agent_cli_profiles),
        )
        .route(
            "/api/admin/agent-cli-config/profiles/{id}",
            put(admin::update_agent_cli_profile),
        )
        .route(
            "/api/admin/agent-cli-config/profiles/{id}",
            delete(admin::delete_agent_cli_profile),
        )
        .route(
            "/api/admin/agent-cli-config/profiles/{id}/activate",
            post(admin::activate_agent_cli_profile),
        )
        .route(
            "/api/admin/agent-cli-config/profiles/{id}/test",
            post(admin::test_agent_cli_profile),
        )
        // Image providers admin
        .route(
            "/api/admin/image-providers",
            get(admin::list_image_providers),
        )
        .route(
            "/api/admin/image-providers",
            post(admin::create_image_provider),
        )
        .route(
            "/api/admin/image-providers/{id}",
            put(admin::update_image_provider),
        )
        .route(
            "/api/admin/image-providers/{id}",
            delete(admin::delete_image_provider),
        )
        // Admin requirement docs & tasks
        .route(
            "/api/admin/requirement-docs",
            get(requirement_docs::admin_list_docs),
        )
        .route(
            "/api/admin/requirement-docs/{id}",
            get(requirement_docs::admin_get_doc),
        )
        .route("/api/admin/tasks", get(requirement_docs::admin_list_tasks))
        // Weixin bot admin
        .route(
            "/api/admin/weixin/login",
            post(weixin::routes::create_login),
        )
        .route(
            "/api/admin/weixin/login/{login_id}",
            get(weixin::routes::get_login),
        )
        .route(
            "/api/admin/weixin/accounts",
            get(weixin::routes::list_accounts),
        )
        .route(
            "/api/admin/weixin/accounts/{id}",
            patch(weixin::routes::update_account),
        )
        .route(
            "/api/admin/weixin/accounts/{id}",
            delete(weixin::routes::delete_account),
        )
        .route(
            "/api/admin/weixin/bindings",
            get(weixin::routes::list_bindings),
        )
        .route(
            "/api/admin/weixin/bindings/{id}",
            delete(weixin::routes::delete_binding_admin),
        )
        .route("/api/admin/weixin/send", post(weixin::routes::send_message))
        // Feishu admin routes（应用账号 + 绑定管理）
        .route(
            "/api/admin/feishu/accounts",
            get(feishu::routes::list_accounts),
        )
        .route(
            "/api/admin/feishu/accounts",
            post(feishu::routes::create_account),
        )
        .route(
            "/api/admin/feishu/accounts/{id}",
            patch(feishu::routes::update_account),
        )
        .route(
            "/api/admin/feishu/accounts/{id}",
            delete(feishu::routes::delete_account),
        )
        .route(
            "/api/admin/feishu/bindings",
            get(feishu::routes::list_bindings),
        )
        .route(
            "/api/admin/feishu/bind-code",
            post(feishu::routes::create_bind_code_admin),
        )
        .route(
            "/api/admin/feishu/bindings/{id}",
            delete(feishu::routes::delete_binding_admin),
        )
        .route("/api/admin/feishu/send", post(feishu::routes::send_message))
        // Channel chat records（目前开放飞书，后续渠道复用）
        .route(
            "/api/admin/chat-records/conversations",
            get(channel_records::list_conversations),
        )
        .route(
            "/api/admin/chat-records/messages",
            get(channel_records::list_messages),
        )
        // Scheduler admin routes（定时任务管理）
        .route("/api/admin/jobs", get(scheduler::routes::list_jobs))
        .route(
            "/api/admin/jobs/{id}",
            patch(scheduler::routes::update_job),
        )
        .route(
            "/api/admin/jobs/{id}/runs",
            get(scheduler::routes::job_runs),
        )
        .route(
            "/api/admin/jobs/{id}/run",
            post(scheduler::routes::run_job),
        )
        // Admin terminal proxy
        .route("/api/admin/clients", get(admin_terminal::list_clients))
        .route(
            "/api/admin/clients/{cid}/terminals",
            get(admin_terminal::list_terminals),
        )
        .route(
            "/api/admin/clients/{cid}/terminals/{tid}/output",
            get(admin_terminal::terminal_output),
        )
        .route(
            "/api/admin/clients/{cid}/terminals/{tid}/input",
            post(admin_terminal::terminal_input),
        )
        .route(
            "/api/admin/notifications",
            get(admin_terminal::list_notifications),
        )
        .layer(middleware::from_fn(admin_required))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // Static file serving for admin SPA
    // 注意用 fallback（保留 200 状态），not_found_service 会把 SPA 路由也标成 404
    let admin_static =
        ServeDir::new("admin/dist").fallback(ServeFile::new("admin/dist/index.html"));

    let app = public
        .merge(protected)
        .merge(admin_api)
        .nest_service("/admin", admin_static)
        .layer(cors_layer(&config.server.cors_origins))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    tracing::info_span!(
                        "http_request",
                        method = %request.method(),
                        uri = %request.uri(),
                    )
                })
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state);

    let addr = format!("{}:{}", config.server.host, config.server.port);
    tracing::info!("Server listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
