mod admin;
mod api_keys;
mod auth;
mod changes;
mod channel_records;
mod chat;
mod checkpoints;
mod config;
mod server_workspace;
mod feishu;
mod handy;
mod image_gen;
mod interaction_flow;
mod interactions;
mod llm;
pub mod provider_registry;
mod remote_term;
mod requirement_docs;
pub mod response;
mod routes;
mod scheduler;
mod skills;
mod snap_tools;
mod specs;
pub mod task_state;
mod app_web;
mod turn;
mod websnap;
mod weixin;

use anyhow::Result;
use axum::{
    extract::State,
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
    /// quant 高成本 skill 会话级授权存储（进程内，重启失效）
    pub quant_grant_store: Arc<code_tools::quant_grant::QuantGrantStore>,
    /// 渠道任务闸门与实时进度快照（单任务串行 + 随时可查进度）
    pub tasks: Arc<task_state::TaskRegistry>,
    /// 桌面 client 远程终端通道（user_id → 长轮询/派发状态）
    pub client_hubs: RwLock<HashMap<String, remote_term::UserHub>>,
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
        // API key 路径：trk_ 前缀分流，合成 client scope claims（can_admin 恒 false）
        Some(t) if t.starts_with(api_keys::KEY_PREFIX) => {
            match api_keys::authenticate_api_key(&state, t).await {
                Ok((claims, identity)) => {
                    request.extensions_mut().insert(claims);
                    request.extensions_mut().insert(identity);
                    next.run(request).await
                }
                Err(msg) => {
                    tracing::warn!("auth failed: {msg}");
                    response::unauthorized(msg)
                }
            }
        }
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

    // 运维 provision 子命令：直连 DB 后即退出，不初始化日志与 HTTP 服务。
    // 例：hank-server create-api-key --username <名> --name <key名>
    if let Some(cmd) = std::env::args().nth(1) {
        if api_keys::is_provision_command(&cmd) {
            let config = Config::load()?;
            let db = Database::new(&config.server.database_url).await?;
            return api_keys::run_provision(&db, &cmd, &std::env::args().skip(2).collect::<Vec<_>>())
                .await;
        }
    }

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
        quant_grant_store: Arc::new(code_tools::quant_grant::QuantGrantStore::new()),
        tasks: Arc::new(task_state::TaskRegistry::new()),
        client_hubs: RwLock::new(HashMap::new()),
    });

    // 一次性收尾：过期 pending → expired；卡住的 answered 僵尸 → pending；
    // executing 僵尸 → failed（执行进程已随重启消失，不能永远留在中间态）。
    // 不进 scheduler——scheduler_enabled=false 时也要跑；与部署恢复同级。
    // 飞书长连接尚未启动，但 close_interaction_card 走 REST（自取 tenant token），
    // 与长连接无关，可直接改写过期卡片。
    match state.db.expire_stale_interactions().await {
        Ok(sweep) => {
            if !sweep.expired.is_empty() {
                tracing::info!(
                    count = sweep.expired.len(),
                    "启动收尾：已过期交互单标记为 expired"
                );
            }
            if sweep.reverted > 0 {
                tracing::info!(
                    count = sweep.reverted,
                    "启动收尾：answered 僵尸退回 pending（派发未完成可重试）"
                );
            }
            if sweep.failed > 0 {
                tracing::info!(
                    count = sweep.failed,
                    "启动收尾：executing 僵尸标记为 failed（执行进程已中断）"
                );
            }
            for (interaction_id, card_message_id) in &sweep.expired {
                if card_message_id.as_deref().map_or(true, str::is_empty) {
                    continue;
                }
                if let Err(e) = interaction_flow::close_interaction_card(
                    &state,
                    interaction_id,
                    card_message_id.as_deref(),
                    "已超时",
                    "系统",
                )
                .await
                {
                    tracing::warn!(
                        interaction_id = %interaction_id,
                        "过期回收改写飞书卡片失败: {e:#}"
                    );
                }
            }
        }
        Err(e) => tracing::warn!("启动收尾 expire_stale_interactions 失败: {e:#}"),
    }

    match state.db.cleanup_feishu_card_actions().await {
        Ok(n) if n > 0 => tracing::info!(count = n, "启动收尾：清理过期飞书卡片按钮 payload"),
        Ok(_) => {}
        Err(e) => tracing::warn!("清理飞书卡片按钮 payload 失败: {e:#}"),
    }

    // 启动微信 bot 长轮询（为每个 enabled 账号起一个 monitor task）
    weixin::monitor::start_monitors(state.clone());

    // 启动飞书 WS 长连接（为每个 enabled 账号起一个 monitor task）
    feishu::monitor::start_monitors(state.clone());

    // 恢复跨越 server 自身重启的独立部署任务监听。

    // 启动定时任务调度器（cron 驱动的系统主动工作入口）
    scheduler::start(state.clone());


    // Public routes (no auth required)
    let public = Router::new()
        .route("/api/health", get(routes::health))
        .route("/api/auth/login", post(routes::login))
        // handy webhook：无 JWT，handler 内按 user_id 解析账号后自行 HMAC-SHA256 验签
        .route(
            "/api/channels/handy/{user_id}/webhook",
            post(handy::webhook::webhook_handler),
        );

    // Protected routes (auth required)
    let protected = Router::new()
        .route("/api/auth/whoami", get(routes::whoami))
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
        // client 级交互单端点（第三方系统驱动 trace；归属校验在 handler 内）
        .route(
            "/api/sessions/{id}/interactions",
            get(interactions::list_session_interactions),
        )
        .route(
            "/api/sessions/{id}/interactions/{iid}/answer",
            post(interactions::answer_session_interaction),
        )
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
        // Handy routes (client)：用户级 handy 连接配置管理
        .route("/api/handy/account", get(handy::routes::get_account))
        .route("/api/handy/account", put(handy::routes::put_account))
        .route(
            "/api/handy/account/test",
            post(handy::routes::test_account),
        )
        // 桌面 client 远程终端通道（注册 + 长轮询 + 结果回传）
        .route(
            "/api/client/registration",
            put(remote_term::register_client),
        )
        .route("/api/client/poll", get(remote_term::poll_requests))
        .route(
            "/api/client/tool-result",
            post(remote_term::post_tool_result),
        )
        .route("/api/client/online", get(remote_term::list_online))
        // App 产品：用户作用域远程终端代理 + WebRTC
        .route("/api/app/clients", get(app_web::list_clients))
        .route(
            "/api/app/clients/{cid}",
            delete(app_web::delete_client),
        )
        .route(
            "/api/app/clients/{cid}/enabled",
            post(app_web::set_client_enabled),
        )
        .route(
            "/api/app/clients/{cid}/terminals",
            get(app_web::list_terminals).post(app_web::create_terminal),
        )
        .route(
            "/api/app/clients/{cid}/terminals/{tid}",
            delete(app_web::close_terminal),
        )
        .route(
            "/api/app/clients/{cid}/terminals/{tid}/output",
            get(app_web::terminal_output),
        )
        .route(
            "/api/app/clients/{cid}/terminals/{tid}/input",
            post(app_web::terminal_input),
        )
        .route(
            "/api/app/clients/{cid}/terminals/{tid}/resize",
            post(app_web::terminal_resize),
        )
        .route(
            "/api/app/clients/{cid}/rtc/offer",
            post(app_web::rtc_offer),
        )
        .route("/api/app/rtc/ice", get(app_web::rtc_ice))
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
        .route("/api/admin/jobs/{id}", patch(scheduler::routes::update_job))
        .route(
            "/api/admin/jobs/{id}/runs",
            get(scheduler::routes::job_runs),
        )
        .route("/api/admin/jobs/{id}/run", post(scheduler::routes::run_job))
        // 交互单管理（列表/详情/手动应答/取消；应答会真派发 resume）
        .route(
            "/api/admin/interactions",
            get(interactions::list_interactions),
        )        .route(
            "/api/admin/interactions/{id}",
            get(interactions::get_interaction),
        )
        .route(
            "/api/admin/interactions/{id}/answer",
            post(interactions::answer_interaction),
        )
        .route(
            "/api/admin/interactions/{id}/cancel",
            post(interactions::cancel_interaction),
        )
        // API key 管理（创建/列表/吊销；明文只在创建响应出现一次）
        .route("/api/admin/api-keys", post(api_keys::create_api_key))
        .route("/api/admin/api-keys", get(api_keys::list_api_keys))
        .route(
            "/api/admin/api-keys/{id}/revoke",
            post(api_keys::revoke_api_key),
        )
        .layer(middleware::from_fn(admin_required))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // Static file serving for admin / app SPA
    // 注意用 fallback（保留 200 状态），not_found_service 会把 SPA 路由也标成 404
    let admin_static =
        ServeDir::new("admin/dist").fallback(ServeFile::new("admin/dist/index.html"));
    let app_static =
        ServeDir::new("app/dist").fallback(ServeFile::new("app/dist/index.html"));

    let app = public
        .merge(protected)
        .merge(admin_api)
        .nest_service("/admin", admin_static)
        .nest_service("/app", app_static)
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
