//! 渠道 agent：单次 LLM 调用的轻量前置路由。
//!
//! 判断微信消息是渠道层面的寒暄/问询（直接回复），
//! 还是需要派发给 coding agent 的任务（改写后 dispatch），
//! 亦或等价于 /stop /new 的控制意图。
//! 调用失败或输出无法解析时返回 None，由调用方降级为直接 dispatch。
//!
//! 对话记忆规范（哪些进上下文、哪些不进、何时清空）：
//!
//! 【记录 —— 有上下文价值】用户之后的指代/追问依赖这些：
//! - 普通文本消息及其渠道回复（reply/dispatch/stop 决策），见 router::handle_chat
//! - /ai 直派任务（记录任务原文 + "已派发"，用户可能追问任务进展）
//!
//! 【不记录 —— 无上下文价值】瞬时信息或固定文案，录进去只会占名额、误导后续决策：
//! - 全部斜杠命令及其输出：/菜单 /help（固定长文案）、/status（瞬时状态，很快过期）、
//!   /terms /term /send /shot /snap（终端/截图的瞬时输出）、/ls /cd（目录浏览设置）、
//!   /kimi /kstop /stop（会话控制）——见 router::handle_command，刻意不碰 history
//! - kimi <文本> 托管 CLI 交互（托管会话自己有终端上下文）
//! - pusher 推送的执行进度/最终结果（任务上下文在 coding agent 会话里，追问会 dispatch 回去）
//!
//! 【清空 —— 仅 /new】斜杠 /new 或自然语言 New 决策成功开新会话时 clear_history；
//! 其他一切情况（包括 /stop、任务完成、server 报错）都保持记忆。
//! 记忆随 server 重启丢失；单绑定上限 MAX_HISTORY_TURNS 轮，旧轮次自动淘汰，
//! 因此偶尔录入的过期状态类回复会自然滚出，无需专门清理。

use crate::provider_registry;
use crate::AppState;
use anyhow::{anyhow, Result};
use futures::StreamExt;
use hank_db::WeixinBinding;
use hank_provider::{CompletionRequest, ContentBlock, Message, Role, StreamEvent};
use serde::Deserialize;
use std::collections::VecDeque;

/// 一轮渠道对话（用户消息 + 机器人回复），作为渠道 agent 的短期记忆。
#[derive(Clone, Debug)]
pub struct ChannelTurn {
    pub user: String,
    pub assistant: String,
}

/// 每个绑定最多保留的对话轮数
const MAX_HISTORY_TURNS: usize = 12;
/// 单条历史消息截断长度，避免 prompt 膨胀
const HISTORY_MSG_MAX: usize = 800;

/// 读取某绑定的渠道对话记忆。
pub async fn history(state: &AppState, binding_id: &str) -> Vec<ChannelTurn> {
    state
        .weixin_channel_history
        .read()
        .await
        .get(binding_id)
        .map(|h| h.iter().cloned().collect())
        .unwrap_or_default()
}

/// 追加一轮对话到渠道记忆（超出上限丢弃最旧的）。
pub async fn push_history(state: &AppState, binding_id: &str, user: &str, assistant: &str) {
    let turn = ChannelTurn {
        user: truncate(user, HISTORY_MSG_MAX),
        assistant: truncate(assistant, HISTORY_MSG_MAX),
    };
    let mut map = state.weixin_channel_history.write().await;
    let h = map
        .entry(binding_id.to_string())
        .or_insert_with(VecDeque::new);
    h.push_back(turn);
    while h.len() > MAX_HISTORY_TURNS {
        h.pop_front();
    }
}

/// 清空渠道记忆（/new 开新会话时调用）。
pub async fn clear_history(state: &AppState, binding_id: &str) {
    state
        .weixin_channel_history
        .write()
        .await
        .remove(binding_id);
}

/// 渠道 agent 的路由决策。
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum ChannelAction {
    /// 渠道问题/寒暄，text 直接回给用户
    Reply { text: String },
    /// 派发任务给 coding agent：ack 立即回给用户，task 为改写后的完整指令
    Dispatch { ack: String, task: String },
    /// 停止当前任务（等价 /stop），text 为回复语
    Stop { text: String },
    /// 开启新会话（等价 /new），text 为回复语
    New { text: String },
}

