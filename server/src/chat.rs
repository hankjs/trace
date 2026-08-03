use crate::provider_registry;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        IntoResponse,
    },
};
use code_agent::{AgentEvent, AgentSession, AskUserQuestion};
use code_tools::{
    ask_user::AskUserTool,
    explore_tools::FinalizeExploreTool,
    file_checksum::new_checksum_store,
    generate_tools::GenerateArtifactsTool,
    git::GitTool,
    list_directory::ListDirectoryTool,
    quant_tools::quant_tools,
    read_file::ReadFileTool,
    search::SearchTool,
    shell::ShellTool,
    spec_tools::{UpdateArtifactTool, UpdateSpecTool, UpdateTaskStatusTool},
    str_replace::StrReplaceTool,
    suggest_actions::SuggestActionsTool,
    test_runner::TestRunnerTool,
    web_fetch::WebFetchTool,
    write_file::WriteFileTool,
    PermissionConfig, PermissionMode, Tool,
};
use futures::stream::Stream;
use hank_db::NewInteraction;
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{error, Instrument};

// --- quant-research skill 内建文档（仅在有 quant 工具的会话注入） ---
const QUANT_RESEARCH_SKILL_MD: &str = include_str!("../skills/quant-research/SKILL.md");
const QUANT_RESEARCH_SKILL_NAME: &str = "quant-research";
const QUANT_RESEARCH_SKILL_DESC: &str = "Trace 内置 A2A 量化研究 Agent：在 catalog/validate/experiment/trial/backtest/factor 工具链上执行可检验假设，遵守停止条件与 findings 强制落表，不输出交易指令。";
const QUANT_RESEARCH_SKILL_PATH: &str = "skills/quant-research";

/// 构造 quant-research skill 索引与 project segment；仅在有 quant 工具的会话注入。
fn quant_research_prompt_inputs(
    quant_tools_added: bool,
) -> (Vec<code_agent::SkillInfo>, Vec<code_agent::PromptSegment>) {
    let skills = if quant_tools_added {
        vec![code_agent::SkillInfo {
            name: QUANT_RESEARCH_SKILL_NAME.to_string(),
            description: QUANT_RESEARCH_SKILL_DESC.to_string(),
            path: QUANT_RESEARCH_SKILL_PATH.to_string(),
        }]
    } else {
        vec![]
    };
    let mut segments = Vec::new();
    if quant_tools_added {
        segments.push(code_agent::PromptSegment::Static(QUANT_RESEARCH_SKILL_MD));
    }
    (skills, segments)
}

/// quant_research 话题的 project segment 文案。
/// 产品口径：只提供研究信息与模拟结果，不得写成交易建议（见 quant/PRODUCT.md）。
fn quant_research_session_prompt() -> &'static str {
    "路由 Agent 已将当前话题标记为 quant 研究话题。你没有工作目录，也没有\
     shell、文件或 Git 工具，只能通过 quant_* 工具读取数据与执行研究操作。\
     回答用中文，说明结论用了哪一天的数据、命中了什么条件。\
     quant 只提供研究信息与模拟结果：不要输出买卖指令，也不要暗示自动交易。\
     若用户想修改 quant 源码或看板，提示用 /new 开启新话题重新路由。"
}

// --- Event Buffer types ---

#[derive(Clone, Debug)]
pub struct EventEntry {
    pub id: u64,
    pub event: AgentEvent,
}

pub struct EventBuffer {
    pub events: Vec<EventEntry>,
    pub next_id: u64,
    pub completed: bool,
    pub tx: broadcast::Sender<EventEntry>,
}

impl EventBuffer {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            events: Vec::new(),
            next_id: 1,
            completed: false,
            tx,
        }
    }

    pub fn push(&mut self, event: AgentEvent) -> EventEntry {
        let id = self.next_id;
        self.next_id += 1;
        let entry = EventEntry { id, event };
        self.events.push(entry.clone());
        let _ = self.tx.send(entry.clone());
        entry
    }
}

/// Chat 的 AgentSession 会在 `run()` 返回前发出 TurnComplete，但服务端要在
/// `run()` 返回后才保存完整消息链。这里暂不广播终态，避免客户端立即回复
/// ask_user 时读到旧 active_leaf，形成孤立的 tool_result。
fn push_chat_stream_event(buffer: &mut EventBuffer, event: AgentEvent) {
    if !matches!(event, AgentEvent::TurnComplete) {
        buffer.push(event);
    }
}

/// event_tx 关闭意味着 Agent 已返回且消息链已保存，此时才关闭 SSE。
fn finish_chat_stream(buffer: &mut EventBuffer) {
    if !buffer.completed {
        buffer.push(AgentEvent::TurnComplete);
        buffer.completed = true;
    }
}

// --- Request types ---

