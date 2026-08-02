//! 截图类 agent 工具：网页快照（server 本机 headless Chrome）+ 终端屏幕截图（client 快照 server 渲染）。
//! 工具产出 PNG 写到 server 临时目录，返回绝对路径；agent 最终回复里按 [file:绝对路径] 约定回传用户。

use crate::AppState;
use anyhow::Result;
use async_trait::async_trait;
use code_tools::{Tool, ToolOutput};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// 工具执行超时：websnap 自身封顶 30s，留足余量
const SNAP_TOOL_TIMEOUT: Duration = Duration::from_secs(60);
/// 远程读终端快照超时（与微信 /shot 命令一致）
const TERM_READ_TIMEOUT: Duration = Duration::from_secs(15);

fn err_out(content: impl Into<String>) -> ToolOutput {
    ToolOutput {
        content: content.into(),
        is_error: true,
    }
}

/// 写临时 PNG，返回绝对路径（pusher 按 [file:] 约定读取回传）
fn save_png(png: &[u8], kind: &str) -> Result<String> {
    let path = std::env::temp_dir().join(format!("trace-{kind}-{}.png", uuid::Uuid::new_v4()));
    std::fs::write(&path, png)?;
    Ok(path.to_string_lossy().into_owned())
}

fn saved_text(path: &str) -> ToolOutput {
    ToolOutput {
        content: format!(
            "截图已保存：{path}\n最终回复必须单独一行写 [file:{path}]，系统才会把图片发给用户。"
        ),
        is_error: false,
    }
}

// ─── 网页快照 ────────────────────────────────────────────────────────────────

pub struct WebSnapshotTool {
    chrome_path: Option<String>,
}

impl WebSnapshotTool {
    pub fn new(chrome_path: Option<String>) -> Self {
        Self { chrome_path }
    }
}

#[async_trait]
impl Tool for WebSnapshotTool {
    fn name(&self) -> &str {
        "web_snapshot"
    }

    fn description(&self) -> &str {
        "Capture a full-page screenshot of a web page as a PNG image (headless Chrome on the server). \
         Use when the user asks to screenshot/snap a website or URL (e.g. \"截图 kimi 官网\"). \
         Runs on the server, no local client needed. Returns the absolute path of the PNG file; \
         the final reply MUST include [file:<absolute path>] on its own line to send the image to the user."
    }

    fn timeout(&self) -> Duration {
        SNAP_TOOL_TIMEOUT
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Complete http(s) URL of the page to capture. \
                     Convert colloquial names to full URLs first (e.g. \"kimi 官网\" -> \"https://www.kimi.com\")."
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let url = input["url"].as_str().map(str::trim).unwrap_or_default();
        if url.is_empty() {
            return Ok(err_out("Error: url is required"));
        }
        match crate::websnap::snap_url(self.chrome_path.as_deref(), url).await {
            Ok(png) => Ok(saved_text(&save_png(&png, "snap")?)),
            Err(e) => Ok(err_out(format!("网页截图失败：{e:#}"))),
        }
    }
}

// ─── 终端屏幕截图 ────────────────────────────────────────────────────────────

pub struct TerminalSnapshotTool {
    state: Arc<AppState>,
    user_id: String,
}

impl TerminalSnapshotTool {
    pub fn new(state: Arc<AppState>, user_id: String) -> Self {
        Self { state, user_id }
    }

