//! 截图类 agent 工具：网页快照（server 本机 headless Chrome）。
//! 工具产出 PNG 写到 server 临时目录，返回绝对路径；agent 最终回复里按 [file:绝对路径] 约定回传用户。

use anyhow::Result;
use async_trait::async_trait;
use code_tools::{Tool, ToolOutput};
use serde_json::{json, Value};
use std::time::Duration;

/// 工具执行超时：websnap 自身封顶 30s，留足余量
const SNAP_TOOL_TIMEOUT: Duration = Duration::from_secs(60);

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