#[derive(Deserialize)]
pub struct ChatRequest {
    pub content: String,
    pub images: Option<Vec<ImagePayload>>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub parent_id: Option<String>,
    pub apply_change_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ImagePayload {
    pub media_type: String,
    pub data: String,
}

// --- Chat turn core (decoupled from HTTP) ---

/// Options for a single chat turn, supplied by the caller (HTTP handler,
/// WeChat bot, ...).
pub struct ChatTurnOpts {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub parent_id: Option<String>,
    pub apply_change_id: Option<String>,
    /// JWT carried by spec-family tools when they call back into this server.
    pub auth_token: String,
    /// 渠道可选的额外 system prompt 片段（追加为 Dynamic PromptSegment）。
    /// 例如飞书 conversation 会话注入链路说明与 hank-cli 节点快照。
    pub extra_prompt_segments: Vec<String>,
}

/// Handle returned by [`run_chat_turn`]. The receiver is subscribed to the
/// session's EventBuffer BEFORE the agent task starts, so the caller sees
/// every event from the beginning. The stream ends when the buffer's
/// broadcast channel closes after the turn completes.
pub struct ChatTurnHandle {
    pub event_rx: broadcast::Receiver<EventEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum ChatTurnError {
    #[error("No enabled providers available")]
    NoProviders,
    #[error("{0}")]
    ExternalAgent(String),
    /// 用户作答格式错误等可直接回显的业务错误（交互单保持 pending，不启 agent）。
    #[error("{0}")]
    UserFacing(String),
}

/// Extract the plain text of a content-block list (used for checkpoint
/// labels, session titles and ask_user tool_result payloads).
fn text_from_blocks(blocks: &[hank_provider::ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            hank_provider::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Run one full chat turn for a session, independent of any HTTP request.
///
/// Persists agent events/metrics, updates the session's EventBuffer (the
/// resume endpoint depends on it), and registers the cancellation token in
/// `state.active_tasks` exactly like the HTTP chat endpoint does.
pub async fn run_chat_turn(
    state: &Arc<AppState>,
    session_id: &str,
    content: Vec<hank_provider::ContentBlock>,
    opts: ChatTurnOpts,
) -> Result<ChatTurnHandle, ChatTurnError> {
    let session_id = session_id.to_string();

    let session_record = state.db.get_session(&session_id).await.ok().flatten();
    let session_metadata_value = session_record
        .as_ref()
        .and_then(|session| session.metadata.as_deref())
        .and_then(|metadata| serde_json::from_str::<serde_json::Value>(metadata).ok());
    let agent_backend = session_metadata_value
        .as_ref()
        .and_then(|metadata| metadata["agent_backend"].as_str())
        .unwrap_or("native");
    // 外部代码 Agent：client-only 会话在 cli_agent 内强制 remote agent_run；
    // server bubblewrap 路径仅保留兼容，飞书不再创建。
    if matches!(agent_backend, "codex" | "claude" | "grok" | "kimi") {
        return crate::cli_agent::run_cli_turn(
            state,
            &session_id,
            session_record,
            content,
            agent_backend,
        )
        .await
        .map_err(|error| ChatTurnError::ExternalAgent(format!("{error:#}")));
    }

    // Resolve providers with fallback from DB
    let fallback_list = match opts.provider.as_deref() {
        Some(name) => provider_registry::resolve_with_fallback(&state.db, name).await,
        None => {
            let all = state.db.list_providers_ordered().await.unwrap_or_default();
            all.into_iter()
                .filter(|r| r.enabled)
                .map(|r| {
                    let p = provider_registry::build_provider_from_record(&r);
                    (r, p)
                })
                .collect()
        }
    };
    if fallback_list.is_empty() {
        return Err(ChatTurnError::NoProviders);
    }

    // Determine model from the first (preferred) provider record
    let first_record = &fallback_list[0].0;
    let model = match &opts.model {
        Some(m) => provider_registry::resolve_model(first_record, m),
        None => provider_registry::resolve_default_model(first_record),
    };

    // 渠道会话（飞书 / 微信）建表时不知道会落到哪个 provider，这里把实际解析结果回写，
    // admin 才能看出这轮是 native 的哪个 provider/model 在跑。
    if let Some(session) = session_record.as_ref() {
        if session.provider != first_record.name || session.model != model {
            if let Err(error) = state
                .db
                .update_session_provider_model(&session_id, &first_record.name, &model)
                .await
            {
                tracing::warn!(session_id = %session_id, "回写会话 provider/model 失败: {error:#}");
            }
        }
    }

    let work_dir = session_record.as_ref().and_then(|s| s.work_dir.clone());
    let work_dir_for_checkpoint = work_dir.clone();
    let work_dir_for_agent = work_dir.clone();
    let session_change_id = session_record.as_ref().and_then(|s| s.change_id.clone());
    let session_type = session_record
        .as_ref()
        .map(|s| s.session_type.clone())
        .unwrap_or_else(|| "chat".to_string());
    // 远程执行：会话绑定的桌面 client（None = server 本地执行）
    let exec_client_id = session_record
        .as_ref()
        .and_then(|s| s.exec_client_id.clone());
    let session_user_id = session_record.as_ref().and_then(|s| s.user_id.clone());
    let session_metadata = session_record.as_ref().and_then(|s| s.metadata.as_deref());
    let server_agent_session = session_metadata_value
        .as_ref()
        .and_then(|metadata| metadata["server_agent"].as_bool())
        .unwrap_or(false);
    let repository_workspace =
        crate::deployment::is_repository_workspace_metadata(session_metadata);
    let routed_agent_kind = session_metadata_value
        .as_ref()
        .and_then(|metadata| metadata["agent_kind"].as_str());
    let conversation_agent = routed_agent_kind
        .map(|kind| kind == "conversation")
        .unwrap_or(false);
    let research_agent = routed_agent_kind
        .map(|kind| kind == "quant_research")
        .unwrap_or(false);
    let quant_code_agent = routed_agent_kind
        .map(|kind| kind == "quant_code")
        .unwrap_or(false);
    let legacy_repository_agent = repository_workspace && routed_agent_kind.is_none();

    // 待确认状态统一从 agent_interactions 表读取（不再读 sessions.pending_ask_user
    // 或进程内 map）。session 重建也不丢单——交互单有自己的主键。
    let pending_ask_user = resolve_pending_ask_user(state, &session_id).await;

    let parent_id_for_new_msg = match opts.parent_id.as_deref() {
        Some("root") => None,
        Some(id) => Some(id.to_string()),
        None => session_record
            .as_ref()
            .and_then(|s| s.active_leaf_id.clone()),
    };

    // 标记本会话是否注册了 quant_* 工具，用于后续决定是否注入 quant-research skill 全文。
    let mut quant_tools_added = false;
    // metadata.source 决定确认话术与是否允许「确认N次」批量授权（weixin 禁止批量）。
    // 提到 if 之前，research 与全量工具分支共用，避免复制解析。
    let metadata_source = session_record
        .as_ref()
        .and_then(|s| s.metadata.as_deref())
        .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
        .and_then(|v| v["source"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "trace_chat".to_string());
    let token = opts.auth_token.clone();

    let tools: Vec<Arc<dyn Tool>> = if conversation_agent {
        // 路由 Agent 已将该话题标记为纯对话：没有文件系统工作区，也不暴露 shell/Git
        // 等工具，避免无 cwd 的会话意外操作 server 进程目录。
        Vec::new()
    } else if research_agent {
        // quant 研究话题：只挂 quant_* 工具与 ask_user / web_fetch；没有工作目录，
        // 不注册 shell/文件/Git/测试工具（挂了必然报错或误导模型）。
        let mut t: Vec<Arc<dyn Tool>> = vec![Arc::new(AskUserTool::new())];
        t.push(Arc::new(WebFetchTool::new()));
        if let Some(ref quant_cfg) = state.config.quant_a2a {
            if quant_cfg.enabled && !token.is_empty() {
                let mut quant = quant_tools(
                    quant_cfg.base_url.clone(),
                    token.clone(),
                    session_id.clone(),
                    metadata_source.clone(),
                    state.quant_grant_store.clone(),
                );
                t.append(&mut quant);
                quant_tools_added = true;
            }
        }
        if !quant_tools_added {
            // 路由本应在 quant_a2a 关闭时挡住 quant_research；落到此属防御异常。
            tracing::warn!(
                session_id = %session_id,
                "quant_research session without quant tools (quant_a2a disabled or empty token)"
            );
        }
        t
    } else {
        let base_url = format!("http://127.0.0.1:{}", state.config.server.port);
        let checksum_store = new_checksum_store();
        let execution_user =
            server_agent_session.then(|| state.config.server_agent.execution_user.clone());
        // fs/shell 类工具：绑定 exec_client_id 的会话改用远程代理工具，
        // 在桌面 client 本地执行；test_runner 远程会话不提供（可用 shell 跑测试）
        let mut t: Vec<Arc<dyn Tool>> = match (&exec_client_id, &session_user_id) {
            (Some(client_id), Some(user_id)) => {
                let mut remote = crate::remote_tools::remote_tool_set(
                    state.clone(),
                    user_id,
                    client_id,
                    work_dir.clone(),
                );
                remote.push(Arc::new(WebFetchTool::new()));
                remote
            }
            _ => {
                let shell = match &execution_user {
                    Some(user) => ShellTool::new_as_user(work_dir.clone(), user.clone()),
                    None => ShellTool::new(work_dir.clone()),
                };
                let git = match &execution_user {
                    Some(user) => GitTool::new_as_user(work_dir.clone(), user.clone()),
                    None => GitTool::new(work_dir.clone()),
                };
                let test_runner = match &execution_user {
                    Some(user) => TestRunnerTool::new_as_user(work_dir.clone(), user.clone()),
                    None => TestRunnerTool::new(work_dir.clone()),
                };
                vec![
                    Arc::new(shell),
                    Arc::new(ReadFileTool::with_checksum_store(
                        work_dir.clone(),
                        checksum_store.clone(),
                    )),
                    Arc::new(WriteFileTool::with_checksum_store(
                        work_dir.clone(),
                        checksum_store.clone(),
                    )),
                    Arc::new(StrReplaceTool::with_checksum_store(
                        work_dir.clone(),
                        checksum_store.clone(),
                    )),
                    Arc::new(ListDirectoryTool::new(work_dir.clone())),
                    Arc::new(SearchTool::new(work_dir.clone())),
                    Arc::new(git),
                    Arc::new(WebFetchTool::new()),
                    Arc::new(test_runner),
                ]
            }
        };
        t.push(Arc::new(UpdateSpecTool::new(
            base_url.clone(),
            token.clone(),
            session_id.clone(),
        )));
        t.push(Arc::new(UpdateTaskStatusTool::new(
            base_url.clone(),
            token.clone(),
            session_id.clone(),
        )));
        t.push(Arc::new(UpdateArtifactTool::new(
            base_url.clone(),
            token.clone(),
            session_id.clone(),
        )));
        t.push(Arc::new(AskUserTool::new()));
        // 收尾建议动作：不中断循环；quant_research 精简工具集不注册（纯查询不该提议代码动作）
        t.push(Arc::new(SuggestActionsTool::new()));
        // 截图类工具永远 server 本地执行：网页快照用 server 本机 Chrome，
        // 终端截图由 server 拉 client 快照后本地渲染（无 user_id 的会话无法定位 client，不注册）
        t.push(Arc::new(crate::snap_tools::WebSnapshotTool::new(
            state.config.server.chrome_path.clone(),
        )));
        if let Some(ref uid) = session_user_id {
            t.push(Arc::new(crate::snap_tools::TerminalSnapshotTool::new(
                state.clone(),
                uid.clone(),
            )));
        }
        // Add explore/generate tools if session is bound to a change or is explore type
        if let Some(ref cid) = session_change_id {
            t.push(Arc::new(FinalizeExploreTool::new(
                base_url.clone(),
                token.clone(),
                cid.clone(),
                session_id.clone(),
            )));
            t.push(Arc::new(GenerateArtifactsTool::new(
                base_url.clone(),
                token.clone(),
                cid.clone(),
            )));
        } else if session_type == "explore" {
            // Explore session without a change yet — finalize_explore will create the change
            t.push(Arc::new(FinalizeExploreTool::new(
                base_url.clone(),
                token.clone(),
                String::new(),
                session_id.clone(),
            )));
        }

        // 注册 quant_* 工具：需 quant_a2a.enabled 显式开启、用户已登录（有 JWT）、且会话来源明确
        if let Some(ref quant_cfg) = state.config.quant_a2a {
            if quant_cfg.enabled && !token.is_empty() {
                let mut quant = quant_tools(
                    quant_cfg.base_url.clone(),
                    token.clone(),
                    session_id.clone(),
                    metadata_source.clone(),
                    state.quant_grant_store.clone(),
                );
                t.append(&mut quant);
                quant_tools_added = true;
            }
        }

        t
    };

    // Initialize event buffer for this session
    {
        let mut buffers = state.event_buffers.write().await;
        buffers.insert(session_id.clone(), EventBuffer::new());
    }

    // Subscribe to the buffer's broadcast BEFORE spawning the task
    let rx = {
        let buffers = state.event_buffers.read().await;
        buffers.get(&session_id).unwrap().tx.subscribe()
    };

    // Set up internal channel for agent -> buffer forwarding
    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(64);
    let db = state.db.clone();
    let sid = session_id.clone();
    let content_text = text_from_blocks(&content);
    let apply_change_id = opts.apply_change_id.clone();
    let extra_prompt_segments = opts.extra_prompt_segments;

    // If pending_ask_user, the user's reply becomes a tool_result
    let user_content: Vec<hank_provider::ContentBlock> = if let Some(ref pending_json) =
        pending_ask_user
    {
        let pending: serde_json::Value = serde_json::from_str(pending_json).unwrap_or_default();
        let tool_use_id = pending["tool_use_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let interaction_id = pending["interaction_id"].as_str().unwrap_or("").to_string();
        let answered_by = session_user_id.as_deref().unwrap_or("");

        // 多问题：从 pending 取 questions（resolve_pending_ask_user 已塞进 JSON）
        let multi_questions = parse_questions_from_pending(&pending);

        // 文字回复路径：仍为 pending，在此原子应答；按钮回调路径：已是 answered。
        // 多问题文字作答先校验格式——失败直接回用户、保持 pending，不启 agent。
        let (answer_blocked, multi_answer_pairs) =
            if pending["status"].as_str() == Some("pending") && !interaction_id.is_empty() {
                if !multi_questions.is_empty() {
                    match parse_multi_answer(&multi_questions, &content_text) {
                        Ok(pairs) => {
                            let full = format_multi_answer_token_string(&multi_questions, &pairs);
                            // answer 列 VARCHAR(64)：完整串另存 final_answer，列内截断
                            let for_db = truncate_answer_for_column(&full);
                            if let Err(e) = state
                                .db
                                .set_interaction_final_answer(&interaction_id, &full)
                                .await
                            {
                                tracing::warn!(
                                    interaction_id = %interaction_id,
                                    "set_interaction_final_answer 失败: {e:#}"
                                );
                            }
                            match state
                                .db
                                .answer_interaction(&interaction_id, &for_db, answered_by)
                                .await
                            {
                                Ok(Some(_)) => (None, Some(pairs)),
                                Ok(None) => {
                                    let expired = pending_expires_at_is_past(&pending);
                                    (
                                        Some(if expired {
                                            "待确认已超时，未执行。如需执行请重新发起。".to_string()
                                        } else {
                                            "这个操作已经提交过了。".to_string()
                                        }),
                                        None,
                                    )
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        interaction_id = %interaction_id,
                                        "answer_interaction 失败: {e:#}"
                                    );
                                    (Some(format!("确认写入失败：{e:#}")), None)
                                }
                            }
                        }
                        // 格式错误：交互单保持 pending 等重答；不注入 tool_result、不启 agent。
                        Err(msg) => {
                            return Err(ChatTurnError::UserFacing(msg));
                        }
                    }
                } else {
                    let for_db = truncate_answer_for_column(&content_text);
                    match state
                        .db
                        .answer_interaction(&interaction_id, &for_db, answered_by)
                        .await
                    {
                        Ok(Some(_)) => (None, None),
                        Ok(None) => {
                            let expired = pending_expires_at_is_past(&pending);
                            (
                                Some(if expired {
                                    "待确认已超时，未执行。如需执行请重新发起。".to_string()
                                } else {
                                    "这个操作已经提交过了。".to_string()
                                }),
                                None,
                            )
                        }
                        Err(e) => {
                            tracing::warn!(
                                interaction_id = %interaction_id,
                                "answer_interaction 失败: {e:#}"
                            );
                            (Some(format!("确认写入失败：{e:#}")), None)
                        }
                    }
                }
            } else {
                (None, None)
            };

        let content = if let Some(msg) = answer_blocked {
            msg
        } else if let Some(kind) = pending["kind"].as_str() {
            if let Some(source) = kind.strip_prefix("quant_confirm:") {
                handle_quant_confirmation(state, &session_id, source, &pending, &content_text).await
            } else if let Some(ref pairs) = multi_answer_pairs {
                format_multi_answer_human(&multi_questions, pairs)
            } else if !multi_questions.is_empty() {
                // 按钮路径：status 已是 answered，content_text 是 "1A 2B" 串
                // 优先用 final_answer，再回落 content_text
                let answer_src = pending["final_answer"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(content_text.as_str());
                match parse_multi_answer(&multi_questions, answer_src) {
                    Ok(pairs) => format_multi_answer_human(&multi_questions, &pairs),
                    Err(_) => content_text.clone(),
                }
            } else {
                content_text.clone()
            }
        } else if let Some(ref pairs) = multi_answer_pairs {
            format_multi_answer_human(&multi_questions, pairs)
        } else if !multi_questions.is_empty() {
            let answer_src = pending["final_answer"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(content_text.as_str());
            match parse_multi_answer(&multi_questions, answer_src) {
                Ok(pairs) => format_multi_answer_human(&multi_questions, &pairs),
                Err(_) => content_text.clone(),
            }
        } else {
            content_text.clone()
        };

        // resume 消费后标 done，避免同一交互单被再次当成待确认
        if !interaction_id.is_empty() {
            let _ = state
                .db
                .update_interaction_status(&interaction_id, "done", Some(&content), None)
                .await;
        }

        vec![hank_provider::ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error: false,
        }]
    } else {
        content
    };
    let is_first_message = {
        let msgs = if let Some(ref leaf) = parent_id_for_new_msg {
            state
                .db
                .get_branch_messages(&session_id, leaf)
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        msgs.is_empty()
    };
    let parent_for_chain = parent_id_for_new_msg.clone();

    // Build history for the session
    let history: Vec<hank_provider::Message> = if let Some(ref leaf) = parent_id_for_new_msg {
        state
            .db
            .get_branch_messages(&session_id, leaf)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    }
    .iter()
    .filter_map(|m| {
        let content: Vec<hank_provider::ContentBlock> = serde_json::from_str(&m.content).ok()?;
        let role = match m.role.as_str() {
            "user" => hank_provider::Role::User,
            "assistant" => hank_provider::Role::Assistant,
            _ => return None,
        };
        Some(hank_provider::Message { role, content })
    })
    .collect();
    let history_len = history.len();

    // ─── Checkpoint: 在 agent 执行前创建快照 ─────────────────────────────
    if let Some(ref wd) = work_dir_for_checkpoint {
        let cp_state = state.clone();
        let cp_session_id = session_id.clone();
        let cp_message_id = parent_id_for_new_msg.clone().unwrap_or_default();
        let cp_work_dir = wd.clone();
        let cp_label: String = content_text.chars().take(40).collect();
        tokio::spawn(async move {
            if let Err(e) = crate::checkpoints::create_checkpoint_for_turn(
                &cp_state,
                &cp_session_id,
                &cp_message_id,
                &cp_work_dir,
                &cp_label,
            )
            .await
            {
                tracing::warn!(session_id = %cp_session_id, "checkpoint creation failed: {e:#}");
            }
        });
    }

    let cancel_token = CancellationToken::new();
    {
        let mut tasks = state.active_tasks.write().await;
        tasks.insert(session_id.clone(), cancel_token.clone());
    }
    let state_for_cleanup = state.clone();
    let sid_for_cleanup = session_id.clone();

    // Forwarder task: reads from agent mpsc, writes to EventBuffer + persists metrics
    let state_fwd = state.clone();
    let sid_fwd = session_id.clone();
    let db_fwd = state.db.clone();
    let sid_fwd2 = session_id.clone();
    // 交互单落表需要 user_id / channel；从会话记录取，避免从 session metadata 现读 resume 上下文
    let fwd_user_id = session_user_id.clone().unwrap_or_default();
    let fwd_channel_source = metadata_source.clone();
    let fwd_span = tracing::info_span!("chat_fwd", session_id = %session_id);
    tokio::spawn(
        async move {
            let mut seq = db_fwd
                .get_last_agent_event_seq(&sid_fwd2)
                .await
                .unwrap_or(0);
            while let Some(event) = event_rx.recv().await {
                seq += 1;

                // Persist event to agent_events table
                let event_type = extract_event_type(&event);
                if let Ok(payload) = serde_json::to_string(&event) {
                    let _ = db_fwd
                        .save_agent_event(&sid_fwd2, event_type, &payload, seq)
                        .await;
                }

                // Keep existing metric/tool persistence for backward compatibility
                match &event {
                    AgentEvent::Metrics {
                        input_tokens,
                        output_tokens,
                        latency_ms,
                        model,
                        provider,
                        phase: _,
                    } => {
                        let _ = db_fwd
                            .save_agent_metric(
                                &sid_fwd2,
                                None,
                                *input_tokens,
                                *output_tokens,
                                *latency_ms,
                                model,
                                provider,
                            )
                            .await;
                    }
                    AgentEvent::ToolMetrics {
                        tool_name,
                        duration_ms,
                        is_error,
                    } => {
                        let _ = db_fwd
                            .save_tool_execution(
                                &sid_fwd2,
                                None,
                                tool_name,
                                *duration_ms,
                                *is_error,
                            )
                            .await;
                    }
                    AgentEvent::AskUser {
                        question,
                        options,
                        tool_use_id,
                        kind,
                        questions,
                    } => {
                        // 两类 ask_user 统一落 agent_interactions：此前 quant_confirm 走进程内
                        // map、普通 ask_user 走 sessions 字段，都以 session_id 为 key，会话重建即丢单。
                        // 必须在 push 到 event_buffers 之前 await 写完，pusher 才能靠
                        // latest_pending_interaction 取到已落库的 interaction_id。
                        let (interaction_kind, source) = match kind.as_deref() {
                            Some(k) if k.starts_with("quant_confirm:") => (
                                "quant_confirm",
                                k.strip_prefix("quant_confirm:").unwrap_or("").to_string(),
                            ),
                            _ => ("ask_user", fwd_channel_source.clone()),
                        };
                        // 微信 5 分钟 TTL 是渠道特性；飞书/网页不过期。
                        let expires_at = (source == "weixin")
                            .then(|| chrono::Utc::now() + chrono::Duration::minutes(5));
                        let title: String = question
                            .lines()
                            .next()
                            .unwrap_or(question)
                            .chars()
                            .take(255)
                            .collect();
                        // 多问题：options 列存扁平合法答案全集（如 ["1A","1B","2A","2B"]），
                        // 结构存 resume_ref.questions；不要改 options 列语义。
                        let options_for_db: Vec<String> = if !questions.is_empty() {
                            flatten_question_options(questions)
                        } else {
                            options.clone()
                        };
                        let options_json = serde_json::to_string(&options_for_db)
                            .unwrap_or_else(|_| "[]".to_string());
                        let mut resume_val = serde_json::json!({
                            "tool_use_id": tool_use_id,
                            "source": source,
                            "question": question,
                        });
                        if !questions.is_empty() {
                            resume_val["questions"] = serde_json::to_value(questions)
                                .unwrap_or_else(|_| serde_json::json!([]));
                        }
                        let resume_ref = resume_val.to_string();
                        let channel = match source.as_str() {
                            "weixin" => "weixin",
                            "feishu" => "feishu",
                            _ => "trace_chat",
                        };
                        match db_fwd
                            .create_interaction(NewInteraction {
                                session_id: &sid_fwd2,
                                user_id: if fwd_user_id.is_empty() {
                                    "unknown"
                                } else {
                                    &fwd_user_id
                                },
                                channel,
                                account_id: None,
                                chat_id: None,
                                topic_id: None,
                                kind: interaction_kind,
                                title: &title,
                                goal: None,
                                analysis: None,
                                options: &options_json,
                                resume_ref: Some(&resume_ref),
                                expires_at,
                            })
                            .await
                        {
                            Ok(row) => {
                                tracing::info!(
                                    interaction_id = %row.id,
                                    session_id = %sid_fwd2,
                                    kind = interaction_kind,
                                    "交互单已落表"
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    session_id = %sid_fwd2,
                                    "create_interaction 失败: {e:#}"
                                );
                            }
                        }
                    }
                    _ => {}
                }
                let mut buffers = state_fwd.event_buffers.write().await;
                if let Some(buf) = buffers.get_mut(&sid_fwd) {
                    push_chat_stream_event(buf, event_for_stream(&event));
                }
            }

            // event_tx 只会在 Agent 返回、消息链持久化完成后关闭。将终态放在
            // 这里可保证客户端看到 TurnComplete 后立即回复也能恢复完整历史。
            let mut buffers = state_fwd.event_buffers.write().await;
            if let Some(buf) = buffers.get_mut(&sid_fwd) {
                finish_chat_stream(buf);
            }
        }
        .instrument(fwd_span),
    );

    // Agent task with fallback loop
    let agent_span = tracing::info_span!("chat_agent", session_id = %session_id);
    tokio::spawn(async move {
        let max_attempts = fallback_list.len().min(3);
        let mut last_error = String::new();

        // Business prompts are assembled by the client. The server keeps a
        // neutral fallback prompt while it forwards requests and records logs.
        let system_prompt = "You are a helpful AI assistant. Follow the user's message exactly.".to_string();

        // If apply_change_id provided, bind session to change
        if let Some(ref apply_cid) = apply_change_id {
            let _ = db.set_session_change_id(&sid, apply_cid).await;
        }

        for attempt in 0..max_attempts {
            let (ref record, ref provider) = fallback_list[attempt];
            let current_model = if attempt == 0 {
                model.clone()
            } else {
                provider_registry::resolve_default_model(record)
            };

            // Emit fallback event if not first attempt
            if attempt > 0 {
                let prev_name = &fallback_list[attempt - 1].0.name;
                let _ = event_tx.send(AgentEvent::ProviderFallback {
                    from: prev_name.clone(),
                    to: record.name.clone(),
                    reason: last_error.clone(),
                }).await;
            }

            let mut session = AgentSession::new(
                provider.clone(),
                tools.clone(),
                current_model,
                system_prompt.clone(),
            );
            // 接入权限模型：workspace-write 模式 + work_dir 作为可写根（FR-PERM-1/4）
            if let Some(ref wd) = work_dir_for_agent {
                if server_agent_session {
                    let mut permission = PermissionConfig::default();
                    permission.mode = PermissionMode::WorkspaceWrite;
                    permission.sandbox_paths = vec![wd.clone()];
                    permission.restrict_read_paths = true;
                    permission.blocked_commands.extend([
                        "sudo".to_string(),
                        "systemctl".to_string(),
                        "service ".to_string(),
                        "shutdown".to_string(),
                        "reboot".to_string(),
                        "kill ".to_string(),
                        "pkill ".to_string(),
                        "git reset --hard".to_string(),
                        "git clean -f".to_string(),
                        "git checkout --".to_string(),
                        "../".to_string(),
                        "/opt/hank".to_string(),
                        "/home/hank".to_string(),
                        "/etc/".to_string(),
                    ]);
                    if repository_workspace {
                        permission.blocked_paths.push(format!("{wd}/client"));
                        permission.blocked_commands.extend([
                            "client/".to_string(),
                            "cd client".to_string(),
                            "cd ./client".to_string(),
                            "config.toml".to_string(),
                            "trace-production".to_string(),
                            "update-ref".to_string(),
                        ]);
                    }
                    session = session.with_permission_config(permission, wd.clone());
                } else {
                    session = session.with_permission(PermissionMode::WorkspaceWrite, wd.clone());
                }
            }
            // 分层注入运行时 + 环境上下文（FR-CTX-1/2）。
            // base 沿用客户端组装的 system_prompt（业务提示词由客户端负责）。
            {
                // 仅在有 quant 工具的会话暴露 skill 索引与完整 skill 文档，避免污染其它会话。
                let (quant_skills, mut quant_segments) = quant_research_prompt_inputs(quant_tools_added);
                let runtime = code_agent::RuntimeContext {
                    permission_mode: "workspace-write".to_string(),
                    approval_policy: "auto".to_string(),
                    writable_roots: work_dir_for_agent.clone().into_iter().collect(),
                    network_policy: "restricted".to_string(),
                    tools: tools
                        .iter()
                        .map(|t| code_agent::ToolInfo {
                            name: t.name().to_string(),
                            description: t.description().to_string(),
                            risk: format!("{:?}", t.risk_level()),
                        })
                        .collect(),
                    skills: quant_skills,
                };
                let env = code_agent::EnvironmentContext {
                    cwd: work_dir_for_agent.clone(),
                    shell: "/bin/sh".to_string(),
                    current_date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
                    timezone: "UTC".to_string(),
                    repo_root: repository_workspace
                        .then(|| work_dir_for_agent.clone())
                        .flatten(),
                    sandbox_mode: "workspace-write".to_string(),
                    network_policy: "restricted".to_string(),
                };
                // 远程执行会话：向 agent 说明工作目录在用户本地桌面机器上
                let mut project_segments: Vec<code_agent::PromptSegment> = vec![];
                if exec_client_id.is_some() {
                    project_segments.push(code_agent::PromptSegment::Dynamic(
                        "执行环境说明：工作目录在用户的本地桌面机器上，文件与 shell 类工具\
                        通过远程执行通道在该机器上运行，所有路径均为该机器的本地路径。"
                            .to_string(),
                    ));
                }
                project_segments.append(&mut quant_segments);
                // 渠道注入的额外片段（如飞书 conversation 的链路说明与节点快照）
                for segment in &extra_prompt_segments {
                    project_segments.push(code_agent::PromptSegment::Dynamic(segment.clone()));
                }
                if conversation_agent {
                    project_segments.push(code_agent::PromptSegment::Dynamic(
                        "路由 Agent 已将当前话题标记为纯对话。没有工作目录，也没有本地执行工具；\
                        直接结合对话上下文回答用户。若用户后续转为需要文件、命令或 Trace/quant 代码的任务，\
                        请提示用户使用 /new 开启新话题，以便重新路由并分配合适工作区。"
                            .to_string(),
                    ));
                } else if research_agent {
                    project_segments.push(code_agent::PromptSegment::Dynamic(
                        quant_research_session_prompt().to_string(),
                    ));
                } else if repository_workspace {
                    project_segments.extend(load_server_agent_instructions(
                        &work_dir_for_agent,
                        quant_code_agent || legacy_repository_agent,
                    ));
                } else if server_agent_session {
                    project_segments.push(code_agent::PromptSegment::Dynamic(
                        "你正在 wananyun 的普通隔离工作区中工作。这个话题与 Trace/quant 仓库无关，\
                        不要访问 /opt/hank-src 或任何 Trace worktree，也不能使用 /diff、/test、/deploy、\
                        /rollback。该目录会在同一飞书话题中持续复用。"
                            .to_string(),
                    ));
                }
                let (_assembled, named) = code_agent::build_layered_prompt(
                    Some(&system_prompt),
                    Some(&runtime),
                    Some(&env),
                    &project_segments,
                );
                session = session.with_layered_prompt(named);
            }
            session.set_messages(history.clone());

            match session.run(user_content.clone(), event_tx.clone(), cancel_token.clone()).await {
                Ok(()) => {
                    // Success — save messages
                    let new_messages: Vec<_> = session.messages().iter().skip(history_len).collect();
                    if !new_messages.is_empty() {
                        let base_time = chrono::Utc::now();
                        let mut prev_id = parent_for_chain;
                        for (i, msg) in new_messages.iter().enumerate() {
                            let role = match msg.role {
                                hank_provider::Role::User => "user",
                                hank_provider::Role::Assistant => "assistant",
                            };
                            let content_val = serde_json::to_value(&msg.content).unwrap_or_default();
                            let ts = base_time + chrono::Duration::microseconds(i as i64);
                            match db.save_message(&sid, role, &content_val, ts, prev_id.as_deref()).await {
                                Ok(new_id) => prev_id = Some(new_id),
                                Err(_) => break,
                            }
                        }
                        if let Some(ref leaf) = prev_id {
                            let _ = db.update_active_leaf(&sid, leaf).await;
                        }
                        let _ = db.touch_session(&sid).await;
                    }

                    if is_first_message {
                        let title: String = content_text.chars().take(50).collect();
                        let _ = db.update_session_title(&sid, &title).await;
                    }
                    break;
                }
                Err(e) => {
                    last_error = format!("{e:#}");
                    let is_retryable = is_retryable_error(&last_error);

                    if !is_retryable || attempt == max_attempts - 1 {
                        // Non-retryable or last attempt — emit error
                        error!(session_id = %sid, provider = %record.name, "Agent error: {e:#}");
                        let _ = event_tx.send(AgentEvent::Error { message: format!("{e:#}") }).await;

                        let error_content = serde_json::json!([{"type": "error", "text": format!("{e:#}")}]);
                        let ts = chrono::Utc::now();
                        if let Ok(err_id) = db.save_message(&sid, "assistant", &error_content, ts, parent_for_chain.as_deref()).await {
                            let _ = db.update_active_leaf(&sid, &err_id).await;
                        }
                        let _ = db.touch_session(&sid).await;
                        break;
                    }
                    // Retryable — continue to next provider
                    tracing::warn!(provider = %record.name, "Provider failed, trying fallback: {}", last_error);
                }
            }
        }

        // Drop event_tx so forwarder finishes
        drop(event_tx);

        // Remove token from active tasks
        {
            let mut tasks = state_for_cleanup.active_tasks.write().await;
            tasks.remove(&sid_for_cleanup);
        }
    }.instrument(agent_span));

    Ok(ChatTurnHandle { event_rx: rx })
}

fn load_server_agent_instructions(
    work_dir: &Option<String>,
    include_quant: bool,
) -> Vec<code_agent::PromptSegment> {
    let Some(work_dir) = work_dir else {
        return Vec::new();
    };
    let root = std::path::Path::new(work_dir);
    let mut segments = Vec::new();
    let mut instruction_files = vec![
        ("AGENTS.md", root.join("AGENTS.md")),
        (
            "Server Agent 双向 Git 同步协议",
            root.join("docs/src/operations/server-agent-sync.md"),
        ),
    ];
    if include_quant {
        instruction_files.insert(1, ("quant/AGENTS.md", root.join("quant/AGENTS.md")));
    }
    for (name, path) in instruction_files {
        match std::fs::read_to_string(&path) {
            Ok(content) => segments.push(code_agent::PromptSegment::Dynamic(format!(
                "项目指令 {name}:\n{content}"
            ))),
            Err(e) => {
                tracing::debug!(path = %path.display(), "server agent instruction unavailable: {e}")
            }
        }
    }
    segments.push(code_agent::PromptSegment::Dynamic(
        "你正在 wananyun 的 server-only 工作区中工作。client/ 永远不在迭代和部署范围内；不要调用 sudo、systemctl 或直接修改运行目录。完成代码后使用 /diff、/test、/deploy，由用户在飞书审批部署。".to_string(),
    ));
    segments
}

// PLACEHOLDER_CHAT_HANDLER

pub async fn chat_handler(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<ChatRequest>,
) -> impl IntoResponse {
    // spec 类工具回调 server 自身时使用请求携带的 JWT
    let auth_token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();

    let mut blocks = vec![hank_provider::ContentBlock::Text { text: body.content }];
    if let Some(images) = body.images {
        for img in images {
            blocks.push(hank_provider::ContentBlock::Image {
                source: hank_provider::ImageSource {
                    source_type: "base64".to_string(),
                    media_type: img.media_type,
                    data: img.data,
                },
            });
        }
    }

    let opts = ChatTurnOpts {
        provider: body.provider,
        model: body.model,
        parent_id: body.parent_id,
        apply_change_id: body.apply_change_id,
        auth_token,
        extra_prompt_segments: Vec::new(),
    };

    match run_chat_turn(&state, &session_id, blocks, opts).await {
        Ok(handle) => {
            // Build SSE stream from broadcast receiver + heartbeat
            let stream = make_sse_stream(handle.event_rx);
            Sse::new(stream).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, session_id = %session_id, "chat turn failed to start");
            (
                StatusCode::BAD_REQUEST,
                "failed to start chat turn".to_string(),
            )
                .into_response()
        }
    }
}

/// Check if an error is retryable (network, rate limit, 5xx, auth issues).
fn is_retryable_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("connection")
        || lower.contains("timeout")
        || lower.contains("rate limit")
        || lower.contains("429")
        || lower.contains("500")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("504")
        || lower.contains("overloaded")
}

// PLACEHOLDER_MAKE_SSE

fn make_sse_stream(
    mut rx: broadcast::Receiver<EventEntry>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(15));

        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(entry) => {
                            let json = serde_json::to_string(&entry.event).unwrap_or_default();
                            yield Ok(Event::default().data(json).id(entry.id.to_string()));

                            // If this was TurnComplete, end the stream
                            if matches!(entry.event, AgentEvent::TurnComplete) {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("SSE client lagged by {n} events");
                            break;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            break;
                        }
                    }
                }
                _ = heartbeat_interval.tick() => {
                    yield Ok(Event::default().event("heartbeat").data("{}"));
                }
            }
        }
    }
}