    /// 派发 terminal_* 工具调用到 client，取文本结果
    async fn dispatch_text(
        &self,
        client_id: &str,
        tool: &str,
        input: Value,
    ) -> Result<String, String> {
        match crate::remote_exec::dispatch_tool_call(
            &self.state,
            &self.user_id,
            client_id,
            tool,
            input,
            TERM_READ_TIMEOUT,
        )
        .await
        {
            Ok(r) if !r.is_error => Ok(r.content),
            Ok(r) => Err(r.content),
            Err(e) => Err(format!("{e:#}")),
        }
    }
}

/// 按 id 前缀解析终端会话：不给前缀且只有一个会话时直接用它；无法唯一确定时返回候选列表文本
fn resolve_prefix(terms: &[Value], prefix: Option<&str>) -> Result<String, String> {
    let ids: Vec<&str> = terms.iter().filter_map(|t| t["id"].as_str()).collect();
    let candidates = |ids: &[&str]| {
        terms
            .iter()
            .filter(|t| t["id"].as_str().is_some_and(|id| ids.contains(&id)))
            .map(|t| {
                let id = t["id"].as_str().unwrap_or("?");
                let fg = t["foreground_cmd"].as_str().unwrap_or("?");
                format!("- [{}] {fg}", &id[..id.len().min(8)])
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    match prefix {
        Some(p) => {
            let matches: Vec<&str> = ids.iter().copied().filter(|id| id.starts_with(p)).collect();
            match matches.len() {
                1 => Ok(matches[0].to_string()),
                0 => Err(format!(
                    "没有找到 id 以 {p} 开头的终端会话：\n{}",
                    candidates(&ids)
                )),
                _ => Err(format!(
                    "id 前缀 {p} 匹配到多个会话，请多输几位：\n{}",
                    candidates(&matches)
                )),
            }
        }
        None if ids.len() == 1 => Ok(ids[0].to_string()),
        None => Err(format!(
            "有多个终端会话，请用 id 前缀指定要截图的会话：\n{}",
            candidates(&ids)
        )),
    }
}

#[async_trait]
impl Tool for TerminalSnapshotTool {
    fn name(&self) -> &str {
        "terminal_snapshot"
    }

    fn description(&self) -> &str {
        "Capture a screenshot of a terminal session running on the user's desktop client, rendered as a PNG image. \
         Use when the user asks to screenshot the terminal / show the terminal screen (e.g. \"截图给我看看\"). \
         Requires the desktop client to be online. Returns the absolute path of the PNG file; \
         the final reply MUST include [file:<absolute path>] on its own line to send the image to the user."
    }

    fn timeout(&self) -> Duration {
        SNAP_TOOL_TIMEOUT
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Optional terminal id prefix (see terminal_list). \
                     Omit when there is only one terminal session."
                }
            }
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let prefix = input["id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(client) = crate::remote_exec::pick_online_client(&self.state, &self.user_id).await
        else {
            return Ok(err_out("桌面 client 不在线，无法截取终端屏幕"));
        };
        // 列会话并解析 id 前缀
        let list = match self
            .dispatch_text(&client.id, "terminal_list", json!({}))
            .await
        {
            Ok(c) => c,
            Err(e) => return Ok(err_out(format!("获取终端列表失败：{e}"))),
        };
        let terms: Vec<Value> = serde_json::from_str(&list).unwrap_or_default();
        if terms.is_empty() {
            return Ok(err_out("client 上当前没有终端会话"));
        }
        let id = match resolve_prefix(&terms, prefix) {
            Ok(id) => id,
            Err(msg) => return Ok(err_out(msg)),
        };
        // raw=true：client 返回带 SGR 转义码的当前屏幕快照，server 本地渲染成 PNG
        let snap = match self
            .dispatch_text(
                &client.id,
                "terminal_read",
                json!({ "id": id, "raw": true }),
            )
            .await
        {
            Ok(c) => c,
            Err(e) => return Ok(err_out(format!("读取终端屏幕失败：{e}"))),
        };
        match crate::termshot::render_png(&snap) {
            Ok(png) => Ok(saved_text(&save_png(&png, "termshot")?)),
            Err(e) => Ok(err_out(format!("终端截图渲染失败：{e:#}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(id: &str, fg: &str) -> Value {
        json!({ "id": id, "foreground_cmd": fg })
    }

    #[test]
    fn resolve_single_without_prefix() {
        let terms = vec![term("a1b2c3d4", "zsh")];
        assert_eq!(resolve_prefix(&terms, None).unwrap(), "a1b2c3d4");
    }

    #[test]
    fn resolve_multiple_requires_prefix() {
        let terms = vec![term("a1b2c3d4", "zsh"), term("e5f6a7b8", "vim")];
        let msg = resolve_prefix(&terms, None).unwrap_err();
        assert!(msg.contains("多个终端会话"));
        assert!(msg.contains("a1b2c3d4"));
        assert_eq!(resolve_prefix(&terms, Some("e5")).unwrap(), "e5f6a7b8");
        assert!(resolve_prefix(&terms, Some("zz"))
            .unwrap_err()
            .contains("没有找到"));
    }
}