impl ChannelAction {
    fn action_name(&self) -> &'static str {
        match self {
            ChannelAction::Reply { .. } => "reply",
            ChannelAction::Dispatch { .. } => "dispatch",
            ChannelAction::Stop { .. } => "stop",
            ChannelAction::New { .. } => "new",
        }
    }

    fn text_preview(&self) -> &str {
        match self {
            ChannelAction::Reply { text }
            | ChannelAction::Stop { text }
            | ChannelAction::New { text } => text,
            ChannelAction::Dispatch { task, .. } => task,
        }
    }
}

/// 让渠道 agent 决策一条消息。失败（无 provider / LLM 错误 / JSON 解析失败）返回 None。
/// history 为该绑定的近期渠道对话，用于理解「这个」「上面第 1 条」这类指代。
pub async fn decide(
    state: &AppState,
    binding: &WeixinBinding,
    session_id: Option<&str>,
    text: &str,
    history: &[ChannelTurn],
) -> Option<ChannelAction> {
    match try_decide(state, binding, session_id, text, history).await {
        Ok(action) => {
            tracing::info!(
                action = action.action_name(),
                text = %truncate(action.text_preview(), 60),
                "weixin channel decision"
            );
            Some(action)
        }
        Err(e) => {
            tracing::warn!("weixin channel agent failed, fallback to dispatch: {e:#}");
            None
        }
    }
}

async fn try_decide(
    state: &AppState,
    binding: &WeixinBinding,
    session_id: Option<&str>,
    text: &str,
    history: &[ChannelTurn],
) -> Result<ChannelAction> {
    let system = build_system_prompt(state, binding, session_id).await;

    let (record, provider) = provider_registry::resolve_default(&state.db)
        .await
        .ok_or_else(|| anyhow!("no enabled provider"))?;
    // 近期对话作为多轮上下文，当前消息放最后
    let mut messages: Vec<Message> = history
        .iter()
        .flat_map(|t| {
            [
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: t.user.clone(),
                    }],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: t.assistant.clone(),
                    }],
                },
            ]
        })
        .collect();
    messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
    });
    let req = CompletionRequest {
        model: provider_registry::resolve_default_model(&record),
        system: Some(system),
        messages,
        tools: vec![],
        max_tokens: 512,
    };

    let mut stream = provider.stream(req).await?;
    let mut output = String::new();
    while let Some(event) = stream.next().await {
        match event {
            Ok(StreamEvent::TextDelta(t)) => output.push_str(&t),
            Ok(StreamEvent::MessageEnd { .. }) => break,
            Err(e) => anyhow::bail!("stream error: {e}"),
            _ => {}
        }
    }

    parse_decision(&output)
}