// --- Resume handler ---

#[derive(Deserialize)]
pub struct ResumeQuery {
    pub last_event_id: u64,
}

pub async fn resume_handler(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<ResumeQuery>,
) -> impl IntoResponse {
    let last_id = query.last_event_id;

    // Get missed events and optionally subscribe for live events
    let (missed, rx, completed) = {
        let buffers = state.event_buffers.read().await;
        match buffers.get(&session_id) {
            Some(buf) => {
                let missed: Vec<EventEntry> = buf
                    .events
                    .iter()
                    .filter(|e| e.id > last_id)
                    .cloned()
                    .collect();
                let rx = if !buf.completed {
                    Some(buf.tx.subscribe())
                } else {
                    None
                };
                (missed, rx, buf.completed)
            }
            None => {
                return (StatusCode::NOT_FOUND, "No event buffer for session").into_response();
            }
        }
    };

    let stream = make_resume_stream(missed, rx, completed, last_id);
    Sse::new(stream).into_response()
}

fn make_resume_stream(
    missed: Vec<EventEntry>,
    rx: Option<broadcast::Receiver<EventEntry>>,
    completed: bool,
    last_id: u64,
) -> impl Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        // First replay missed events
        for entry in &missed {
            let json = serde_json::to_string(&entry.event).unwrap_or_default();
            yield Ok(Event::default().data(json).id(entry.id.to_string()));
        }

        // If session already completed and TurnComplete wasn't in missed, send it
        if completed {
            let has_turn_complete = missed.iter().any(|e| matches!(e.event, AgentEvent::TurnComplete));
            if !has_turn_complete {
                let json = serde_json::to_string(&AgentEvent::TurnComplete).unwrap_or_default();
                yield Ok(Event::default().data(json).id("end".to_string()));
            }
            return;
        }

        // Subscribe to live events
        if let Some(mut rx) = rx {
            let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(15));
            // Determine the highest ID we've already sent
            let mut max_sent = missed.last().map(|e| e.id).unwrap_or(last_id);

            loop {
                tokio::select! {
                    result = rx.recv() => {
                        match result {
                            Ok(entry) => {
                                if entry.id <= max_sent {
                                    continue; // already sent during replay
                                }
                                max_sent = entry.id;
                                let json = serde_json::to_string(&entry.event).unwrap_or_default();
                                yield Ok(Event::default().data(json).id(entry.id.to_string()));
                                if matches!(entry.event, AgentEvent::TurnComplete) {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    _ = heartbeat_interval.tick() => {
                        yield Ok(Event::default().event("heartbeat").data("{}"));
                    }
                }
            }
        }
    }
}

// --- Stop handler ---

pub async fn stop_handler(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let tasks = state.active_tasks.read().await;
    if let Some(token) = tasks.get(&session_id) {
        token.cancel();
        StatusCode::OK
    } else {
        StatusCode::OK
    }
}

// --- Quant confirmation helpers ---

/// 从 agent_interactions 表取本会话最近可恢复的交互单，映射为调用方期望的 JSON 形状。
///
/// 返回字段保持 tool_use_id / question / options / kind，另附 interaction_id /
/// status / expires_at 供应答与超时判断。多问题时附 questions / final_answer。
/// 不再读 sessions.pending_ask_user。
async fn resolve_pending_ask_user(state: &Arc<AppState>, session_id: &str) -> Option<String> {
    let row = match state.db.latest_pending_interaction(session_id).await {
        Ok(v) => v?,
        Err(e) => {
            tracing::warn!(session_id, "latest_pending_interaction 失败: {e:#}");
            return None;
        }
    };
    let resume: serde_json::Value =
        serde_json::from_str(row.resume_ref.as_deref().unwrap_or("{}")).unwrap_or_default();
    let options: serde_json::Value =
        serde_json::from_str(&row.options).unwrap_or_else(|_| serde_json::json!([]));
    let source = resume["source"].as_str().unwrap_or("");
    let kind = if row.kind == "quant_confirm" {
        format!("quant_confirm:{source}")
    } else {
        row.kind.clone()
    };
    let question = resume["question"]
        .as_str()
        .unwrap_or(row.title.as_str())
        .to_string();
    let mut payload = serde_json::json!({
        "tool_use_id": resume["tool_use_id"].as_str().unwrap_or(""),
        "question": question,
        "options": options,
        "kind": kind,
        "interaction_id": row.id,
        "status": row.status,
        "answer": row.answer,
        "expires_at": row.expires_at.map(|t| t.to_rfc3339()),
        "created_at_ms": row.created_at.timestamp_millis(),
    });
    if let Some(qs) = resume.get("questions") {
        payload["questions"] = qs.clone();
    }
    if let Some(fa) = resume.get("final_answer") {
        payload["final_answer"] = fa.clone();
    }
    Some(payload.to_string())
}

// ── 多问题 ask_user 纯函数 ──────────────────────────────────────────────

/// answer 列是 VARCHAR(64)。完整作答串写入前按 60 字符截断；
/// 完整版应另存 resume_ref.final_answer，resume 时读那份。不改列宽（迁移成本，
/// 且 64 对单问题场景够用）。
pub fn truncate_answer_for_column(answer: &str) -> String {
    answer.chars().take(60).collect()
}

/// 多问题的合法答案全集：每题每选项一个 token，形如 "1A"。
/// 选项超过 26 个的题按前 26 个处理（A-Z 用尽）——实际上限是 4，不会触发。
pub fn flatten_question_options(questions: &[AskUserQuestion]) -> Vec<String> {
    let mut out = Vec::new();
    for q in questions {
        for (i, _) in q.options.iter().enumerate().take(26) {
            let letter = (b'A' + i as u8) as char;
            out.push(format!("{}{letter}", q.id));
        }
    }
    out
}

/// 从 pending JSON 解析 questions 数组。
fn parse_questions_from_pending(pending: &serde_json::Value) -> Vec<AskUserQuestion> {
    pending
        .get("questions")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// 解析用户的文字作答，如 "1A 2B" / "1a2b" / "1A，2B"。
/// 返回 Ok(题号→选项文案) 或 Err(给用户看的中文错误)。
///
/// 容错：忽略大小写、允许中英文逗号/空格/无分隔符；
/// 拒绝：未知题号、选项越界、有题未作答（错误信息要指出缺哪题）。
pub fn parse_multi_answer(
    questions: &[AskUserQuestion],
    text: &str,
) -> Result<Vec<(String, String)>, String> {
    if questions.is_empty() {
        return Err("没有待答的问题".to_string());
    }

    // 合法 token → (question_id, option_text)，key 大写
    let mut token_map: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for q in questions {
        for (i, opt) in q.options.iter().enumerate().take(26) {
            let letter = (b'A' + i as u8) as char;
            let token = format!("{}{letter}", q.id).to_ascii_uppercase();
            token_map.insert(token, (q.id.clone(), opt.clone()));
        }
    }

    // 按长度降序，贪心匹配（避免 "1" 与 "10" 前缀歧义时优先长 token）
    let mut tokens_by_len: Vec<String> = token_map.keys().cloned().collect();
    tokens_by_len.sort_by(|a, b| b.len().cmp(&a.len()));

    // 归一化：中英文逗号 → 空格，去其它分隔符时仍可连续匹配
    let normalized: String = text
        .chars()
        .map(|c| match c {
            '，' | ',' | '；' | ';' | '、' | '\n' | '\t' => ' ',
            _ => c,
        })
        .collect();
    let upper = normalized.to_ascii_uppercase();
    // 去掉空格后做无分隔扫描，同时也按空白分词
    let compact: String = upper.chars().filter(|c| !c.is_whitespace()).collect();

    let mut found: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut rest = compact.as_str();
    while !rest.is_empty() {
        let mut matched = false;
        for tok in &tokens_by_len {
            if rest.starts_with(tok.as_str()) {
                if let Some((qid, opt)) = token_map.get(tok) {
                    // 同题多次作答：后者覆盖前者
                    found.insert(qid.clone(), opt.clone());
                }
                rest = &rest[tok.len()..];
                matched = true;
                break;
            }
        }
        if !matched {
            // 跳过无法识别的单个字符，避免卡死
            let mut chars = rest.chars();
            let bad = chars.next().unwrap_or('?');
            rest = chars.as_str();
            // 若整串都扫完仍无任何匹配，后面统一报错
            let _ = bad;
        }
    }

    if found.is_empty() {
        let example = questions
            .iter()
            .map(|q| format!("{}A", q.id))
            .collect::<Vec<_>>()
            .join(" ");
        return Err(format!(
            "无法解析作答。请按「{example}」格式回复（题号+选项字母），或点卡片按钮。"
        ));
    }

    // 检查漏题
    let mut missing = Vec::new();
    for q in questions {
        if !found.contains_key(&q.id) {
            missing.push(q.id.clone());
        }
    }
    if !missing.is_empty() {
        let ids = missing
            .iter()
            .map(|id| format!("第 {id} 题"))
            .collect::<Vec<_>>()
            .join("、");
        return Err(format!(
            "还没答完：缺 {ids}。请补全后重发，或点卡片按钮逐题作答。"
        ));
    }

    // 按 questions 顺序输出
    Ok(questions
        .iter()
        .filter_map(|q| found.get(&q.id).map(|opt| (q.id.clone(), opt.clone())))
        .collect())
}

/// 从 questions + pairs 生成 "1A 2B" token 串（用于 answer 列 / final_answer）。
pub fn format_multi_answer_token_string(
    questions: &[AskUserQuestion],
    pairs: &[(String, String)],
) -> String {
    let mut tokens = Vec::new();
    for (qid, opt_text) in pairs {
        if let Some(q) = questions.iter().find(|q| &q.id == qid) {
            if let Some(i) = q.options.iter().position(|o| o == opt_text) {
                if i < 26 {
                    let letter = (b'A' + i as u8) as char;
                    tokens.push(format!("{qid}{letter}"));
                }
            }
        }
    }
    tokens.join(" ")
}

/// 人类可读 tool_result：`用哪个分支？→ main；要跑测试吗？→ 要`
pub fn format_multi_answer_human(
    questions: &[AskUserQuestion],
    pairs: &[(String, String)],
) -> String {
    let mut parts = Vec::new();
    for (qid, opt_text) in pairs {
        let q_text = questions
            .iter()
            .find(|q| &q.id == qid)
            .map(|q| q.question.as_str())
            .unwrap_or(qid.as_str());
        // 题干取首行，避免多行题干把 tool_result 撑爆
        let q_one: String = q_text
            .lines()
            .next()
            .unwrap_or(q_text)
            .chars()
            .take(40)
            .collect();
        parts.push(format!("{q_one}→ {opt_text}"));
    }
    parts.join("；")
}

/// 交互单是否已过期：`expires_at` 为 None 永不过期；为过去时刻则过期。
/// TTL 写在行上（微信 5min / 飞书 NULL），不再按 source 硬编码。
fn interaction_expired(expires_at: Option<chrono::DateTime<chrono::Utc>>) -> bool {
    match expires_at {
        None => false,
        Some(t) => chrono::Utc::now() > t,
    }
}

/// 从 pending JSON 的 expires_at 字段判断是否过期（RFC3339 字符串）。
fn pending_expires_at_is_past(pending: &serde_json::Value) -> bool {
    let expires = pending["expires_at"].as_str().and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.with_timezone(&chrono::Utc))
    });
    interaction_expired(expires)
}

/// 处理 quant 高成本工具的恢复确认：解析用户回复，授予授权，并返回给模型的 ToolResult 文案。
async fn handle_quant_confirmation(
    state: &Arc<AppState>,
    session_id: &str,
    source: &str,
    pending: &serde_json::Value,
    text: &str,
) -> String {
    // 超时读交互单 expires_at，不再按 source 硬编码 5 分钟
    if pending_expires_at_is_past(pending) {
        return "待确认单已超时（5 分钟），未执行高成本量化操作。如需执行请重新发起工具调用。"
            .to_string();
    }

    let (grant, summary) = parse_quant_confirmation(text, source);
    if grant == 0 {
        return "用户未确认，停止高成本量化操作。".to_string();
    }

    let remaining = state.quant_grant_store.grant(session_id, grant);
    format!(
        "{}。本会话剩余授权 {} 次，请重新发起工具调用。",
        summary, remaining
    )
}

/// 解析 quant 确认回复。返回 (授权次数, 可读摘要)。
fn parse_quant_confirmation(text: &str, source: &str) -> (u32, String) {
    let normalized = normalize_quant_confirm(text);

    let whitelist = ["确认", "好的", "是", "ok", "同意"];
    if whitelist.iter().any(|w| normalized == *w) {
        return (1, "用户已确认，本次高成本量化操作已授权".to_string());
    }

    // 微信入口无批量授权
    if source != "weixin" {
        // 飞书卡片按钮的文案，等价于「确认50次」。
        // 与打字路径共用同一个 grant 上限，不引入第二套配额语义。
        // 用 trim 后的原文判断（normalize 会去空白/小写，中文不受影响）。
        if text.trim() == "本会话全部同意" {
            return (50, "用户已确认批量授权本会话后续高成本量化操作".to_string());
        }
        for prefix in ["确认", "允许"] {
            if let Some(rest) = normalized.strip_prefix(prefix) {
                if let Some(num_part) = rest.strip_suffix("次") {
                    if let Ok(n) = num_part.parse::<u32>() {
                        let n = n.clamp(1, 50);
                        return (n, format!("用户已确认批量授权 {} 次高成本量化操作", n));
                    }
                }
            }
        }
    }

    (0, "用户未确认".to_string())
}