/// 拼装 system prompt：角色说明 + 输出契约 + 当前渠道状态。
async fn build_system_prompt(
    state: &AppState,
    binding: &WeixinBinding,
    session_id: Option<&str>,
) -> String {
    let username = state
        .db
        .get_user_by_id(&binding.user_id)
        .await
        .ok()
        .flatten()
        .map(|u| u.username)
        .unwrap_or_else(|| "未知".to_string());

    let current = match session_id {
        Some(sid) => {
            let title = state
                .db
                .get_session(sid)
                .await
                .ok()
                .flatten()
                .map(|s| s.title)
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "(无标题)".to_string());
            let running = state.active_tasks.read().await.contains_key(sid);
            format!(
                "{title}（ID：{sid}，状态：{}）",
                if running { "执行中" } else { "空闲" }
            )
        }
        None => "无（用户还没有会话）".to_string(),
    };

    let sessions = state
        .db
        .list_sessions_by_user(&binding.user_id)
        .await
        .unwrap_or_default();
    let recent = if sessions.is_empty() {
        "无".to_string()
    } else {
        sessions
            .iter()
            .take(5)
            .enumerate()
            .map(|(i, s)| {
                let title = if s.title.is_empty() {
                    "(无标题)"
                } else {
                    &s.title
                };
                format!(
                    "{}. {}（ID：{}，更新于 {}）",
                    i + 1,
                    title,
                    s.id,
                    s.updated_at.format("%m-%d %H:%M")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    // 桌面 client 在线状态：决定本地执行类任务能否 dispatch
    let client_status = match crate::remote_exec::pick_online_client(state, &binding.user_id).await
    {
        Some(c) => format!(
            "在线（主机：{}，工作目录：{}）",
            c.hostname.as_deref().unwrap_or("未知"),
            c.work_dir.as_deref().unwrap_or("未设置")
        ),
        None => "离线".to_string(),
    };

    format!(
        "你是 Trace 的微信渠道助手。Trace 是一个 AI agent 平台，用户通过微信与你对话，\
        开发任务由背后的 agent 执行。\n\
        \n\
        你的职责：判断用户消息的意图，只输出一个 JSON 决策，不要输出任何其他内容，不要用 markdown 代码块包裹。\n\
        \n\
        可选动作：\n\
        - {{\"action\":\"reply\",\"text\":\"...\"}}：渠道层面的寒暄或问询（问候、问绑定是否成功、问当前任务状态、\
        问你能做什么、问最近的会话等），text 直接回复给用户。\n\
        - {{\"action\":\"dispatch\",\"ack\":\"...\",\"task\":\"...\"}}：用户想派发开发任务、问技术问题、\
        或继续之前会话的话题。ack 是立即发给用户的简短确认语（如\"收到，开始处理\"）；\
        task 是发给 coding agent 的完整指令，可在保留原意的基础上改写得更清晰完整。\n\
        - {{\"action\":\"stop\",\"text\":\"...\"}}：用户想停止当前正在执行的任务（等价 /stop 命令），text 为回复语。\n\
        - {{\"action\":\"new\",\"text\":\"...\"}}：用户想开启新会话（等价 /new 命令），text 为回复语。\n\
        \n\
        当前渠道状态：\n\
        - 绑定用户：{username}\n\
        - 当前会话：{current}\n\
        - 桌面 client：{client_status}\n\
        - 最近会话：\n{recent}\n\
        \n\
        要求：\n\
        - 对话中会附带最近的聊天记录（含你之前的回复），\
        用户说「这个」「上面第 1 条」「我是问」这类指代或追问时，结合记录理解，不要重复回答已经答过的内容。\n\
        - text/ack 用中文，简短口语化；不要用 markdown 表格、代码块（微信里显示不好看）。\n\
        - 桌面 client 在线时，派发的任务在用户本地机器执行，按现有逻辑决策即可。\n\
        - 桌面 client 离线时：纯查询、闲聊、查会话状态等不需要操作用户本地文件的消息，正常 reply 或 dispatch\
        （在服务器端执行）；涉及本地文件、代码修改、本地命令的任务不要 dispatch，用 reply 告知\
        \"你的桌面 client 不在线，请打开 Trace 客户端后重试\"。\n\
        - 截图/网页快照类请求（如\"截图 kimi 官网\"、\"截图给我看看\"）属于任务，正常 dispatch 即可，\
        server 端自带截图能力，不要求桌面 client 在线；改写 task 时把\"kimi 官网\"这类口语补全成完整 URL 更好。\n\
        - 用户消息若是在回应机器人上一条的回复或命令结果（如\"不是啊\"\"不对\"\"哪有这个\"这类纠正、质疑、追问），\
        选 reply 解释或引导正确使用命令（如 /ls 浏览目录、/cd 设置目录、/new 开新会话），不要 dispatch。\n\
        - 用户问\"你能做什么\"\"有什么功能\"时，只简要概述核心能力（派任务、追问、进度推送、文件回传），\
        并提示发送 /菜单 查看全部命令；不要凭印象罗列具体命令或功能清单，以免与实际能力不符。\n\
        - 用户明确要换个不相干的新话题时，可提示发送 /new 开新会话，避免旧话题上下文干扰。\n\
        - 拿不准时选 dispatch，task 直接使用用户原文。"
    )
}

/// 解析 LLM 输出为决策：容忍代码块包裹和首尾杂散文本。
fn parse_decision(output: &str) -> Result<ChannelAction> {
    let s = output.trim();
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(s)
        .trim();
    let start = s
        .find('{')
        .ok_or_else(|| anyhow!("no JSON object in output"))?;
    let end = s
        .rfind('}')
        .ok_or_else(|| anyhow!("no JSON object in output"))?;
    let action: ChannelAction = serde_json::from_str(&s[start..=end])?;
    Ok(action)
}

fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}