/// 归一化确认文本：去首尾空白、去内部空白、全半角转换、小写。
fn normalize_quant_confirm(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '０' => '0',
            '１' => '1',
            '２' => '2',
            '３' => '3',
            '４' => '4',
            '５' => '5',
            '６' => '6',
            '７' => '7',
            '８' => '8',
            '９' => '9',
            'Ａ'..='Ｚ' => ((c as u32 - 'Ａ' as u32) + 'a' as u32) as u8 as char,
            'ａ'..='ｚ' => ((c as u32 - 'ａ' as u32) + 'a' as u32) as u8 as char,
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

fn extract_event_type(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::TextDelta { .. } => "text_delta",
        AgentEvent::ToolStart { .. } => "tool_start",
        AgentEvent::ToolResult { .. } => "tool_result",
        AgentEvent::TurnComplete => "turn_complete",
        AgentEvent::Error { .. } => "error",
        AgentEvent::Thinking { .. } => "thinking",
        AgentEvent::WorkerSpawned { .. } => "worker_spawned",
        AgentEvent::WorkerCompleted { .. } => "worker_completed",
        AgentEvent::Verification { .. } => "verification",
        AgentEvent::LoopDetected { .. } => "loop_detected",
        AgentEvent::TokenWarning { .. } => "token_warning",
        AgentEvent::CompressionTriggered { .. } => "compression_triggered",
        AgentEvent::Metrics { .. } => "metrics",
        AgentEvent::ToolMetrics { .. } => "tool_metrics",
        AgentEvent::ProviderFallback { .. } => "provider_fallback",
        AgentEvent::SpecUpdated { .. } => "spec_updated",
        AgentEvent::TaskUpdated { .. } => "task_updated",
        AgentEvent::ArtifactUpdated { .. } => "artifact_updated",
        AgentEvent::AskUser { .. } => "ask_user",
        AgentEvent::SuggestedActions { .. } => "suggested_actions",
        AgentEvent::ExploreComplete { .. } => "explore_complete",
        AgentEvent::GenerateComplete { .. } => "generate_complete",
        AgentEvent::LlmRequest { .. } => "llm_request",
        AgentEvent::LlmResponse { .. } => "llm_response",
        AgentEvent::LlmRetry { .. } => "llm_retry",
        AgentEvent::LlmFailed { .. } => "llm_failed",
        AgentEvent::ToolOutputDelta { .. } => "tool_output_delta",
        AgentEvent::RunStarted { .. } => "run_started",
        AgentEvent::RunCompleted { .. } => "run_completed",
        AgentEvent::RunFailed { .. } => "run_failed",
        AgentEvent::RunCancelled { .. } => "run_cancelled",
        AgentEvent::TurnStarted { .. } => "turn_started",
        AgentEvent::TurnCompleted { .. } => "turn_completed",
        AgentEvent::FileChanged { .. } => "file_changed",
        AgentEvent::PermissionRequested { .. } => "permission_requested",
        AgentEvent::PermissionDenied { .. } => "permission_denied",
        AgentEvent::ContextAssembled { .. } => "context_assembled",
        AgentEvent::PlanUpdated { .. } => "plan_updated",
        AgentEvent::VerificationStarted { .. } => "verification_started",
        AgentEvent::VerificationCompleted { .. } => "verification_completed",
    }
}

/// 审计表保留完整 LLM 请求/响应；实时 SSE 只发送轻量摘要，避免每轮重复传输
/// 完整上下文和工具 schema，拖慢桌面端与断线恢复缓冲。
pub(crate) fn event_for_stream(event: &AgentEvent) -> AgentEvent {
    let mut stream_event = event.clone();
    match &mut stream_event {
        AgentEvent::LlmRequest {
            system,
            messages,
            tool_definitions,
            ..
        } => {
            *system = system.as_deref().map(|text| preview_chars(text, 500));
            messages.clear();
            tool_definitions.clear();
        }
        AgentEvent::LlmResponse { content, .. } => content.clear(),
        AgentEvent::ToolResult { content, .. } => {
            *content = preview_chars(content, 12_000);
        }
        _ => {}
    }
    stream_event
}

fn preview_chars(text: &str, max_chars: usize) -> String {
    let Some((end, _)) = text.char_indices().nth(max_chars) else {
        return text.to_string();
    };
    format!("{}\n...[实时流已截断，完整内容见调用链记录]", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_agent_loads_shared_sync_protocol() {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("server crate should be inside project root")
            .to_string_lossy()
            .into_owned();
        let segments = load_server_agent_instructions(&Some(project_root), true);
        let prompt = code_agent::build_system_prompt(&segments);

        assert!(prompt.contains("Server Agent 双向 Git 同步协议"));
        assert!(prompt.contains("协议版本："));
        assert!(prompt.contains("wananyun 不依赖 GitHub 网络"));
        assert!(prompt.contains("始终排除 `client/`"));
    }

    #[test]
    fn test_normalize_quant_confirm() {
        assert_eq!(normalize_quant_confirm("  确认  "), "确认");
        assert_eq!(normalize_quant_confirm("确　认"), "确认");
        assert_eq!(normalize_quant_confirm("ＯＫ"), "ok");
        assert_eq!(normalize_quant_confirm("确认３次"), "确认3次");
    }

    #[test]
    fn test_parse_quant_confirmation_whitelist() {
        for text in [
            "确认",
            "  确认  ",
            "好的",
            "是",
            "OK",
            "ok",
            "同意",
            "　同意　",
        ] {
            let (n, summary) = parse_quant_confirmation(text, "trace_chat");
            assert_eq!(n, 1, "failed for: {}", text);
            assert!(summary.contains("已确认"), "failed for: {}", text);
        }
    }

    #[test]
    fn test_parse_quant_confirmation_batch() {
        let (n, summary) = parse_quant_confirmation("确认5次", "trace_chat");
        assert_eq!(n, 5);
        assert!(summary.contains("5 次"));

        let (n, _) = parse_quant_confirmation("允许10次", "trace_chat");
        assert_eq!(n, 10);

        let (n, _) = parse_quant_confirmation("确认100次", "trace_chat");
        assert_eq!(n, 50);

        let (n, _) = parse_quant_confirmation("确认0次", "trace_chat");
        assert_eq!(n, 1);
    }

    #[test]
    fn test_parse_quant_confirmation_weixin_no_batch() {
        let (n, _) = parse_quant_confirmation("确认5次", "weixin");
        assert_eq!(n, 0);

        let (n, _) = parse_quant_confirmation("确认", "weixin");
        assert_eq!(n, 1);

        // 微信也不能靠「本会话全部同意」绕过批量限制
        let (n, _) = parse_quant_confirmation("本会话全部同意", "weixin");
        assert_eq!(n, 0);
    }

    #[test]
    fn test_parse_quant_confirmation_batch_agree_button() {
        let (n, summary) = parse_quant_confirmation("本会话全部同意", "feishu");
        assert_eq!(n, 50);
        assert!(summary.contains("批量授权"));

        let (n, _) = parse_quant_confirmation("本会话全部同意", "trace_chat");
        assert_eq!(n, 50);
    }

    #[test]
    fn test_parse_quant_confirmation_reject() {
        let (n, _) = parse_quant_confirmation("不要", "trace_chat");
        assert_eq!(n, 0);

        let (n, _) = parse_quant_confirmation("再想想", "weixin");
        assert_eq!(n, 0);
    }

    fn sample_multi_questions() -> Vec<AskUserQuestion> {
        vec![
            AskUserQuestion {
                id: "1".into(),
                question: "用哪个分支？".into(),
                options: vec!["main".into(), "dev".into()],
            },
            AskUserQuestion {
                id: "2".into(),
                question: "要跑测试吗？".into(),
                options: vec!["要".into(), "不要".into()],
            },
        ]
    }

    #[test]
    fn flatten_question_options_tokens() {
        let flat = flatten_question_options(&sample_multi_questions());
        assert_eq!(flat, vec!["1A", "1B", "2A", "2B"]);
    }

    #[test]
    fn parse_multi_answer_space_separated() {
        let qs = sample_multi_questions();
        let pairs = parse_multi_answer(&qs, "1A 2B").unwrap();
        assert_eq!(pairs[0], ("1".into(), "main".into()));
        assert_eq!(pairs[1], ("2".into(), "不要".into()));
        assert_eq!(format_multi_answer_token_string(&qs, &pairs), "1A 2B");
    }

    #[test]
    fn parse_multi_answer_no_sep_lowercase() {
        let pairs = parse_multi_answer(&sample_multi_questions(), "1a2b").unwrap();
        assert_eq!(pairs[0].1, "main");
        assert_eq!(pairs[1].1, "不要");
    }

    #[test]
    fn parse_multi_answer_chinese_comma() {
        let pairs = parse_multi_answer(&sample_multi_questions(), "1A，2B").unwrap();
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn parse_multi_answer_missing_question() {
        let err = parse_multi_answer(&sample_multi_questions(), "1A").unwrap_err();
        assert!(err.contains("第 2 题"), "err={err}");
    }

    #[test]
    fn parse_multi_answer_unknown_question() {
        // 3A 非法被跳过；1A 2B 齐全 → 仍成功
        assert!(parse_multi_answer(&sample_multi_questions(), "3A 1A 2B").is_ok());
        // 只有非法题号 → 无法解析或缺题
        let err = parse_multi_answer(&sample_multi_questions(), "3A").unwrap_err();
        assert!(err.contains("无法解析") || err.contains("缺"), "err={err}");
    }

    #[test]
    fn parse_multi_answer_option_out_of_range() {
        // 第 1 题只有 A/B，1C 非法
        let err = parse_multi_answer(&sample_multi_questions(), "1C 2A").unwrap_err();
        assert!(
            err.contains("无法解析") || err.contains("缺") || err.contains("第 1"),
            "err={err}"
        );
    }

    #[test]
    fn parse_multi_answer_garbage() {
        let err = parse_multi_answer(&sample_multi_questions(), "随便写点啥").unwrap_err();
        assert!(err.contains("无法解析"), "err={err}");
    }

    #[test]
    fn format_multi_answer_human_readable() {
        let qs = sample_multi_questions();
        let pairs = parse_multi_answer(&qs, "1A 2A").unwrap();
        let human = format_multi_answer_human(&qs, &pairs);
        assert!(human.contains("用哪个分支？→ main"));
        assert!(human.contains("要跑测试吗？→ 要"));
    }

    #[test]
    fn truncate_answer_for_column_caps_at_60() {
        let long: String = "1A ".repeat(30);
        assert!(long.chars().count() > 60);
        let t = truncate_answer_for_column(&long);
        assert_eq!(t.chars().count(), 60);
    }

    #[test]
    fn test_quant_research_prompt_injected_when_quant_enabled() {
        let (skills, segments) = quant_research_prompt_inputs(true);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "quant-research");

        let runtime = code_agent::RuntimeContext {
            permission_mode: "workspace-write".to_string(),
            approval_policy: "auto".to_string(),
            writable_roots: vec![],
            network_policy: "restricted".to_string(),
            tools: vec![],
            skills,
        };
        let (prompt, named) =
            code_agent::build_layered_prompt(Some("base prompt"), Some(&runtime), None, &segments);
        // project segment 存在，且 runtime 技能索引包含 quant-research
        assert!(
            named.iter().any(|s| s.name == "project"),
            "project segment should exist"
        );
        assert!(
            prompt.contains("quant-research"),
            "prompt should reference quant-research"
        );
        assert!(
            prompt.contains("S1 | 用户停止 / 会话结束"),
            "prompt should contain stop condition S1"
        );
        assert!(
            prompt.contains("不得先调用通用 `ask_user`"),
            "prompt should route high-cost confirmation through the runtime gate"
        );
        assert!(
            prompt.contains("不得为了满足落表步骤而编造 finding"),
            "prompt should keep inconclusive results out of the gap table"
        );
    }

    #[test]
    fn test_chat_turn_complete_waits_for_message_persistence() {
        let mut buffer = EventBuffer::new();
        push_chat_stream_event(
            &mut buffer,
            AgentEvent::TextDelta {
                text: "waiting".to_string(),
            },
        );
        push_chat_stream_event(&mut buffer, AgentEvent::TurnComplete);

        assert_eq!(buffer.events.len(), 1);
        assert!(!buffer.completed);
        assert!(
            !buffer
                .events
                .iter()
                .any(|entry| matches!(entry.event, AgentEvent::TurnComplete)),
            "消息持久化前不得向客户端暴露 TurnComplete"
        );

        finish_chat_stream(&mut buffer);
        assert!(buffer.completed);
        assert!(matches!(
            buffer.events.last().map(|entry| &entry.event),
            Some(AgentEvent::TurnComplete)
        ));
    }

    #[test]
    fn test_quant_research_prompt_not_injected_when_quant_disabled() {
        let (skills, segments) = quant_research_prompt_inputs(false);
        assert!(skills.is_empty());
        assert!(segments.is_empty());

        let runtime = code_agent::RuntimeContext {
            permission_mode: "workspace-write".to_string(),
            approval_policy: "auto".to_string(),
            writable_roots: vec![],
            network_policy: "restricted".to_string(),
            tools: vec![],
            skills,
        };
        let (prompt, named) =
            code_agent::build_layered_prompt(Some("base prompt"), Some(&runtime), None, &segments);
        assert!(!named.iter().any(|s| s.name == "project"));
        assert!(!prompt.contains("quant-research"));
        assert!(!prompt.contains("S1 | 用户停止 / 会话结束"));
    }

    #[test]
    fn test_parse_quant_confirmation_summary_is_non_trading() {
        // 验收 #5：确认/授权文案本身不得含交易指令类措辞。
        let forbidden = ["已下单", "建议买入", "买入建议", "建议卖出", "卖出建议"];
        for text in ["确认", "确认5次", "允许10次"] {
            for source in ["trace_chat", "weixin"] {
                let (_, summary) = parse_quant_confirmation(text, source);
                for phrase in forbidden {
                    assert!(
                        !summary.contains(phrase),
                        "summary '{}' contains forbidden phrase '{}' for source={} text={}",
                        summary,
                        phrase,
                        source,
                        text
                    );
                }
            }
        }
    }

    #[test]
    fn test_quant_research_skill_forbids_trading_wording() {
        // 验收 #5：SKILL.md 必须包含文案禁忌与投资/交易免责声明。
        assert!(
            QUANT_RESEARCH_SKILL_MD.contains("不输出交易指令"),
            "SKILL.md should forbid trading instructions"
        );
        assert!(
            QUANT_RESEARCH_SKILL_MD.contains("非投资建议"),
            "SKILL.md should disclaim investment advice"
        );
        assert!(
            QUANT_RESEARCH_SKILL_MD.contains("文案禁忌"),
            "SKILL.md should list wording taboos"
        );
    }

    #[test]
    fn test_quant_research_session_prompt_forbids_trading_and_cwd() {
        let text = quant_research_session_prompt();
        assert!(
            text.contains("不要输出买卖指令"),
            "research session prompt should forbid trading wording"
        );
        assert!(
            text.contains("不要暗示自动交易"),
            "research session prompt should forbid auto-trading implication"
        );
        // 必须明确无工作目录 / 无 shell 文件工具，避免与远程执行 prompt 冲突。
        assert!(
            text.contains("没有工作目录"),
            "research session prompt should state there is no work dir"
        );
        assert!(
            !text.contains("本地桌面"),
            "research session prompt must not claim desktop work dir"
        );
        assert!(
            !text.contains("远程执行"),
            "research session prompt must not claim remote exec"
        );
    }

    #[test]
    fn test_quant_confirm_timeout_message_is_safe() {
        // 微信入口 5 分钟超时文案不应包含交易指令类措辞。
        let msg = "待确认单已超时（5 分钟），未执行高成本量化操作。如需执行请重新发起工具调用。";
        for phrase in ["已下单", "建议买入", "建议卖出"] {
            assert!(
                !msg.contains(phrase),
                "timeout message contains forbidden phrase '{}'",
                phrase
            );
        }
    }

    #[test]
    fn test_interaction_expired_from_expires_at() {
        // expires_at 为 None：永不过期（飞书/网页）
        assert!(!interaction_expired(None));

        // 未来时刻：未过期
        let future = chrono::Utc::now() + chrono::Duration::minutes(5);
        assert!(!interaction_expired(Some(future)));

        // 过去时刻：已过期（微信 5min TTL 写进行后的形态）
        let past = chrono::Utc::now() - chrono::Duration::minutes(10);
        assert!(interaction_expired(Some(past)));
    }

    #[test]
    fn test_pending_expires_at_is_past_json() {
        let past = (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
        let future = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        assert!(pending_expires_at_is_past(
            &serde_json::json!({ "expires_at": past })
        ));
        assert!(!pending_expires_at_is_past(
            &serde_json::json!({ "expires_at": future })
        ));
        assert!(!pending_expires_at_is_past(&serde_json::json!({})));
        assert!(!pending_expires_at_is_past(
            &serde_json::json!({ "expires_at": serde_json::Value::Null })
        ));
    }
}
